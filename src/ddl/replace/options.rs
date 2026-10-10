//! The `options` of `pg_tviews_create_or_replace()`, and the names they hold.

use crate::config::UncascadedPolicy;
use crate::ddl::aggregate::GroupKeys;
use crate::ddl::create::Storage;
use crate::ddl::uncascaded::{Declarations, TimeRefresh};
use crate::error::{TViewError, TViewResult};
use pgrx::prelude::*;

/// The `options` of `pg_tviews_create_or_replace()` and `pg_tviews_create()`, as
/// passed. `None`: not passed, so the default (ADR 0220).
#[derive(Debug, Default)]
pub(crate) struct Options {
    pub(super) logged: Option<bool>,
    pub(super) fillfactor: Option<i32>,
    pub(super) data_gin_index: Option<bool>,
    /// An aggregate TVIEW's group keys.
    pub(super) group_keys: Option<GroupKeys>,
    /// What a write to a table no cascade reaches does.
    pub(super) uncascaded_policy: Option<UncascadedPolicy>,
    /// Tables with a policy of their own, as named: resolved when used.
    pub(super) uncascaded_tables: Option<Vec<(String, UncascadedPolicy)>>,
    /// Functions the definition calls, each with the tables it reads, as
    /// named: resolved when used.
    pub(super) function_reads: Option<Vec<(String, Vec<String>)>>,
    /// How the TVIEW is brought up to date when it reads the current time.
    pub(super) time_refresh: Option<TimeRefresh>,
    /// The GraphQL type name `pg_tviews_flush_and_report()` reports.
    pub(super) typename: Option<String>,
}

/// A TVIEW as its options declare it: every option, those not passed at their
/// defaults (ADR 0220).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declared {
    pub storage: Storage,
    pub group_keys: Option<GroupKeys>,
    pub declarations: Declarations,
    /// `None`: `PascalCase(entity)`.
    pub typename: Option<String>,
}

impl Options {
    /// Options of a `CREATE [UNLOGGED] TABLE tv_* [WITH (fillfactor = n)] AS`.
    pub(crate) fn storage(logged: Option<bool>, fillfactor: Option<i32>) -> Self {
        Self {
            logged,
            fillfactor,
            ..Self::default()
        }
    }

    /// Options of `pg_tviews_create_aggregate()`.
    pub(crate) fn aggregate(group_keys: GroupKeys) -> Self {
        Self {
            group_keys: Some(group_keys),
            ..Self::default()
        }
    }

    /// What these options declare, every option not passed at its default.
    ///
    /// # Errors
    /// Returns an error naming a table or function that does not resolve.
    pub(crate) fn resolve(self) -> TViewResult<Declared> {
        let defaults = Storage::DEFAULT;
        Ok(Declared {
            storage: Storage {
                logged: self.logged.unwrap_or(defaults.logged),
                fillfactor: self.fillfactor.unwrap_or(defaults.fillfactor),
                data_gin_index: self.data_gin_index.unwrap_or(defaults.data_gin_index),
            },
            group_keys: self.group_keys,
            declarations: Declarations::new(
                self.uncascaded_policy.unwrap_or_default(),
                match &self.uncascaded_tables {
                    Some(declared) => resolve_tables(declared)?,
                    None => Vec::new(),
                },
                match &self.function_reads {
                    Some(declared) => resolve_function_reads(declared)?,
                    None => Vec::new(),
                },
                self.time_refresh.unwrap_or(TimeRefresh::None),
            ),
            typename: self.typename,
        })
    }
}

pub(super) fn invalid(parameter: &str, reason: impl Into<String>) -> TViewError {
    TViewError::InvalidInput {
        parameter: parameter.to_string(),
        reason: reason.into(),
    }
}

/// Parse the `options` object: an unknown key or a value of the wrong type is an
/// error.
///
/// # Errors
/// Returns an error naming the offending key.
pub(crate) fn parse_options(value: &serde_json::Value) -> TViewResult<Options> {
    let serde_json::Value::Object(map) = value else {
        return Err(invalid("options", "must be a JSON object"));
    };
    let mut options = Options::default();
    for (key, value) in map {
        match key.as_str() {
            "logged" => options.logged = Some(boolean(key, value)?),
            "data_gin_index" => options.data_gin_index = Some(boolean(key, value)?),
            "fillfactor" => options.fillfactor = Some(fillfactor(key, value)?),
            "uncascaded_policy" => options.uncascaded_policy = Some(policy(key, value, "")?),
            "uncascaded_tables" => options.uncascaded_tables = Some(uncascaded_tables(key, value)?),
            "function_reads" => options.function_reads = Some(function_reads(key, value)?),
            "time_refresh" => options.time_refresh = Some(time_refresh(key, value)?),
            "group_keys" => options.group_keys = group_keys(key, value)?,
            "typename" => options.typename = typename(key, value)?,
            other => {
                return Err(invalid(
                    "options",
                    format!(
                        "unknown option \"{other}\" (known: logged, fillfactor, \
                         data_gin_index, group_keys, uncascaded_policy, uncascaded_tables, \
                         function_reads, time_refresh, typename)"
                    ),
                ));
            }
        }
    }
    Ok(options)
}

fn boolean(key: &str, value: &serde_json::Value) -> TViewResult<bool> {
    value
        .as_bool()
        .ok_or_else(|| invalid(key, "must be a boolean"))
}

fn fillfactor(key: &str, value: &serde_json::Value) -> TViewResult<i32> {
    let fillfactor = value
        .as_i64()
        .ok_or_else(|| invalid(key, "must be an integer"))?;
    if !(10..=100).contains(&fillfactor) {
        return Err(invalid(key, "must be between 10 and 100"));
    }
    i32::try_from(fillfactor).map_err(|_| invalid(key, "must be between 10 and 100"))
}

/// A policy name; `of` names what it applies to in the error ("" for the option).
fn policy(key: &str, value: &serde_json::Value, of: &str) -> TViewResult<UncascadedPolicy> {
    value
        .as_str()
        .and_then(UncascadedPolicy::parse)
        .ok_or_else(|| invalid(key, format!("{of}{POLICIES}")))
}

fn uncascaded_tables(
    key: &str,
    value: &serde_json::Value,
) -> TViewResult<Vec<(String, UncascadedPolicy)>> {
    let serde_json::Value::Object(tables) = value else {
        return Err(invalid(
            key,
            "must be an object mapping each table to its policy, e.g. \
             {\"public.tb_locale\": \"full_refresh\"}",
        ));
    };
    tables
        .iter()
        .map(|(table, p)| {
            Ok((
                table.clone(),
                policy(key, p, &format!("the policy of {table} "))?,
            ))
        })
        .collect()
}

fn function_reads(key: &str, value: &serde_json::Value) -> TViewResult<Vec<(String, Vec<String>)>> {
    let shape = || {
        invalid(
            key,
            "must be an object mapping each function, with its argument types, \
             to the tables it reads, e.g. \
             {\"public.label_suffix()\": [\"public.tb_setting\"]}",
        )
    };
    let serde_json::Value::Object(functions) = value else {
        return Err(shape());
    };
    functions
        .iter()
        .map(|(function, tables)| {
            let tables = tables
                .as_array()
                .ok_or_else(shape)?
                .iter()
                .map(|t| t.as_str().map(str::to_string).ok_or_else(shape))
                .collect::<TViewResult<Vec<_>>>()?;
            Ok((function.clone(), tables))
        })
        .collect()
}

fn time_refresh(key: &str, value: &serde_json::Value) -> TViewResult<TimeRefresh> {
    match value {
        serde_json::Value::Null => Ok(TimeRefresh::None),
        serde_json::Value::String(s) if s == "external" => {
            Ok(TimeRefresh::External { declared: true })
        }
        _ => Err(invalid(
            key,
            "must be \"external\" (pg_tviews_refresh_time_dependent() is \
             called at the boundary) or null",
        )),
    }
}

fn group_keys(key: &str, value: &serde_json::Value) -> TViewResult<Option<GroupKeys>> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(keys) if !keys.is_empty() => Ok(Some(
            serde_json::from_value::<GroupKeys>(value.clone())
                .map_err(|_| invalid(key, "must map source table names to column names"))?,
        )),
        _ => Err(invalid(
            key,
            "must be null or an object mapping each source table to its \
             group key column, e.g. {\"tb_order\": \"fk_user\"}",
        )),
    }
}

fn typename(key: &str, value: &serde_json::Value) -> TViewResult<Option<String>> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(name) if is_graphql_name(name) => Ok(Some(name.clone())),
        _ => Err(invalid(
            key,
            "must be null (PascalCase of the entity) or a GraphQL name ([_A-Za-z][_0-9A-Za-z]*)",
        )),
    }
}

/// Whether `name` is a GraphQL name: `[_A-Za-z][_0-9A-Za-z]*`.
pub(crate) fn is_graphql_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) const POLICIES: &str = "must be \"error\", \"full_refresh\" or \"warn\"";

/// The tables of the `uncascaded_tables` option, by OID.
///
/// # Errors
/// Returns an error naming a table that does not exist or is not a table.
pub(super) fn resolve_tables(
    declared: &[(String, UncascadedPolicy)],
) -> TViewResult<Vec<(pg_sys::Oid, UncascadedPolicy)>> {
    declared
        .iter()
        .map(|(name, policy)| Ok((resolve_table(name, "uncascaded_tables")?, *policy)))
        .collect()
}

/// The table an option names, by OID.
///
/// # Errors
/// Returns an error if it does not exist or is not a table.
pub(super) fn resolve_table(name: &str, option: &str) -> TViewResult<pg_sys::Oid> {
    let found = Spi::connect(|client| {
        client
            .select(
                "SELECT c.oid, c.relkind IN ('r', 'p', 'm', 'f') \
                 FROM (SELECT pg_catalog.to_regclass($1) AS relation) r \
                 LEFT JOIN pg_catalog.pg_class c ON c.oid = r.relation",
                None,
                &[crate::utils::spi::text(name)],
            )?
            .first()
            .get_two::<pg_sys::Oid, bool>()
    })
    .map_err(|e| crate::utils::spi::catalog_error(&format!("Look up a table of {option}"), &e))?;
    match found {
        (Some(oid), Some(true)) => Ok(oid),
        (Some(_), _) => Err(invalid(
            option,
            format!("{name} is not a table: name the tables a view reads"),
        )),
        _ => Err(invalid(option, format!("relation {name} does not exist"))),
    }
}

/// The functions of the `function_reads` option, as `schema.name(argument
/// types)`, with their tables by OID.
///
/// # Errors
/// Returns an error naming a function or table that does not exist.
pub(super) fn resolve_function_reads(
    declared: &[(String, Vec<String>)],
) -> TViewResult<Vec<(String, Vec<pg_sys::Oid>)>> {
    let mut reads = Vec::new();
    for (function, tables) in declared {
        let signature = Spi::connect(|client| {
            client
                .select(
                    &format!(
                        "SELECT {} FROM pg_catalog.pg_proc p \
                         WHERE p.oid = pg_catalog.to_regprocedure($1)",
                        crate::lineage::FUNCTION_SIGNATURE
                    ),
                    None,
                    &[crate::utils::spi::text(function)],
                )?
                .first()
                .get_one::<String>()
        })
        .or_else(|e| match e {
            spi::Error::InvalidPosition => Ok(None),
            e => Err(e),
        })
        .map_err(|e| crate::utils::spi::catalog_error("Look up a function of function_reads", &e))?
        .ok_or_else(|| {
            invalid(
                "function_reads",
                format!(
                    "function {function} does not exist: name it with its argument types, \
                     e.g. public.label_suffix() or public.price(bigint, date)"
                ),
            )
        })?;
        let tables = tables
            .iter()
            .map(|t| resolve_table(t, "function_reads"))
            .collect::<TViewResult<Vec<_>>>()?;
        reads.push((signature, tables));
    }
    Ok(reads)
}

/// Split a TVIEW name, `tv_<entity>`, `<entity>` or `schema.tv_<entity>`, into
/// its schema (if named) and entity. A part is taken as written, as the names of
/// earlier releases were; double-quote it (`""` for a quote) to include a dot.
///
/// # Errors
/// Returns an error if the name does not parse, or the entity is not a valid
/// identifier.
pub(crate) fn parse_name(name: &str) -> TViewResult<(Option<String>, String)> {
    let mut parts = split_identifiers(name)
        .ok_or_else(|| invalid("tview_name", format!("{name} is not a valid TVIEW name")))?;
    let table = parts.pop().unwrap_or_default();
    let schema = match parts.as_slice() {
        [] => None,
        [schema] => Some(schema.clone()),
        _ => {
            return Err(invalid(
                "tview_name",
                format!("{name} has too many dotted parts: use schema.tv_<entity>"),
            ));
        }
    };
    crate::validation::validate_sql_identifier(&table, "tview_name")?;
    let entity = table.strip_prefix("tv_").unwrap_or(&table);
    Ok((schema, entity.to_string()))
}

/// The dot-separated parts of `name`, double-quoted parts unquoted, or `None` if a
/// part is empty or a quote is not closed.
pub(super) fn split_identifiers(name: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut chars = name.chars().peekable();
    loop {
        let mut part = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next()? {
                    '"' if chars.peek() == Some(&'"') => {
                        chars.next();
                        part.push('"');
                    }
                    '"' => break,
                    c => part.push(c),
                }
            }
            if !matches!(chars.peek(), None | Some('.')) {
                return None;
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == '.' {
                    break;
                }
                part.push(c);
                chars.next();
            }
        }
        if part.is_empty() {
            return None;
        }
        parts.push(part);
        if chars.next().is_none() {
            return Some(parts);
        }
    }
}
