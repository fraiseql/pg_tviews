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
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;
use std::cell::RefCell;
use std::ffi::{CStr, CString};

/// The role an owner frame runs as: a TVIEW's owner, named by its table or
/// entity, or the extension's owner, for writes to its catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner(Oid);

impl Owner {
    /// The owner of the relation `table`.
    ///
    /// # Errors
    /// Returns an error if the relation does not exist.
    pub fn of_table(table: Oid) -> TViewResult<Self> {
        relation_owner(table).map(Self)
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
        Ok(Self(owner))
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
        Ok(Self(unsafe { pg_sys::GetUserId() }))
    }

    /// The role's OID.
    #[must_use]
    pub const fn oid(self) -> Oid {
        self.0
    }

    /// The role `role`: one read with [`relation_owner`].
    #[must_use]
    pub const fn of_role(role: Oid) -> Self {
        Self(role)
    }
}

/// Runs work in an owner frame. SQL derived from a TVIEW, planned or run, runs
/// in its owner's frame only: the functions that plan or run it take the
/// frame's [`OwnerFrame`] token, which nothing else can make.
pub struct AsOwner;

impl AsOwner {
    /// Run `work` as `owner`: the current user is the owner, inside a
    /// security-restricted operation, with `search_path` set to
    /// `pg_catalog, pg_temp` and the [`RENDER_SETTINGS`] and [`PLAN_SETTINGS`]
    /// pinned. Leaving restores the caller's user, context and settings; on an
    /// error, the (sub)transaction abort restores them.
    ///
    /// # Errors
    /// What `work` returns.
    pub fn run<T>(
        owner: Owner,
        work: impl FnOnce(&mut OwnerFrame) -> TViewResult<T>,
    ) -> TViewResult<T> {
        let mut frame = OwnerFrame::enter(owner.0);
        work(&mut frame)
    }
}

/// Run `work` as the extension's owner, who owns the registration catalog: for a
/// catalog write by a caller whose right to change the TVIEW was checked.
///
/// # Errors
/// What `work` returns, or a failure to read the extension's owner.
pub fn as_extension_owner<T>(work: impl FnOnce() -> TViewResult<T>) -> TViewResult<T> {
    AsOwner::run(Owner::of_extension()?, |_| work())
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
    fn enter(owner: Oid) -> Self {
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
            level
        };
        // The owner resolves no name the caller wrote.
        REPLACED_PATHS.with(|paths| paths.borrow_mut().push(None));
        let depth = OPEN_FRAMES.with(|open| {
            open.set(open.get() + 1);
            open.get()
        });
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

/// The settings a value's text rendering depends on, and the values every
/// refresh renders under: a TVIEW's rows then do not depend on the
/// session that wrote last. `CURRENT_DATE` in a refresh is the UTC day.
pub const RENDER_SETTINGS: [(&CStr, &CStr); 5] = [
    (c"TimeZone", c"UTC"),
    (c"DateStyle", c"ISO, YMD"),
    (c"IntervalStyle", c"postgres"),
    (c"extra_float_digits", c"1"),
    (c"bytea_output", c"hex"),
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
