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

/// While alive, the current user is a TVIEW's owner. Dropping it restores the
/// previous user, security context and `search_path`. On an error, the
/// (sub)transaction abort restores them instead, so the guard leaves them alone
/// while unwinding.
pub struct AsOwner {
    saved_user: Oid,
    saved_context: i32,
    guc_level: i32,
}

impl AsOwner {
    /// Switch to the owner of `entity`'s `tv_*` table.
    ///
    /// # Errors
    /// Returns an error if the entity is not registered or its table is gone.
    pub fn of_entity(entity: &str) -> TViewResult<Self> {
        let meta = crate::catalog::TviewMeta::load_by_entity(entity)?.ok_or_else(|| {
            TViewError::TviewNotFound {
                name: entity.to_string(),
            }
        })?;
        Self::of_table(meta.tview_oid)
    }

    /// Switch to the owner of the relation `table`.
    ///
    /// # Errors
    /// Returns an error if the relation does not exist.
    pub fn of_table(table: Oid) -> TViewResult<Self> {
        Ok(Self::role(relation_owner(table)?))
    }

    /// Switch to the owner of the `pg_tviews` extension, to write its catalog.
    ///
    /// # Errors
    /// Returns an error if the extension's row cannot be read.
    pub fn of_extension() -> TViewResult<Self> {
        let owner = Spi::connect(|client| {
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
        Ok(Self::role(owner))
    }

    /// Switch to `owner`, a TVIEW's owner read with [`relation_owner`].
    #[must_use]
    pub fn of_role(owner: Oid) -> Self {
        Self::role(owner)
    }

    fn role(owner: Oid) -> Self {
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
        Self {
            saved_user,
            saved_context,
            guc_level,
        }
    }
}

impl Drop for AsOwner {
    fn drop(&mut self) {
        REPLACED_PATHS.with(|paths| paths.borrow_mut().pop());
        if std::thread::panicking() {
            return;
        }
        // SAFETY: undoes exactly what `of_table` did, innermost first.
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

/// While alive, the [`RENDER_SETTINGS`] are in force; dropping it restores the
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

thread_local! {
    /// The `search_path` each open [`CatalogPath`] replaced: the caller's.
    /// `None` for an [`AsOwner`], under which nothing the caller wrote is resolved.
    static REPLACED_PATHS: RefCell<Vec<Option<CString>>> = const { RefCell::new(Vec::new()) };
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
    for (name, value) in RENDER_SETTINGS {
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
    let allowed = Spi::connect(|client| {
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
