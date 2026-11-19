//! `pg_tviews` is the only writer of a TVIEW's table (D14): a statement trigger
//! on every `tv_*` table refuses an `INSERT`, `UPDATE`, `DELETE` or `TRUNCATE`
//! run outside an owner frame ([`crate::owner::AsOwner`]), with `55000`. The
//! owner's `TRUNCATE` is allowed: a `pg_tviews_refresh()` refills the table.
//!
//! The trigger is an ordinary one: a session in `session_replication_role =
//! replica`, and so a logical-replication apply worker, bypasses it, as it
//! bypasses every ordinary trigger. `pg_dump` creates it after the data.
//!
//! The same trigger checks `pg_tviews`' own statements on the table (D6): row
//! security that applies to the owner (`FORCE ROW LEVEL SECURITY`) would hide
//! rows from the refresh, which then leaves them stale or deletes a parent's
//! embedding. A statement of an owner frame is refused with `55000` unless a
//! policy lets the owner read and write every row ([`owner_filtered`]).

use crate::error::{TViewError, TViewResult};
use pgrx::prelude::*;

/// The guard's trigger function.
pub const GUARD_HANDLER: &str = "pg_tview_guard_trigger";
/// The guard's name on every TVIEW's table.
pub const GUARD_NAME: &str = "trg_tview_guard";

/// Refuse a write to a TVIEW's table from outside `pg_tviews`.
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires a Result return type
fn pg_tview_guard_trigger<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    let _path = crate::owner::CatalogPath::pin();
    let Ok(relation) = trigger.relation() else {
        TViewError::TriggerMisused {
            reason: "pg_tviews: the guard trigger has no relation".to_string(),
        }
        .raise();
    };
    if crate::owner::in_owner_frame() {
        // SAFETY: reads backend-global state: the frame's role, the owner.
        let owner = unsafe { pg_sys::GetUserId() };
        if let Some(why) = owner_filtered(relation.oid(), owner)
            .unwrap_or_else(|e| e.raise_in("pg_tviews: could not read the table's policies"))
        {
            let table = format!("{}.{}", relation.namespace(), relation.name());
            TViewError::WrongState { reason: filtered_message(&table, owner, &why) }.raise();
        }
        return Ok(None);
    }
    let event = match trigger.op() {
        Ok(PgTriggerOperation::Insert) => "INSERT",
        Ok(PgTriggerOperation::Update) => "UPDATE",
        Ok(PgTriggerOperation::Delete) => "DELETE",
        Ok(PgTriggerOperation::Truncate) => "TRUNCATE",
        Err(_) => TViewError::TriggerMisused {
            reason: "pg_tviews: the guard trigger fired on no event".to_string(),
        }
        .raise(),
    };
    // SAFETY: the open relation's catalog row.
    let owner = unsafe { (*relation.rd_rel).relowner };
    // SAFETY: a role membership check by OID.
    if event == "TRUNCATE" && unsafe { pg_sys::has_privs_of_role(pg_sys::GetUserId(), owner) } {
        return Ok(None);
    }
    pg_sys::panic::ErrorReport::new(
        PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
        format!(
            "pg_tviews: {}.{} is written by pg_tviews only: {event} from outside a refresh \
             would leave it, and every document embedding its rows, differing from its \
             definition",
            relation.namespace(),
            relation.name()
        ),
        function_name!(),
    )
    .set_hint(
        "Write the tables the TVIEW reads, or refresh it with tviews.pg_tviews_refresh() (its \
         owner may TRUNCATE it first). A data-only restore leaves out the TVIEW tables' data \
         (pg_dump --exclude-table-data): an empty TVIEW is refilled from its definition. Not \
         --disable-triggers, which also skips the rebind of pg_tviews' catalog.",
    )
    .report(PgLogLevel::ERROR);
    unreachable!("an ERROR report does not return")
}

/// Why row security can filter what `role` reads or writes of TVIEW table
/// `relid`, `None` when it cannot: row security does not apply to the role (not
/// enabled, or the role owns the table without `FORCE`, or holds
/// `BYPASSRLS`), or a permissive `FOR ALL` policy applying to it reads and
/// writes every row (`USING (true)`, `WITH CHECK` absent or `true`) and no
/// restrictive policy applies to it. One syscache lookup when row security does
/// not apply.
///
/// # Errors
/// The policies cannot be read.
pub fn owner_filtered(relid: pg_sys::Oid, role: pg_sys::Oid) -> TViewResult<Option<String>> {
    // SAFETY: a syscache lookup by OID; `noError` reports instead of raising.
    if unsafe { pg_sys::check_enable_rls(relid, role, true) }
        != pg_sys::CheckEnableRlsResult::RLS_ENABLED.cast_signed()
    {
        return Ok(None);
    }
    crate::utils::spi::one::<String>(
        "WITH applying AS ( \
             SELECT p.polname, p.polpermissive, p.polcmd, \
                    pg_catalog.pg_get_expr(p.polqual, p.polrelid) AS qual, \
                    pg_catalog.pg_get_expr(p.polwithcheck, p.polrelid) AS checks \
             FROM pg_catalog.pg_policy p \
             WHERE p.polrelid OPERATOR(pg_catalog.=) $1 \
               AND (0::pg_catalog.oid OPERATOR(pg_catalog.=) ANY (p.polroles) \
                    OR EXISTS (SELECT 1 FROM pg_catalog.unnest(p.polroles) r \
                               WHERE pg_catalog.pg_has_role($2, r, 'USAGE'))) \
         ) \
         SELECT CASE \
             WHEN EXISTS (SELECT 1 FROM applying WHERE NOT polpermissive) THEN \
                 pg_catalog.format('the restrictive policy %I applies to it', \
                     (SELECT pg_catalog.min(polname) FROM applying WHERE NOT polpermissive)) \
             WHEN NOT EXISTS (SELECT 1 FROM applying \
                              WHERE polpermissive AND polcmd OPERATOR(pg_catalog.=) '*' \
                                AND qual OPERATOR(pg_catalog.=) 'true' \
                                AND (checks IS NULL OR checks OPERATOR(pg_catalog.=) 'true')) THEN \
                 'no policy lets it read and write every row' \
             END",
        &[
            crate::utils::spi::oid(relid),
            crate::utils::spi::oid(role),
        ],
    )
}

/// Every TVIEW table whose owner row security can filter ([`owner_filtered`]):
/// the table, its owner and why.
///
/// # Errors
/// The catalog or the policies cannot be read.
pub fn filtered_tviews() -> TViewResult<Vec<(String, String, String)>> {
    let tables = crate::utils::spi::oids(
        &format!(
            "SELECT m.table_oid::pg_catalog.oid FROM {}.pg_tview_meta m \
             JOIN pg_catalog.pg_class c ON c.oid OPERATOR(pg_catalog.=) m.table_oid::pg_catalog.oid \
             WHERE c.relrowsecurity ORDER BY 1",
            crate::utils::ext_schema()
        ),
        &[],
    )?;
    let mut filtered = Vec::new();
    for table in tables {
        let owner = crate::owner::relation_owner(table)?;
        if let Some(why) = owner_filtered(table, owner)? {
            filtered.push((
                crate::utils::qualified_relname_from_oid(table)?,
                crate::owner::role_name(owner),
                why,
            ));
        }
    }
    Ok(filtered)
}

/// After a statement that may make row security apply to a role (`FORCE ROW
/// LEVEL SECURITY` among them): a WARNING for each TVIEW table whose
/// maintenance now fails ([`filtered_tviews`]).
///
/// # Errors
/// The catalog or the policies cannot be read.
pub fn warn_filtered() -> TViewResult<()> {
    for (table, owner, why) in filtered_tviews()? {
        pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_WARNING,
            format!(
                "pg_tviews: row security applies to {owner}, the owner of {table}, and {why}: \
                 every refresh of it fails until that changes"
            ),
            function_name!(),
        )
        .set_hint(format!(
            "Give the owner a policy that filters nothing (CREATE POLICY tviews_owner ON {table} \
             FOR ALL TO {owner} USING (true) WITH CHECK (true)), or ALTER TABLE {table} NO \
             FORCE ROW LEVEL SECURITY."
        ))
        .report(PgLogLevel::WARNING);
    }
    Ok(())
}

/// Refuse to refresh TVIEW table `table`, whose owner row security can filter
/// ([`owner_filtered`]): before a refresh reads it, since a filtered read
/// writes nothing the guard would see.
///
/// # Errors
/// [`TViewError::WrongState`] naming the table, its owner and the fix; an
/// error reading the catalog.
pub fn refuse_filtered_owner(table: pg_sys::Oid) -> TViewResult<()> {
    let owner = crate::owner::relation_owner(table)?;
    match owner_filtered(table, owner)? {
        None => Ok(()),
        Some(why) => Err(TViewError::WrongState {
            reason: filtered_message(&crate::utils::qualified_relname_from_oid(table)?, owner, &why),
        }),
    }
}

/// Why `pg_tviews` refuses to write `table` as `role` ([`owner_filtered`]'s
/// `why`), and the fix.
fn filtered_message(table: &str, role: pg_sys::Oid, why: &str) -> String {
    let owner = crate::owner::role_name(role);
    format!(
        "pg_tviews: row security applies to {owner}, the owner of {table}, and {why}: its \
         refresh would miss the rows a policy hides from it. Give the owner a policy that \
         filters nothing (CREATE POLICY tviews_owner ON {table} FOR ALL TO {owner} USING (true) \
         WITH CHECK (true)), or ALTER TABLE {table} NO FORCE ROW LEVEL SECURITY"
    )
}

/// Give TVIEW table `table_oid` its guard, unless it has one: at its creation
/// and at every re-registration.
///
/// # Errors
/// The table cannot be read, or the trigger cannot be created.
pub fn install(table_oid: pg_sys::Oid) -> TViewResult<()> {
    let present = !crate::utils::spi::oids(
        "SELECT t.oid FROM pg_catalog.pg_trigger t \
         JOIN pg_catalog.pg_proc p ON p.oid OPERATOR(pg_catalog.=) t.tgfoid \
         WHERE t.tgrelid OPERATOR(pg_catalog.=) $1 AND p.proname OPERATOR(pg_catalog.=) $2 \
           AND NOT t.tgisinternal",
        &[
            crate::utils::spi::oid(table_oid),
            crate::utils::spi::text(GUARD_HANDLER),
        ],
    )?
    .is_empty();
    if present {
        return Ok(());
    }
    let table = crate::utils::qualified_relname_from_oid(table_oid)?;
    let sql = format!(
        "CREATE TRIGGER {GUARD_NAME} BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON {table} \
         FOR EACH STATEMENT EXECUTE FUNCTION {}.{GUARD_HANDLER}()",
        crate::utils::ext_schema(),
    );
    // Trigger DDL as the table's owner, whoever re-registers it.
    crate::owner::AsOwner::run(crate::owner::Owner::of_table(table_oid)?, |_frame| {
        crate::utils::spi::run_ddl(&sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Install the guard trigger on {table}"),
            pg_error: e.to_string(),
        })
    })
}
