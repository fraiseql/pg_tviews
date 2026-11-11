//! Run the work of `pg_tviews` as the role that owns it.
//!
//! The flush refreshes TVIEWs on behalf of whichever role wrote to a base table.
//! As `REFRESH MATERIALIZED VIEW` does, every read and write of a `tv_*` table in
//! the flush runs as that table's owner, inside a security-restricted operation
//! and with `search_path` set to `pg_catalog, pg_temp`. The writer then needs no
//! privilege on the TVIEW, its backing view or the tables the view reads, and
//! cannot get the owner to run a function it planted on its `search_path`.
//! Every refresh also renders values under fixed settings ([`RENDER_SETTINGS`]),
//! not the writer's.
//!
//! The registration catalog is writable only by the extension's owner. A caller
//! allowed to change a TVIEW (checked with [`require_owner`] beforehand) has its
//! catalog write run as the extension's owner, the same way.

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::panic::{CaughtError, ErrorReport};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;
use pgrx::{PgLogLevel, PgTryBuilder};
use std::cell::RefCell;
use std::ffi::{CStr, CString};

/// The role an owner frame runs as: a TVIEW's owner, named by its table or
/// entity, or the extension's owner, for writes to its catalog. A TVIEW's owner
/// carries the TVIEW's text search configuration, which its frames pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    role: Oid,
    text_search: Option<&'static CStr>,
}

impl Owner {
    /// The owner of the relation `table`.
    ///
    /// # Errors
    /// Returns an error if the relation does not exist.
    pub fn of_table(table: Oid) -> TViewResult<Self> {
        relation_owner(table).map(Self::of_role)
    }

    /// The owner of the `pg_tviews` extension, who owns its catalog.
    ///
    /// # Errors
    /// Returns an error if the extension's row cannot be read.
    pub fn of_extension() -> TViewResult<Self> {
        let owner = crate::utils::spi::connect(|client| {
            client
                .select(
                    "SELECT extowner FROM pg_catalog.pg_extension WHERE extname = 'pg_tviews'",
                    None,
                    &[],
                )?
                .first()
                .get_one::<Oid>()
        })
        .map_err(|e| TViewError::CatalogError {
            operation: "Look up the owner of pg_tviews".to_string(),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| TViewError::CatalogError {
            operation: "Look up the owner of pg_tviews".to_string(),
            pg_error: "extension pg_tviews is not installed".to_string(),
        })?;
        Ok(Self::of_role(owner))
    }

    /// The current user, the owner-to-be of a TVIEW it creates.
    ///
    /// # Errors
    /// An internal error inside an owner frame, where the current user is the
    /// frame's owner, not the caller.
    pub fn current_user() -> TViewResult<Self> {
        if OPEN_FRAMES.with(std::cell::Cell::get) > 0 {
            return Err(frame_error("the caller is read inside an owner frame"));
        }
        // SAFETY: reads backend-global state.
        Ok(Self::of_role(unsafe { pg_sys::GetUserId() }))
    }

    /// Whether this role has the privileges of `other` (is it, a member of it,
    /// or a superuser), as `ALTER TABLE` on a table `other` owns would require.
    #[must_use]
    pub fn has_privileges_of(self, other: Self) -> bool {
        // SAFETY: a role membership check by OID.
        unsafe { pg_sys::has_privs_of_role(self.role, other.role) }
    }

    /// The role's OID.
    #[must_use]
    pub const fn oid(self) -> Oid {
        self.role
    }

    /// The role `role`: one read with [`relation_owner`].
    #[must_use]
    pub const fn of_role(role: Oid) -> Self {
        Self {
            role,
            text_search: None,
        }
    }

    /// This owner, with `default_text_search_config` pinned to `config` in its
    /// frames (a TVIEW's, its creator's value), or left as it is (`None`).
    #[must_use]
    pub fn with_text_search(self, config: Option<&str>) -> Self {
        Self {
            text_search: config.and_then(intern),
            ..self
        }
    }

    /// The text search configuration its frames pin.
    #[must_use]
    pub fn text_search(self) -> Option<&'static str> {
        self.text_search.and_then(|c| c.to_str().ok())
    }
}

/// The role that called into `pg_tviews`, which privileges are decided for:
/// the current user outside every owner frame, and the current user when the
/// outermost frame opened inside one (where the current user is an owner).
#[must_use]
pub fn caller() -> Oid {
    if OPEN_FRAMES.with(std::cell::Cell::get) > 0 {
        CALLER.with(std::cell::Cell::get)
    } else {
        // SAFETY: reads backend-global state.
        unsafe { pg_sys::GetUserId() }
    }
}

/// The session's `default_text_search_config`: what a TVIEW created now keeps.
#[must_use]
pub fn session_text_search() -> Option<String> {
    // SAFETY: a setting that exists; the result is copied before it can change.
    let value =
        unsafe { pg_sys::GetConfigOption(c"default_text_search_config".as_ptr(), false, false) };
    (!value.is_null()).then(|| {
        // SAFETY: NUL-terminated when not null.
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    })
}

/// `config` as a string that lives as long as the backend: one per distinct
/// configuration name, a handful in a database.
fn intern(config: &str) -> Option<&'static CStr> {
    let config = CString::new(config).ok()?;
    INTERNED.with(|interned| {
        let mut interned = interned.borrow_mut();
        if let Some(found) = interned.iter().find(|c| **c == config.as_c_str()) {
            return Some(*found);
        }
        let leaked: &'static CStr = Box::leak(config.into_boxed_c_str());
        interned.push(leaked);
        Some(leaked)
    })
}

/// Runs work in an owner frame. SQL derived from a TVIEW, planned or run, runs
/// in its owner's frame only: the functions that plan or run it take the
/// frame's [`OwnerFrame`] token, which nothing else can make.
pub struct AsOwner;

impl AsOwner {
    /// Run `work` as `owner`. The frame decides:
    /// - who: the current user is the owner, inside a security-restricted
    ///   operation;
    /// - where names resolve: `search_path` is `pg_catalog, pg_temp`, and a caller
    ///   whose temporary schema shadows a name is refused first;
    /// - how values render: the [`RENDER_SETTINGS`], [`PLAN_SETTINGS`] and the
    ///   owner's text search configuration are pinned;
    /// - what an error reveals: to a caller without the owner's privileges, an
    ///   error the work raises or returns arrives with its SQLSTATE and without
    ///   the values it quotes; the server log has it whole.
    ///
    /// Leaving restores the caller's user, context and settings; on an error,
    /// the (sub)transaction abort restores them.
    ///
    /// # Errors
    /// What `work` returns; a temporary object of the caller's shadowing a name.
    pub fn run<T>(
        owner: Owner,
        work: impl FnOnce(&mut OwnerFrame) -> TViewResult<T>,
    ) -> TViewResult<T> {
        // The role that called into pg_tviews, not the owner of a frame this one
        // opens inside of.
        let caller = caller();
        check_temp_shadows(caller, owner.role)?;
        // Decided before the owner's work runs: nothing below reads who the
        // current user is once it fails.
        // SAFETY: a role membership check by OID.
        let privileged = unsafe { pg_sys::has_privs_of_role(caller, owner.role) };
        let mut frame = OwnerFrame::enter(owner);
        if privileged {
            return work(&mut frame);
        }
        let role = role_name(owner.role);
        let attempt = std::panic::AssertUnwindSafe(|| work(&mut frame));
        PgTryBuilder::new(move || {
            let attempt = attempt;
            (attempt.0)()
        })
        .catch_others(move |error| raise_without_values(&error, &role))
        .execute()
        .map_err(|error| {
            if error.clone().without_values() != error {
                ErrorReport::from(error.clone()).report(PgLogLevel::LOG_SERVER_ONLY);
            }
            error.without_values()
        })
    }
}

/// Raise `error`, raised by work run as `role` for a caller without its
/// privileges, with its SQLSTATE and a message naming its condition; its
/// message, detail and hint go to the server log only. No SQL runs here.
fn raise_without_values(error: &CaughtError, role: &str) -> ! {
    let report = match error {
        CaughtError::PostgresError(report)
        | CaughtError::ErrorReport(report)
        | CaughtError::RustPanic {
            ereport: report, ..
        } => report,
    };
    let code = report.sql_error_code();
    let mut logged = ErrorReport::new(
        code,
        format!(
            "work run as {role}, a TVIEW's owner, failed: {}",
            report.message()
        ),
        "pg_tviews",
    );
    if let Some(detail) = report.detail() {
        logged = logged.set_detail(detail);
    }
    logged.report(PgLogLevel::LOG_SERVER_ONLY);
    ErrorReport::new(
        code,
        format!(
            "refreshing a TVIEW as its owner {role} failed: {}",
            crate::error::condition(code)
        ),
        "pg_tviews",
    )
    .set_hint("The server log has the error; the TVIEW's owner and its members get it whole.")
    .report(PgLogLevel::ERROR);
    unreachable!("an ERROR report does not return")
}

/// The name of `role`, for messages; its OID when it has none.
fn role_name(role: Oid) -> String {
    // SAFETY: a syscache lookup by OID; null when the role is gone.
    let name = unsafe { pg_sys::GetUserNameFromId(role, true) };
    if name.is_null() {
        return role.to_u32().to_string();
    }
    // SAFETY: a NUL-terminated name in a palloc'd copy.
    crate::utils::ident::quote_if_needed(&unsafe { CStr::from_ptr(name) }.to_string_lossy())
}

/// Run `work` as the extension's owner, who owns the registration catalog: for a
/// catalog write by a caller whose right to change the TVIEW was checked.
///
/// # Errors
/// What `work` returns, or a failure to read the extension's owner.
pub fn as_extension_owner<T>(work: impl FnOnce() -> TViewResult<T>) -> TViewResult<T> {
    AsOwner::run(Owner::of_extension()?, |_| work())
}

/// Refuse to run work as `owner` for `caller` when the session's temporary schema
/// holds a relation or type named like one in another schema, unless `caller`
/// has `owner`'s privileges.
///
/// PostgreSQL looks up relations and types in the session's temporary schema
/// first, unless the `search_path` lists `pg_temp` explicitly, later. The frame's
/// own path does, but a function the owner's code calls may set one that does
/// not, directly or through a function it runs by name at run time, which no check
/// of a definition sees. The temporary schema is the session's, so the writer's:
/// one of its relations or types taking another's name would stand in for it
/// while the owner's code runs. Functions and operators are never looked up there
/// implicitly.
///
/// A session that never made a temporary object has no temporary schema and
/// pays nothing. Once checked, the answer holds until a relation or type is
/// created, renamed or dropped, or the transaction ends.
fn check_temp_shadows(caller: Oid, owner: Oid) -> TViewResult<()> {
    let mut temp = pg_sys::InvalidOid;
    let mut toast = pg_sys::InvalidOid;
    // SAFETY: reads the backend's temporary namespace state.
    unsafe { pg_sys::GetTempNamespaceState(&raw mut temp, &raw mut toast) };
    if temp == pg_sys::InvalidOid || TEMP_CHECKED.with(std::cell::Cell::get) == Some(temp) {
        return Ok(());
    }
    // SAFETY: a role membership check by OID.
    if unsafe { pg_sys::has_privs_of_role(caller, owner) } {
        return Ok(());
    }
    let _path = CatalogPath::pin();
    let shadow = crate::utils::spi::catalog_rows(
        "SELECT 'relation', t.relname::pg_catalog.text, n.nspname::pg_catalog.text \
         FROM pg_catalog.pg_class t \
         JOIN pg_catalog.pg_class o ON o.relname = t.relname AND o.relnamespace <> t.relnamespace \
         JOIN pg_catalog.pg_namespace n ON n.oid = o.relnamespace \
         WHERE t.relnamespace = $1 AND t.relkind NOT IN ('i', 'I') \
           AND o.relkind NOT IN ('i', 'I', 't') AND n.nspname !~ '^pg_(temp|toast)' \
         UNION ALL \
         SELECT 'type', t.typname::pg_catalog.text, n.nspname::pg_catalog.text \
         FROM pg_catalog.pg_type t \
         JOIN pg_catalog.pg_type o ON o.typname = t.typname AND o.typnamespace <> t.typnamespace \
         JOIN pg_catalog.pg_namespace n ON n.oid = o.typnamespace \
         WHERE t.typnamespace = $1 AND n.nspname !~ '^pg_(temp|toast)' \
         LIMIT 1",
        &[crate::utils::spi::oid(temp)],
        |row| {
            Ok((
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<String>(3)?,
            ))
        },
    )?;
    if let Some((Some(kind), Some(name), Some(schema))) = shadow.into_iter().next() {
        return Err(TViewError::PermissionDenied {
            reason: format!(
                "a temporary {kind} named {name} shadows {schema}.{name} while a TVIEW's owner \
                 refreshes it: drop it or use another name"
            ),
        });
    }
    TEMP_CHECKED.with(|checked| checked.set(Some(temp)));
    Ok(())
}

/// Forget that the temporary schema was checked: a relation or type changed, or
/// the transaction ended.
pub fn forget_temp_check() {
    TEMP_CHECKED.with(|checked| checked.set(None));
}

#[pg_guard]
unsafe extern "C-unwind" fn names_changed(_arg: pg_sys::Datum, _cache: i32, _hash: u32) {
    forget_temp_check();
}

/// Watch the relation and type names for [`check_temp_shadows`]. Called once
/// from `_PG_init`.
pub fn register_name_callbacks() {
    // SAFETY: registers a static callback with no argument; valid in `_PG_init`.
    unsafe {
        for cache in [
            pg_sys::SysCacheIdentifier::RELNAMENSP,
            pg_sys::SysCacheIdentifier::TYPENAMENSP,
        ] {
            pg_sys::CacheRegisterSyscacheCallback(
                cache.cast_signed(),
                Some(names_changed),
                pg_sys::Datum::from(0),
            );
        }
    }
}

/// The token of an open owner frame, naming its owner. Only [`AsOwner::run`]
/// makes one, and only for the time `work` runs. A frame opened inside it makes
/// it unusable until that frame closes ([`OwnerFrame::require_current`]): no
/// TVIEW's SQL runs as the inner frame's role on the outer frame's token.
pub struct OwnerFrame {
    owner: Oid,
    saved_user: Oid,
    saved_context: i32,
    guc_level: i32,
    depth: usize,
}

impl OwnerFrame {
    fn enter(spec: Owner) -> Self {
        let owner = spec.role;
        let mut saved_user = pg_sys::InvalidOid;
        let mut saved_context = 0;
        // SAFETY: plain backend-global state, as REFRESH MATERIALIZED VIEW sets it;
        // the GUC nest level opened here is closed by Drop or by the abort.
        let guc_level = unsafe {
            pg_sys::GetUserIdAndSecContext(&raw mut saved_user, &raw mut saved_context);
            pg_sys::SetUserIdAndSecContext(
                owner,
                saved_context
                    | (pg_sys::SECURITY_LOCAL_USERID_CHANGE
                        | pg_sys::SECURITY_RESTRICTED_OPERATION)
                        .cast_signed(),
            );
            let level = pg_sys::NewGUCNestLevel();
            set_local(c"search_path", c"pg_catalog, pg_temp");
            pin_settings();
            if let Some(config) = spec.text_search {
                pin(c"default_text_search_config", config);
            }
            level
        };
        // The owner resolves no name the caller wrote.
        REPLACED_PATHS.with(|paths| paths.borrow_mut().push(None));
        let depth = OPEN_FRAMES.with(|open| {
            open.set(open.get() + 1);
            open.get()
        });
        if depth == 1 {
            CALLER.with(|caller| caller.set(saved_user));
        }
        Self {
            owner,
            saved_user,
            saved_context,
            guc_level,
            depth,
        }
    }

    /// Fail unless this is the innermost open frame and `table` (a TVIEW's
    /// table) is owned by its owner: SQL derived from that TVIEW runs as the
    /// TVIEW's owner, never as another frame's.
    ///
    /// # Errors
    /// An internal error on a mismatch, or if the relation does not exist.
    pub fn require_owner_of(&self, table: Oid) -> TViewResult<()> {
        self.require_current()?;
        check_frame_owner(self.owner, relation_owner(table)?, table)
    }

    /// Fail unless this is the innermost open frame and the current user is its
    /// owner.
    ///
    /// # Errors
    /// An internal error when another frame is open inside this one.
    pub fn require_current(&self) -> TViewResult<()> {
        let innermost = OPEN_FRAMES.with(std::cell::Cell::get) == self.depth;
        // SAFETY: reads backend-global state.
        let user = unsafe { pg_sys::GetUserId() };
        if innermost && user == self.owner {
            Ok(())
        } else {
            Err(frame_error(
                "an owner frame is used while another is open inside it",
            ))
        }
    }
}

impl crate::utils::spi::Frame for OwnerFrame {
    fn require_current(&self) -> TViewResult<()> {
        Self::require_current(self)
    }
}

/// `Ok` when a frame owned by `frame_owner` may run the SQL of a TVIEW whose
/// table `table` is owned by `table_owner`.
fn check_frame_owner(frame_owner: Oid, table_owner: Oid, table: Oid) -> TViewResult<()> {
    if frame_owner == table_owner {
        Ok(())
    } else {
        Err(frame_error(&format!(
            "the SQL of relation {} runs in a frame of role {}, not of its owner {}",
            table.to_u32(),
            frame_owner.to_u32(),
            table_owner.to_u32()
        )))
    }
}

fn frame_error(why: &str) -> TViewError {
    TViewError::CatalogError {
        operation: "Run a TVIEW's SQL as its owner".to_string(),
        pg_error: why.to_string(),
    }
}

impl Drop for OwnerFrame {
    fn drop(&mut self) {
        REPLACED_PATHS.with(|paths| paths.borrow_mut().pop());
        OPEN_FRAMES.with(|open| open.set(open.get() - 1));
        if std::thread::panicking() {
            return;
        }
        // SAFETY: undoes exactly what `enter` did, innermost first.
        unsafe {
            pg_sys::AtEOXact_GUC(false, self.guc_level);
            pg_sys::SetUserIdAndSecContext(self.saved_user, self.saved_context);
        }
    }
}

/// The settings that change how a value renders as text or how SQL parses, and
/// the values every refresh runs under: a TVIEW's rows then do not depend on
/// the session that wrote last, nor on the settings its role carries.
/// `CURRENT_DATE` in a refresh is the UTC day. Each, and why:
/// - `TimeZone`: timestamptz output and input without an offset.
/// - `DateStyle`: date and timestamp output, and the field order of input.
/// - `IntervalStyle`: interval output.
/// - `extra_float_digits`: float output precision.
/// - `bytea_output`: bytea output.
/// - `lc_monetary`: money output and input, `to_char`'s `L`.
/// - `lc_numeric`: `to_char`'s `D` and `G`.
/// - `lc_time`: `to_char`'s `TM` day and month names.
/// - `timezone_abbreviations`: what an abbreviation in timestamptz input means.
/// - `array_nulls`: whether `NULL` in array input is a null element.
/// - `quote_all_identifiers`: `quote_ident`, `format('%I')` and deparsed SQL.
/// - `transform_null_equals`: whether `x = NULL` parses as `x IS NULL`.
/// - `standard_conforming_strings`: what a backslash in a string literal means.
/// - `xmlbinary`: how bytea renders in XML.
/// - `xmloption`: whether text casts to XML as a document or as content.
/// - `gin_fuzzy_search_limit`: how many rows a GIN index scan returns.
///
/// `default_text_search_config` is a TVIEW's own: its creator's, pinned by the
/// frames of its [`Owner`].
pub const RENDER_SETTINGS: [(&CStr, &CStr); 16] = [
    (c"TimeZone", c"UTC"),
    (c"DateStyle", c"ISO, YMD"),
    (c"IntervalStyle", c"postgres"),
    (c"extra_float_digits", c"1"),
    (c"bytea_output", c"hex"),
    (c"lc_monetary", c"C"),
    (c"lc_numeric", c"C"),
    (c"lc_time", c"C"),
    (c"timezone_abbreviations", c"Default"),
    (c"array_nulls", c"on"),
    (c"quote_all_identifiers", c"off"),
    (c"transform_null_equals", c"off"),
    (c"standard_conforming_strings", c"on"),
    (c"xmlbinary", c"base64"),
    (c"xmloption", c"content"),
    (c"gin_fuzzy_search_limit", c"0"),
];

/// How the statements of a refresh are planned: as each says when it is kept
/// (`crate::utils::spi::Plan`). A session's `force_generic_plan` would plan a
/// lookup by an array parameter for ten keys (#235), and `force_custom_plan`
/// would plan every lookup again at each call. Never JIT-compiled (#283): a
/// refresh reads a few rows by key, or rebuilds a TVIEW once, and compiling its
/// statements cost more than it saved whenever a cost estimate crossed
/// `jit_above_cost`.
pub const PLAN_SETTINGS: [(&CStr, &CStr); 2] = [(c"plan_cache_mode", c"auto"), (c"jit", c"off")];

/// While alive, the [`RENDER_SETTINGS`] (and [`PLAN_SETTINGS`]) are in force; dropping it restores the
/// session's values (an error's (sub)transaction abort restores them instead).
/// For the work that computes TVIEW rows as the caller rather than as the
/// owner ([`AsOwner`] pins them too).
pub struct RenderPin {
    guc_level: i32,
}

impl RenderPin {
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: the GUC nest level opened here is closed by Drop or by the abort.
        let guc_level = unsafe {
            let level = pg_sys::NewGUCNestLevel();
            pin_settings();
            level
        };
        Self { guc_level }
    }
}

impl Default for RenderPin {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RenderPin {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        // SAFETY: closes the nest level `new` opened.
        unsafe { pg_sys::AtEOXact_GUC(false, self.guc_level) };
    }
}

/// While alive, the planner joins by hash or merge wherever it can
/// (`enable_nestloop = off`); dropping it restores the setting (an error's
/// (sub)transaction abort restores it instead).
///
/// For statements that join a whole TVIEW table with its whole view (a full
/// refresh). The table's statistics may say it is empty when it is not (emptied,
/// analyzed, refilled since): a nested loop chosen on that estimate evaluates the
/// view once per row. A key join always has a hash or merge form, and a nested
/// loop stays where nothing else can run (a lateral call).
pub struct WholeTableJoins {
    guc_level: i32,
}

impl WholeTableJoins {
    #[must_use]
    pub fn pin() -> Self {
        // SAFETY: the GUC nest level opened here is closed by Drop or by the abort.
        let guc_level = unsafe {
            let level = pg_sys::NewGUCNestLevel();
            set_local(c"enable_nestloop", c"off");
            level
        };
        Self { guc_level }
    }
}

impl Drop for WholeTableJoins {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        // SAFETY: closes the nest level `pin` opened.
        unsafe { pg_sys::AtEOXact_GUC(false, self.guc_level) };
    }
}

/// While alive, `search_path` is `pg_catalog, pg_temp`; dropping it restores the
/// session's (an error's (sub)transaction abort restores it instead).
///
/// For the work `pg_tviews` does on a caller's behalf: a function called, a
/// write, a flush, a utility statement. The catalog SQL it runs then resolves its
/// operators and functions in `pg_catalog`, whatever another role planted in a
/// schema on the caller's path, and the names it reads (`format_type`, `regproc`
/// output) come out qualified, so the SQL rendered from them, which an owner may
/// run, does not depend on that path either. The names the caller wrote are
/// resolved under its own path, in a [`CallerPath`].
pub struct CatalogPath {
    guc_level: i32,
}

impl CatalogPath {
    #[must_use]
    pub fn pin() -> Self {
        // Nested, the outermost pin's caller is still the caller.
        REPLACED_PATHS.with(|paths| {
            let mut paths = paths.borrow_mut();
            let replaced = match paths.last() {
                Some(outer) => outer.clone(),
                None => Some(search_path()),
            };
            paths.push(replaced);
        });
        // SAFETY: the GUC nest level opened here is closed by Drop or by the abort.
        let guc_level = unsafe {
            let level = pg_sys::NewGUCNestLevel();
            set_local(c"search_path", c"pg_catalog, pg_temp");
            level
        };
        Self { guc_level }
    }
}

impl Drop for CatalogPath {
    fn drop(&mut self) {
        REPLACED_PATHS.with(|paths| paths.borrow_mut().pop());
        if std::thread::panicking() {
            return;
        }
        // SAFETY: closes the nest level `pin` opened.
        unsafe { pg_sys::AtEOXact_GUC(false, self.guc_level) };
    }
}

/// Whether `pg_tviews` is doing work on a caller's behalf (a [`CatalogPath`] or
/// an [`AsOwner`] is open): the queries running now are its own.
pub fn in_extension_work() -> bool {
    REPLACED_PATHS.with(|paths| !paths.borrow().is_empty())
}

thread_local! {
    /// The `search_path` each open [`CatalogPath`] replaced: the caller's.
    /// `None` for an owner frame, under which nothing the caller wrote is resolved.
    static REPLACED_PATHS: RefCell<Vec<Option<CString>>> = const { RefCell::new(Vec::new()) };
    /// How many owner frames are open, the innermost last.
    static OPEN_FRAMES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The current user when the outermost open frame opened.
    static CALLER: std::cell::Cell<Oid> = const { std::cell::Cell::new(pg_sys::InvalidOid) };
    /// The text search configurations owners carry ([`Owner::with_text_search`]).
    static INTERNED: RefCell<Vec<&'static CStr>> = const { RefCell::new(Vec::new()) };
    /// The temporary schema found to shadow no name, until a name changes.
    static TEMP_CHECKED: std::cell::Cell<Option<Oid>> = const { std::cell::Cell::new(None) };
}

/// The session's `search_path`, as set.
fn search_path() -> CString {
    // SAFETY: a built-in setting, never missing; the value is copied before
    // another setting can change it.
    unsafe {
        let value = pg_sys::GetConfigOption(c"search_path".as_ptr(), false, false);
        if value.is_null() {
            CString::default()
        } else {
            CStr::from_ptr(value).to_owned()
        }
    }
}

/// While alive inside a [`CatalogPath`], `search_path` is the caller's again: for
/// resolving the names the caller wrote (a relation in its statement, a type, a
/// definition's text, the schema of an unqualified TVIEW) as it would. Only that
/// resolution belongs in it, never catalog SQL of `pg_tviews`' own. Outside a pin,
/// or under an [`AsOwner`], it changes nothing.
pub struct CallerPath {
    guc_level: Option<i32>,
}

impl CallerPath {
    #[must_use]
    pub fn enter() -> Self {
        let caller = REPLACED_PATHS.with(|paths| paths.borrow().last().cloned().flatten());
        // SAFETY: the GUC nest level opened here is closed by Drop or by the abort.
        let guc_level = caller.map(|path| unsafe {
            let level = pg_sys::NewGUCNestLevel();
            set_local(c"search_path", &path);
            level
        });
        Self { guc_level }
    }
}

impl Drop for CallerPath {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if let Some(level) = self.guc_level {
            // SAFETY: closes the nest level `enter` opened.
            unsafe { pg_sys::AtEOXact_GUC(false, level) };
        }
    }
}

/// Set `name` to `value` until the innermost GUC nest level closes.
///
/// SAFETY: a GUC nest level must be open.
unsafe fn set_local(name: &CStr, value: &CStr) {
    // SAFETY: NUL-terminated name and value; the caller opened the nest level
    // that GUC_ACTION_SAVE restores at.
    unsafe {
        pg_sys::set_config_option(
            name.as_ptr(),
            value.as_ptr(),
            pg_sys::GucContext::PGC_USERSET,
            pg_sys::GucSource::PGC_S_SESSION,
            pg_sys::GucAction::GUC_ACTION_SAVE,
            true,
            0,
            false,
        );
    }
}

/// SAFETY: a GUC nest level must be open.
unsafe fn pin_settings() {
    for (name, value) in RENDER_SETTINGS.into_iter().chain(PLAN_SETTINGS) {
        // SAFETY: the caller opened the nest level.
        unsafe { pin(name, value) };
    }
    // SAFETY: the caller opened the nest level.
    unsafe { planner::pin_all() };
}

/// The planner settings a refresh is planned under: the server's values, so a
/// writer's session (`SET enable_nestloop = off`) or role (`ALTER ROLE … SET`)
/// does not steer the plans of the owner's refresh, while a DBA's tuning stays in
/// force.
///
/// A setting's value is its reset value when that comes from the configuration
/// file, the command line, `ALTER SYSTEM`, `ALTER ROLE ALL … SET` or `ALTER
/// DATABASE … SET`. When a role's setting masked those (PostgreSQL loads only the
/// highest-priority stored setting), it is the database's value, then the one of
/// `ALTER ROLE ALL`, read from `pg_db_role_setting`; else the built-in default.
///
/// Each frame compares typed values read from the settings' records, found once
/// per backend: the reset value (a configuration reload changes it, and the pin
/// is derived again) and the current one, set only when it differs. A frame
/// whose settings already hold their pins costs a few loads per setting.
mod planner {
    use pgrx::pg_sys;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::{CStr, CString};

    const SETTINGS: [&CStr; 40] = [
        c"enable_async_append",
        c"enable_bitmapscan",
        c"enable_distinct_reordering",
        c"enable_gathermerge",
        c"enable_group_by_reordering",
        c"enable_hashagg",
        c"enable_hashjoin",
        c"enable_incremental_sort",
        c"enable_indexonlyscan",
        c"enable_indexscan",
        c"enable_material",
        c"enable_memoize",
        c"enable_mergejoin",
        c"enable_nestloop",
        c"enable_parallel_append",
        c"enable_parallel_hash",
        c"enable_partition_pruning",
        c"enable_partitionwise_aggregate",
        c"enable_partitionwise_join",
        c"enable_presorted_aggregate",
        c"enable_self_join_elimination",
        c"enable_seqscan",
        c"enable_sort",
        c"enable_tidscan",
        c"random_page_cost",
        c"seq_page_cost",
        c"cpu_tuple_cost",
        c"cpu_index_tuple_cost",
        c"cpu_operator_cost",
        c"effective_cache_size",
        c"work_mem",
        c"hash_mem_multiplier",
        c"join_collapse_limit",
        c"from_collapse_limit",
        c"geqo",
        c"geqo_threshold",
        c"cursor_tuple_fraction",
        c"constraint_exclusion",
        c"max_parallel_workers_per_gather",
        c"default_statistics_target",
    ];

    /// A setting's value as its record holds it.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Value {
        Bool(bool),
        Int(i32),
        Real(f64),
        Enum(i32),
    }

    struct Pin {
        name: &'static CStr,
        record: *mut pg_sys::config_generic,
        /// The reset source and value the pin was derived from.
        derived_from: (pg_sys::GucSource::Type, Value),
        value: Value,
        text: CString,
    }

    #[derive(Default)]
    struct Pins {
        found: bool,
        pins: Vec<Pin>,
        stored: Option<HashMap<String, String>>,
    }

    thread_local! {
        static PINS: RefCell<Pins> = RefCell::new(Pins::default());
    }

    /// Set every planner setting whose value differs from its pin.
    ///
    /// SAFETY: the caller opened the GUC nest level the settings are restored at.
    pub(super) unsafe fn pin_all() {
        PINS.with(|pins| {
            // A frame opened while the pins are derived (their catalog read) keeps
            // what is in force.
            let Ok(mut pins) = pins.try_borrow_mut() else {
                return;
            };
            if !pins.found {
                // SAFETY: built-in settings, looked up by name.
                pins.pins = SETTINGS
                    .into_iter()
                    .filter_map(|name| unsafe { record(name) }.map(|record| (name, record)))
                    .map(|(name, record)| Pin {
                        name,
                        record,
                        derived_from: (pg_sys::GucSource::PGC_S_DEFAULT, Value::Int(i32::MIN)),
                        value: Value::Int(i32::MIN),
                        text: CString::default(),
                    })
                    .collect();
                pins.found = true;
            }
            let Pins { pins, stored, .. } = &mut *pins;
            for pin in pins.iter_mut() {
                // SAFETY: a built-in record, which lives as long as the backend.
                let reset = unsafe { reset_of(pin.record) };
                if reset != pin.derived_from {
                    // SAFETY: as above.
                    unsafe { derive(pin, reset, stored) };
                }
                // SAFETY: as above.
                if unsafe { current_of(pin.record) } != pin.value {
                    // SAFETY: the caller opened the nest level.
                    unsafe { super::set_local(pin.name, &pin.text) };
                }
            }
        });
    }

    /// Derive `pin` from its record's reset source and value.
    ///
    /// SAFETY: `pin.record` is a live record.
    unsafe fn derive(
        pin: &mut Pin,
        reset: (pg_sys::GucSource::Type, Value),
        stored: &mut Option<HashMap<String, String>>,
    ) {
        pin.derived_from = reset;
        let from_text = |text: &str| {
            let text = CString::new(text).ok()?;
            // SAFETY: forwarded.
            unsafe { parse(pin.record, &text) }.map(|value| (value, text))
        };
        let derived = if reset.0 <= pg_sys::GucSource::PGC_S_DATABASE {
            // SAFETY: a setting that exists; the string is copied.
            let text = unsafe {
                let reset = pg_sys::GetConfigOptionResetString(pin.name.as_ptr());
                (!reset.is_null()).then(|| CStr::from_ptr(reset).to_owned())
            };
            text.map(|text| (reset.1, text))
        } else {
            let stored = stored.get_or_insert_with(stored_settings);
            stored
                .get(pin.name.to_str().unwrap_or_default())
                .and_then(|text| from_text(text))
        };
        // SAFETY: forwarded.
        let (value, text) = derived.unwrap_or_else(|| unsafe { boot_of(pin.record) });
        pin.value = value;
        pin.text = text;
    }

    /// SAFETY: a NUL-terminated name.
    unsafe fn record(name: &CStr) -> Option<*mut pg_sys::config_generic> {
        // SAFETY: forwarded; missing_ok, so a setting this server lacks is None.
        let record = unsafe {
            pg_sys::find_option(name.as_ptr(), false, true, pg_sys::DEBUG1.cast_signed())
        };
        (!record.is_null()).then_some(record)
    }

    /// SAFETY: a live record of a bool, int, real or enum setting.
    unsafe fn reset_of(record: *mut pg_sys::config_generic) -> (pg_sys::GucSource::Type, Value) {
        // SAFETY: the record's vartype says which struct it is.
        unsafe {
            let value = match (*record).vartype {
                pg_sys::config_type::PGC_BOOL => {
                    Value::Bool((*record.cast::<pg_sys::config_bool>()).reset_val)
                }
                pg_sys::config_type::PGC_INT => {
                    Value::Int((*record.cast::<pg_sys::config_int>()).reset_val)
                }
                pg_sys::config_type::PGC_REAL => {
                    Value::Real((*record.cast::<pg_sys::config_real>()).reset_val)
                }
                _ => Value::Enum((*record.cast::<pg_sys::config_enum>()).reset_val),
            };
            ((*record).reset_source, value)
        }
    }

    /// SAFETY: as [`reset_of`].
    unsafe fn current_of(record: *mut pg_sys::config_generic) -> Value {
        // SAFETY: as above; each record's variable is the setting's C variable.
        unsafe {
            match (*record).vartype {
                pg_sys::config_type::PGC_BOOL => {
                    Value::Bool(*(*record.cast::<pg_sys::config_bool>()).variable)
                }
                pg_sys::config_type::PGC_INT => {
                    Value::Int(*(*record.cast::<pg_sys::config_int>()).variable)
                }
                pg_sys::config_type::PGC_REAL => {
                    Value::Real(*(*record.cast::<pg_sys::config_real>()).variable)
                }
                _ => Value::Enum(*(*record.cast::<pg_sys::config_enum>()).variable),
            }
        }
    }

    /// The value `text` sets the setting to, as `SET` would read it; None when
    /// it does not parse.
    ///
    /// SAFETY: as [`reset_of`].
    unsafe fn parse(record: *mut pg_sys::config_generic, text: &CStr) -> Option<Value> {
        // SAFETY: as above; the parsers only read `text` and write the result.
        unsafe {
            let flags = (*record).flags;
            match (*record).vartype {
                pg_sys::config_type::PGC_BOOL => {
                    let mut on = false;
                    pg_sys::parse_bool(text.as_ptr(), &raw mut on).then_some(Value::Bool(on))
                }
                pg_sys::config_type::PGC_INT => {
                    let mut n = 0;
                    pg_sys::parse_int(text.as_ptr(), &raw mut n, flags, std::ptr::null_mut())
                        .then_some(Value::Int(n))
                }
                pg_sys::config_type::PGC_REAL => {
                    let mut x = 0.0;
                    pg_sys::parse_real(text.as_ptr(), &raw mut x, flags, std::ptr::null_mut())
                        .then_some(Value::Real(x))
                }
                _ => {
                    let mut n = 0;
                    pg_sys::config_enum_lookup_by_name(
                        record.cast::<pg_sys::config_enum>(),
                        text.as_ptr(),
                        &raw mut n,
                    )
                    .then_some(Value::Enum(n))
                }
            }
        }
    }

    /// The built-in default, as a value and as the text that sets it.
    ///
    /// SAFETY: as [`reset_of`].
    unsafe fn boot_of(record: *mut pg_sys::config_generic) -> (Value, CString) {
        // SAFETY: as above.
        let (value, text) = unsafe {
            match (*record).vartype {
                pg_sys::config_type::PGC_BOOL => {
                    let on = (*record.cast::<pg_sys::config_bool>()).boot_val;
                    (Value::Bool(on), (if on { "on" } else { "off" }).to_string())
                }
                pg_sys::config_type::PGC_INT => {
                    let n = (*record.cast::<pg_sys::config_int>()).boot_val;
                    (Value::Int(n), n.to_string())
                }
                pg_sys::config_type::PGC_REAL => {
                    let x = (*record.cast::<pg_sys::config_real>()).boot_val;
                    (Value::Real(x), x.to_string())
                }
                _ => {
                    let record = record.cast::<pg_sys::config_enum>();
                    let n = (*record).boot_val;
                    let name = pg_sys::config_enum_lookup_by_value(record, n);
                    let text = if name.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(name).to_string_lossy().into_owned()
                    };
                    (Value::Enum(n), text)
                }
            }
        };
        (value, CString::new(text).unwrap_or_default())
    }

    /// The values `ALTER DATABASE … SET` gave this database, over those `ALTER
    /// ROLE ALL … SET` gave every database: `name=value` entries of
    /// `pg_db_role_setting`. Empty when the catalog cannot be read.
    fn stored_settings() -> HashMap<String, String> {
        let rows = crate::utils::spi::catalog_rows(
            "SELECT s.setdatabase <> 0, c.entry \
               FROM pg_catalog.pg_db_role_setting s, pg_catalog.unnest(s.setconfig) AS c(entry) \
              WHERE s.setrole = 0 AND s.setdatabase IN (0, $1) \
              ORDER BY 1",
            // SAFETY: reads backend-global state.
            &[crate::utils::spi::oid(unsafe { pg_sys::MyDatabaseId })],
            |row| row.get::<String>(2),
        )
        .unwrap_or_default();
        // The database's entries come last and override the global ones.
        rows.into_iter()
            .flatten()
            .filter_map(|entry| {
                let (name, value) = entry.split_once('=')?;
                Some((name.to_string(), value.to_string()))
            })
            .collect()
    }
}

/// Set `name` to `value` at the open nest level, unless it has that value
/// already: setting some (`timezone_abbreviations` reads a file, the `lc_*`
/// settings call `setlocale`) costs more than comparing.
///
/// SAFETY: the caller opened the nest level.
unsafe fn pin(name: &CStr, value: &CStr) {
    // SAFETY: a NUL-terminated name of a setting that exists; the result, when
    // not null, is a NUL-terminated string valid until the setting changes.
    let current = unsafe { pg_sys::GetConfigOption(name.as_ptr(), false, false) };
    // SAFETY: as above.
    if current.is_null() || unsafe { CStr::from_ptr(current) } != value {
        // SAFETY: forwarded.
        unsafe { set_local(name, value) };
    }
}

/// Raise `insufficient_privilege` unless the current user has the privileges of
/// the owner of `table` (a TVIEW's `tv_*`) or of the extension's owner, as
/// `ALTER TABLE` would require.
///
/// # Errors
/// Returns an error if the catalog query fails.
pub fn require_owner(table: Oid, tview: &str) -> TViewResult<()> {
    let allowed = crate::utils::spi::connect(|client| {
        let args = [crate::utils::spi::oid(table)];
        client
            .select(
                "SELECT COALESCE((SELECT pg_catalog.pg_has_role(c.relowner, 'USAGE') \
                                  FROM pg_catalog.pg_class c WHERE c.oid = $1), false) \
                     OR pg_catalog.pg_has_role(e.extowner, 'USAGE') \
                 FROM pg_catalog.pg_extension e WHERE e.extname = 'pg_tviews'",
                None,
                &args,
            )?
            .first()
            .get_one::<bool>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check ownership of {tview}"),
        pg_error: e.to_string(),
    })?;
    if allowed != Some(true) {
        pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_INSUFFICIENT_PRIVILEGE,
            format!("must be owner of TVIEW {tview}"),
            function_name!(),
        )
        .report(PgLogLevel::ERROR);
    }
    Ok(())
}

/// Owner of the relation `table`, from the syscache: cheap enough for the row
/// trigger, and never stale after an `ALTER TABLE … OWNER TO`.
///
/// # Errors
/// Returns an error if the relation does not exist.
pub fn relation_owner(table: Oid) -> TViewResult<Oid> {
    // SAFETY: SearchSysCache1 returns a valid pg_class tuple or null; the tuple's
    // fixed part is a FormData_pg_class, and it is released before returning.
    let owner = unsafe {
        let tuple = pg_sys::SearchSysCache1(
            pg_sys::SysCacheIdentifier::RELOID.cast_signed(),
            pg_sys::Datum::from(table),
        );
        if tuple.is_null() {
            None
        } else {
            #[allow(clippy::cast_ptr_alignment)]
            // Reason: GETSTRUCT points at MAXALIGNed tuple data
            let form = pg_sys::GETSTRUCT(tuple).cast::<pg_sys::FormData_pg_class>();
            let owner = (*form).relowner;
            pg_sys::ReleaseSysCache(tuple);
            Some(owner)
        }
    };
    owner.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Look up the owner of relation {table:?}"),
        pg_error: "relation does not exist".to_string(),
    })
}
