//! Base tables a TVIEW reads whose writes no cascade maps to its keys: the tables its lineage classifies `all_keys` (ADR 0157). They are
//! reported when the TVIEW is registered, and its `uncascaded_policy`, or the
//! policy declared for the table itself in `uncascaded_tables`, decides
//! what a write to one of them does: refused at create by default.

use crate::config::UncascadedPolicy;
use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;

/// A base table no cascade reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UncascadedTable {
    pub oid: Oid,
    /// Schema-qualified, quoted name.
    pub name: String,
    /// How the view reads it, when known.
    pub reason: String,
}

/// What a TVIEW declares about the reads no cascade reaches: its policy, the
/// tables with a policy of their own, and the tables the functions it
/// calls read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declarations {
    pub policy: UncascadedPolicy,
    /// Sorted by OID.
    pub tables: Vec<(Oid, UncascadedPolicy)>,
    /// `schema.name(argument types)` of each function, with the tables it reads;
    /// sorted.
    pub function_reads: Vec<(String, Vec<Oid>)>,
    /// How the TVIEW is brought up to date when it reads the current time.
    pub time_refresh: TimeRefresh,
}

/// How a TVIEW that reads the current time is brought up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeRefresh {
    /// Nothing declared: its policy refuses it, or warns.
    None,
    /// `pg_tviews_refresh_time_dependent()`, called at the boundary. `declared`:
    /// passed in this call's options, so a definition reading no time refuses
    /// it; from the setting or the catalog, it only applies to one that does.
    External { declared: bool },
}

impl TimeRefresh {
    /// The value the catalog stores.
    pub(crate) const fn stored(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::External { .. } => Some("external"),
        }
    }
}

impl Declarations {
    /// The settings' declarations: `pg_tviews.uncascaded_policy`, no table.
    pub(crate) fn from_settings() -> Self {
        Self::new(
            crate::config::uncascaded_policy(),
            Vec::new(),
            Vec::new(),
            match crate::config::time_refresh() {
                crate::config::TimeRefreshSetting::None => TimeRefresh::None,
                crate::config::TimeRefreshSetting::External => {
                    TimeRefresh::External { declared: false }
                }
            },
        )
    }

    pub(crate) fn new(
        policy: UncascadedPolicy,
        mut tables: Vec<(Oid, UncascadedPolicy)>,
        mut function_reads: Vec<(String, Vec<Oid>)>,
        time_refresh: TimeRefresh,
    ) -> Self {
        tables.sort_by_key(|(oid, _)| oid.to_u32());
        tables.dedup_by_key(|(oid, _)| *oid);
        for (_, read) in &mut function_reads {
            read.sort_by_key(|oid| oid.to_u32());
            read.dedup();
        }
        function_reads.sort_by(|(a, _), (b, _)| a.cmp(b));
        function_reads.dedup_by(|(a, _), (b, _)| a == b);
        Self {
            policy,
            tables,
            function_reads,
            time_refresh,
        }
    }

    /// As the catalog would store them: what this call's options declared is
    /// no longer told apart.
    pub(crate) fn stored(&self) -> Self {
        Self {
            time_refresh: match self.time_refresh {
                TimeRefresh::None => TimeRefresh::None,
                TimeRefresh::External { .. } => TimeRefresh::External { declared: false },
            },
            ..self.clone()
        }
    }

    /// The declarations stored with a TVIEW.
    pub(crate) fn of(meta: &crate::catalog::TviewMeta) -> Self {
        Self::new(
            meta.uncascaded_policy,
            meta.table_policies.clone(),
            meta.function_reads.clone(),
            if meta.time_refresh_external {
                TimeRefresh::External { declared: false }
            } else {
                TimeRefresh::None
            },
        )
    }

    /// `(function, table)` pairs, a function that reads no table paired with
    /// `Oid::INVALID` (bound as NULL), as `pg_tview_meta` stores them.
    pub(crate) fn function_read_pairs(&self) -> (Vec<String>, Vec<Oid>) {
        self.function_reads
            .iter()
            .flat_map(|(function, tables)| {
                let tables = if tables.is_empty() {
                    vec![Oid::INVALID]
                } else {
                    tables.clone()
                };
                tables.into_iter().map(move |t| (function.clone(), t))
            })
            .unzip()
    }

    /// The policy of writes to `table`.
    pub(crate) fn policy_for(&self, table: Oid) -> UncascadedPolicy {
        self.tables
            .iter()
            .find(|(oid, _)| *oid == table)
            .map_or(self.policy, |(_, policy)| *policy)
    }

    /// Store them for `entity`, as the extension's owner (the caller's right to
    /// change the TVIEW was checked).
    pub(crate) fn store(&self, entity: &str) -> TViewResult<()> {
        let _owner = crate::owner::AsOwner::of_extension()?;
        let oids: Vec<Oid> = self.tables.iter().map(|(oid, _)| *oid).collect();
        let policies: Vec<String> = self
            .tables
            .iter()
            .map(|(_, policy)| policy.as_str().to_string())
            .collect();
        let (functions, function_tables) = self.function_read_pairs();
        let args = [
            crate::utils::spi::text(entity),
            crate::utils::spi::text(self.policy.as_str()),
            crate::utils::spi::oid_array(oids),
            crate::utils::spi::text_array(policies),
            crate::utils::spi::text_array(functions),
            crate::utils::spi::oid_array(function_tables),
            crate::utils::spi::text(self.time_refresh.stored()),
        ];
        Spi::run_with_args(
            &format!(
                "UPDATE {} SET uncascaded_policy = $2, \
                     uncascaded_table_oids = $3::pg_catalog.oid[]::pg_catalog.regclass[], \
                     uncascaded_table_policies = $4, function_read_functions = $5, \
                     function_read_tables = $6::pg_catalog.oid[]::pg_catalog.regclass[], \
                     time_refresh = $7 \
                 WHERE entity = $1",
                crate::utils::meta_table()
            ),
            &args,
        )
        .map_err(|e| TViewError::CatalogError {
            operation: "Store the uncascaded policies".to_string(),
            pg_error: e.to_string(),
        })
    }
}

/// How the trigger and the flush learn what a TVIEW reached: the tables and the
/// stored declarations.
#[derive(Debug, Clone)]
pub(crate) struct Uncascaded {
    pub tables: Vec<UncascadedTable>,
    pub declarations: Declarations,
    /// The definition reads the current time.
    pub time_dependent: bool,
}

impl Uncascaded {
    /// The `time_refresh` the catalog stores: only for a TVIEW that reads the
    /// time.
    pub(crate) fn time_refresh(&self) -> Option<&'static str> {
        if self.time_dependent {
            self.declarations.time_refresh.stored()
        } else {
            None
        }
    }
}

impl Uncascaded {
    pub(crate) fn oids(&self) -> Vec<Oid> {
        self.tables.iter().map(|t| t.oid).collect()
    }
}

/// `writes to a, b will not refresh tv (a: reason; b: reason)`.
fn describe(tview: &str, tables: &[&UncascadedTable], verb: &str) -> String {
    let names = tables
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let reasons = match tables {
        [one] => one.reason.clone(),
        _ => tables
            .iter()
            .map(|t| format!("{}: {}", t.name, t.reason))
            .collect::<Vec<_>>()
            .join("; "),
    };
    format!("writes to {names} {verb} {tview} ({reasons})")
}

/// What to write to declare the policy of `tables` of `tview`: the option of
/// `pg_tviews_create_or_replace()`, for those tables or the whole TVIEW, or the
/// setting `CREATE TABLE … AS` and `pg_tviews_create()` read.
fn how_to_declare(tview: &str, tables: &[&UncascadedTable], policy: &str) -> String {
    let json = |text: &str| serde_json::Value::from(text).to_string();
    let named = tables
        .iter()
        .map(|t| format!("{}: {}", json(&t.name), json(policy)))
        .collect::<Vec<_>>()
        .join(", ");
    // The option as JSON, written as an SQL literal.
    let option = crate::utils::quote_literal(&format!("{{\"uncascaded_tables\": {{{named}}}}}"));
    format!(
        "pg_tviews_create_or_replace('{tview}', <definition>, options => \
         {option}), or for the whole TVIEW \
         '{{\"uncascaded_policy\": \"{policy}\"}}'; before CREATE TABLE … AS or \
         pg_tviews_create(): SET pg_tviews.uncascaded_policy = '{policy}'"
    )
}

/// Report the uncascaded tables of `tview` (schema-qualified), each under its
/// policy: an ERROR that aborts the create and says what to declare instead, a
/// WARNING, or a NOTICE.
///
/// # Errors
/// Never returns one: under the `error` policy the ERROR is raised here.
pub(crate) fn report(tview: &str, uncascaded: &Uncascaded) -> TViewResult<()> {
    let under = |policy: UncascadedPolicy| -> Vec<&UncascadedTable> {
        uncascaded
            .tables
            .iter()
            .filter(|t| uncascaded.declarations.policy_for(t.oid) == policy)
            .collect()
    };
    let refused = under(UncascadedPolicy::Error);
    if !refused.is_empty() {
        pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
            format!(
                "{}: declare what such a write does with the TVIEW's uncascaded_policy",
                describe(tview, &refused, "would not refresh")
            ),
            function_name!(),
        )
        .set_hint(format!(
            "To refresh {tview} in full on such writes: {}. \"warn\" accepts stale \
             rows instead. Or join the tables on a column pg_tviews can trace.",
            how_to_declare(tview, &refused, "full_refresh")
        ))
        .report(PgLogLevel::ERROR);
    }
    let warned = under(UncascadedPolicy::Warn);
    if !warned.is_empty() {
        pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_WARNING,
            describe(tview, &warned, "will not refresh"),
            function_name!(),
        )
        .set_hint(format!(
            "To refresh it in full on such writes instead: {}.",
            how_to_declare(tview, &warned, "full_refresh")
        ))
        .report(PgLogLevel::WARNING);
    }
    let refreshed = under(UncascadedPolicy::FullRefresh);
    if !refreshed.is_empty() {
        notice!(
            "{}",
            describe(tview, &refreshed, "will refresh all rows of")
        );
    }
    Ok(())
}

/// Add the tables the declared functions read to `lineage`, and return
/// them with the functions the definition calls that are not declared.
///
/// # Errors
/// Returns an error naming a declared function the definition does not call.
pub(crate) fn apply_function_reads(
    entity: &str,
    declarations: &Declarations,
    lineage: &mut crate::lineage::Lineage,
) -> TViewResult<(Vec<Oid>, Vec<String>)> {
    let mut reads = Vec::new();
    for (function, tables) in &declarations.function_reads {
        if !lineage.functions.iter().any(|(_, f)| f == function) {
            return Err(TViewError::InvalidInput {
                parameter: "function_reads".to_string(),
                reason: format!(
                    "{function} is declared in function_reads, but tv_{entity} does not call it: \
                     remove it from the list"
                ),
            });
        }
        for &table in tables {
            reads.push(function_read(function, table)?);
        }
    }
    lineage.add_function_reads(&reads);
    let tables = reads
        .iter()
        .filter(|r| r.tview.is_none())
        .map(|r| Oid::from(r.relid))
        .collect();
    let undeclared = lineage
        .functions
        .iter()
        .filter(|(_, f)| !declarations.function_reads.iter().any(|(d, _)| d == f))
        .map(|(_, f)| f.clone())
        .collect();
    Ok((tables, undeclared))
}

/// Table `table`, read inside `function`, as the lineage records it.
fn function_read(function: &str, table: Oid) -> TViewResult<crate::lineage::FunctionRead> {
    let (relname, relkind, tview) = Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT c.relname::text, c.relkind::text, \
                            (SELECT m.entity::text FROM {} m WHERE m.table_oid::pg_catalog.oid = c.oid) \
                     FROM pg_catalog.pg_class c WHERE c.oid = $1",
                    crate::utils::meta_table()
                ),
                None,
                &[crate::utils::spi::oid(table)],
            )?
            .first()
            .get_three::<String, String, String>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Read a table a function reads".to_string(),
        pg_error: e.to_string(),
    })?;
    Ok(crate::lineage::FunctionRead {
        function: function.to_string(),
        relid: table.to_u32(),
        relname: relname.unwrap_or_default(),
        qualified: crate::utils::qualified_relname_from_oid(table)?,
        matview: relkind.as_deref() == Some("m"),
        tview,
    })
}

/// Report the functions `tview` calls that may read tables and are not declared:
/// nothing would refresh it when those tables change, so they are refused
/// under `error` and `full_refresh`, and warned about under `warn`.
///
/// # Errors
/// Never returns one: a refusal is raised here.
pub(crate) fn report_functions(
    tview: &str,
    functions: &[String],
    policy: UncascadedPolicy,
) -> TViewResult<()> {
    let Some(first) = functions.first() else {
        return Ok(());
    };
    let names = functions.join(", ");
    let (calls, it) = if functions.len() == 1 {
        ("calls", "it reads")
    } else {
        ("calls functions", "they read")
    };
    let message = format!(
        "{tview} {calls} {names}, not immutable: the tables {it} are invisible to pg_tviews, \
         and writes to them would not refresh {tview}: declare them in function_reads"
    );
    let hint = format!(
        "Declare them: pg_tviews_create_or_replace('{tview}', <definition>, options => \
         {}), [] for a function that reads no table; then give those tables a policy in \
         uncascaded_tables. Or make the function IMMUTABLE if it reads nothing that changes.",
        crate::utils::quote_literal(&format!(
            "{{\"function_reads\": {{{}: [\"<schema.table>\", …]}}}}",
            serde_json::Value::from(first.as_str())
        ))
    );
    let level = if policy == UncascadedPolicy::Warn {
        PgLogLevel::WARNING
    } else {
        PgLogLevel::ERROR
    };
    let code = if policy == UncascadedPolicy::Warn {
        PgSqlErrorCode::ERRCODE_WARNING
    } else {
        PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE
    };
    pg_sys::panic::ErrorReport::new(code, message, function_name!())
        .set_hint(hint)
        .report(level);
    Ok(())
}

/// Report how `tview` reads the current time: its rows change at a
/// boundary no write marks. Refused under `error` and `full_refresh` unless it
/// declares `time_refresh`, warned about under `warn`; a declared `time_refresh`
/// for a definition that reads no time is refused.
///
/// # Errors
/// Returns an error for a declaration that does not apply; a refusal of the
/// definition is raised here.
pub(crate) fn report_time(
    tview: &str,
    time_reads: &[String],
    declarations: &Declarations,
) -> TViewResult<()> {
    match (time_reads.is_empty(), declarations.time_refresh) {
        (true, TimeRefresh::External { declared: true }) => Err(TViewError::InvalidInput {
            parameter: "time_refresh".to_string(),
            reason: format!(
                "{tview} declares time_refresh, but its definition reads no time: remove it"
            ),
        }),
        (true, _) | (false, TimeRefresh::External { .. }) => Ok(()),
        (false, TimeRefresh::None) => {
            let warn = declarations.policy == UncascadedPolicy::Warn;
            pg_sys::panic::ErrorReport::new(
                if warn {
                    PgSqlErrorCode::ERRCODE_WARNING
                } else {
                    PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE
                },
                format!(
                    "{tview} reads the time ({}): its rows change with no write, which nothing \
                     refreshes: declare time_refresh",
                    time_reads.join(", ")
                ),
                function_name!(),
            )
            .set_hint(format!(
                "Declare who brings it up to date: pg_tviews_create_or_replace('{tview}', \
                 <definition>, options => '{{\"time_refresh\": \"external\"}}'), or before \
                 CREATE TABLE … AS / pg_tviews_create(): SET pg_tviews.time_refresh = \
                 'external'; then call tviews.pg_tviews_refresh_time_dependent() at the \
                 boundary (pg_cron, the application). Or pass the date as data instead."
            ))
            .report(if warn {
                PgLogLevel::WARNING
            } else {
                PgLogLevel::ERROR
            });
            Ok(())
        }
    }
}

/// Refuse a table declared in `uncascaded_tables` that `lineage` does not read,
/// or whose writes it traces: the declaration would never apply.
///
/// # Errors
/// Returns an error naming the first such table.
pub(crate) fn check_declared(
    tview: &str,
    declarations: &Declarations,
    lineage: &crate::lineage::Lineage,
) -> TViewResult<()> {
    for (oid, _) in &declarations.tables {
        let name = crate::utils::qualified_relname_from_oid(*oid)?;
        let reason = match lineage.tables.iter().find(|t| t.relid == oid.to_u32()) {
            None => format!("{tview} does not read it"),
            Some(t) if !matches!(t.kind, crate::lineage::TableKind::AllKeys(_)) => format!(
                "its writes to {tview} are traced ({}): no policy applies to them",
                t.kind.name()
            ),
            Some(_) => continue,
        };
        return Err(TViewError::InvalidInput {
            parameter: "uncascaded_tables".to_string(),
            reason: format!(
                "{name} is declared in uncascaded_tables, but {reason}: remove it from the list"
            ),
        });
    }
    Ok(())
}

/// After `REFRESH MATERIALIZED VIEW matview`: refresh in full every TVIEW that
/// reads it under the `full_refresh` policy (its own or the TVIEW's), then flush the queue, as the flush
/// trigger does after a write. Under `warn` the TVIEW was created knowing
/// it would go stale; under `error` it was never created.
///
/// # Errors
/// Returns an error if the catalog cannot be read or a refresh fails.
pub(crate) fn refresh_readers_of(matview: Oid) -> TViewResult<()> {
    let args = [crate::utils::spi::oid(matview)];
    let entities: Vec<String> = Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT entity::text FROM {} \
                     WHERE $1::pg_catalog.regclass = ANY (uncascaded_oids) \
                       AND COALESCE(uncascaded_table_policies[pg_catalog.array_position( \
                               uncascaded_table_oids, $1::pg_catalog.regclass)], \
                           uncascaded_policy) = 'full_refresh' \
                     ORDER BY entity",
                    crate::utils::meta_table()
                ),
                None,
                &args,
            )?
            .map(|row| row.get::<String>(1))
            .collect::<Result<Vec<_>, _>>()
            .map(|entities| entities.into_iter().flatten().collect())
    })
    .map_err(|e| crate::TViewError::CatalogError {
        operation: "Find the TVIEWs reading a refreshed materialized view".to_string(),
        pg_error: e.to_string(),
    })?;
    if entities.is_empty() {
        return Ok(());
    }
    for entity in &entities {
        if crate::config::suspend_triggers() || crate::suspend::is_suspended() {
            crate::suspend::record_change(entity);
        } else {
            crate::queue::enqueue_refresh_all(entity);
        }
    }
    crate::flush::flush_refresh_queue()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_lists_tables_and_reasons() {
        let t = |n: &str, r: &str| UncascadedTable {
            oid: Oid::INVALID,
            name: n.to_string(),
            reason: r.to_string(),
        };
        let (a, b) = (t("public.a", "x"), t("public.b", "y"));
        assert_eq!(
            describe("public.tv_o", &[&a], "will not refresh"),
            "writes to public.a will not refresh public.tv_o (x)"
        );
        assert_eq!(
            describe("public.tv_o", &[&a, &b], "will not refresh"),
            "writes to public.a, public.b will not refresh public.tv_o (public.a: x; public.b: y)"
        );
    }

    #[test]
    fn a_declared_table_overrides_the_policy() {
        let (a, b) = (Oid::from(10_u32), Oid::from(20_u32));
        let d = Declarations::new(
            UncascadedPolicy::Error,
            vec![
                (b, UncascadedPolicy::FullRefresh),
                (b, UncascadedPolicy::Warn),
            ],
            Vec::new(),
            TimeRefresh::None,
        );
        assert_eq!(d.tables, vec![(b, UncascadedPolicy::FullRefresh)]);
        assert_eq!(d.policy_for(a), UncascadedPolicy::Error);
        assert_eq!(d.policy_for(b), UncascadedPolicy::FullRefresh);
    }

    #[test]
    fn function_reads_are_stored_as_pairs() {
        let (a, b) = (Oid::from(10_u32), Oid::from(20_u32));
        let d = Declarations::new(
            UncascadedPolicy::Error,
            Vec::new(),
            vec![
                ("public.tag()".to_string(), Vec::new()),
                ("public.label()".to_string(), vec![b, a, b]),
            ],
            TimeRefresh::None,
        );
        assert_eq!(
            d.function_reads,
            vec![
                ("public.label()".to_string(), vec![a, b]),
                ("public.tag()".to_string(), Vec::new()),
            ]
        );
        let (functions, tables) = d.function_read_pairs();
        assert_eq!(
            functions,
            ["public.label()", "public.label()", "public.tag()"]
        );
        assert_eq!(tables, [a, b, Oid::INVALID]);
    }

    #[test]
    fn the_hint_names_the_tables() {
        let a = UncascadedTable {
            oid: Oid::INVALID,
            name: "public.tb_locale".to_string(),
            reason: String::new(),
        };
        assert!(
            how_to_declare("public.tv_x", &[&a], "full_refresh")
                .contains(r#"'{"uncascaded_tables": {"public.tb_locale": "full_refresh"}}'"#)
        );
    }
}
