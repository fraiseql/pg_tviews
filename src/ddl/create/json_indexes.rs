//! Declared JSON-path indexes (option `indexes`, #226): expression btrees over
//! `data` written the way `FraiseQL` writes its filters and sorts, so the planner
//! matches them: `data->>'k'`, `data->'a'->>'b'`, `(data->>'k')::bigint`.
//!
//! An index on `data` makes every refresh of a row a non-HOT update (the
//! refresh rewrites `data`): a declared index costs writes for the reads it
//! serves.

use super::ManagedIndex;
use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys;
use serde_json::Value;

/// The type a key is cast to, as `FraiseQL` casts a typed field. Only immutable
/// casts: `text → timestamptz` (and `date`, `time`) depend on settings, so no
/// index can be built over them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyType {
    Text,
    Bigint,
    Numeric,
    Boolean,
}

impl KeyType {
    fn parse(name: &str) -> TViewResult<Self> {
        match name {
            "text" => Ok(Self::Text),
            "bigint" => Ok(Self::Bigint),
            "numeric" => Ok(Self::Numeric),
            "boolean" => Ok(Self::Boolean),
            "timestamptz" | "date" | "time" => Err(invalid(format!(
                "a cast to {name} is not immutable, so no index can serve it: sort the ISO \
                 8601 text instead (COLLATE \"C\"), or keep the value in a column of its own"
            ))),
            other => Err(invalid(format!(
                "unknown type \"{other}\" (known: text, bigint, numeric, boolean)"
            ))),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Bigint => "bigint",
            Self::Numeric => "numeric",
            Self::Boolean => "boolean",
        }
    }
}

/// One key of an index: a path into `data`, its type and its order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonKey {
    path: Vec<String>,
    key_type: KeyType,
    desc: bool,
}

/// A declared index: its keys, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonIndex {
    keys: Vec<JsonKey>,
}

fn invalid(reason: impl Into<String>) -> TViewError {
    TViewError::InvalidInput {
        parameter: "indexes".to_string(),
        reason: reason.into(),
    }
}

/// PostgreSQL's limit on the keys of an index.
const MAX_KEYS: usize = 32;

/// Parse the `indexes` option: an array of indexes, each a non-empty array of
/// keys, no index twice.
///
/// # Errors
/// [`TViewError::InvalidInput`] naming what is wrong.
pub fn parse(value: &Value) -> TViewResult<Vec<JsonIndex>> {
    let shape = || {
        invalid(
            "must be an array of indexes, each an array of keys: a path (\"author.id\") or \
             {\"path\": …, \"type\": \"text\"|\"bigint\"|\"numeric\"|\"boolean\", \"desc\": …}",
        )
    };
    let Value::Array(indexes) = value else {
        return Err(shape());
    };
    let mut parsed: Vec<JsonIndex> = Vec::new();
    for index in indexes {
        let Value::Array(keys) = index else {
            return Err(shape());
        };
        if keys.is_empty() || keys.len() > MAX_KEYS {
            return Err(invalid(format!(
                "an index has 1 to {MAX_KEYS} keys, not {}",
                keys.len()
            )));
        }
        let index = JsonIndex {
            keys: keys.iter().map(parse_key).collect::<TViewResult<_>>()?,
        };
        if parsed.contains(&index) {
            return Err(invalid(format!(
                "index {} is declared twice",
                index.canonical()
            )));
        }
        parsed.push(index);
    }
    Ok(parsed)
}

fn parse_key(key: &Value) -> TViewResult<JsonKey> {
    match key {
        Value::String(path) => Ok(JsonKey {
            path: parse_path(path)?,
            key_type: KeyType::Text,
            desc: false,
        }),
        Value::Object(fields) => {
            let mut path = None;
            let mut key_type = KeyType::Text;
            let mut desc = false;
            for (field, value) in fields {
                match (field.as_str(), value) {
                    ("path", Value::String(p)) => path = Some(parse_path(p)?),
                    ("type", Value::String(t)) => key_type = KeyType::parse(t)?,
                    ("desc", Value::Bool(d)) => desc = *d,
                    ("path" | "type" | "desc", _) => {
                        return Err(invalid(format!(
                            "a key's \"{field}\" must be a {}",
                            if field == "desc" { "boolean" } else { "string" }
                        )));
                    }
                    (other, _) => {
                        return Err(invalid(format!(
                            "unknown key field \"{other}\" (known: path, type, desc)"
                        )));
                    }
                }
            }
            Ok(JsonKey {
                path: path.ok_or_else(|| invalid("a key object needs a \"path\""))?,
                key_type,
                desc,
            })
        }
        _ => Err(invalid(
            "a key is a path string or an object with a \"path\"",
        )),
    }
}

fn parse_path(path: &str) -> TViewResult<Vec<String>> {
    let segments: Vec<String> = path.split('.').map(str::to_string).collect();
    if segments.iter().any(String::is_empty) {
        return Err(invalid(format!(
            "path \"{path}\" has an empty segment: name the keys from data down, \
             separated by dots"
        )));
    }
    Ok(segments)
}

impl JsonKey {
    /// The key as declared, shortest form: the path alone for an ascending text key.
    fn canonical(&self) -> Value {
        let path = self.path.join(".");
        if self.key_type == KeyType::Text && !self.desc {
            return Value::String(path);
        }
        let mut key = serde_json::Map::new();
        key.insert("path".into(), Value::String(path));
        if self.key_type != KeyType::Text {
            key.insert("type".into(), Value::String(self.key_type.name().into()));
        }
        if self.desc {
            key.insert("desc".into(), Value::Bool(true));
        }
        Value::Object(key)
    }

    /// The expression, as `pg_get_indexdef()` writes it, so a dump restores the
    /// same parse tree: `(data ->> 'k'::text)`, `((data -> 'a'::text) ->> 'b'::text)`,
    /// `((data ->> 'k'::text))::bigint`.
    fn expression(&self) -> String {
        let literal = |segment: &str| format!("{}::text", crate::utils::quote_literal(segment));
        let (last, objects) = self.path.split_last().unwrap_or((&self.path[0], &[]));
        let object = objects.iter().fold("data".to_string(), |e, s| {
            format!("({e} -> {})", literal(s))
        });
        let text = format!("({object} ->> {})", literal(last));
        match self.key_type {
            KeyType::Text => text,
            typed => format!("({text})::{}", typed.name()),
        }
    }

    /// Its part of the index name.
    fn label(&self) -> String {
        let mut label = self.path.join("_");
        if self.key_type != KeyType::Text {
            label = format!("{label}_{}", self.key_type.name());
        }
        if self.desc {
            label.push_str("_desc");
        }
        label
    }
}

impl JsonIndex {
    /// The `indexes` option ([`parse`]).
    ///
    /// # Errors
    /// [`TViewError::InvalidInput`] naming what is wrong.
    pub fn parse_option(value: &Value) -> TViewResult<Vec<Self>> {
        parse(value)
    }

    /// The index as declared, shortest form (`tviews.registry` publishes it).
    #[must_use]
    pub fn canonical(&self) -> Value {
        Value::Array(self.keys.iter().map(JsonKey::canonical).collect())
    }

    /// The index `pg_tviews` creates for it on `tview_name`:
    /// `idx_<tview>_data_<keys>`.
    #[must_use]
    pub fn managed(&self, tview_name: &str) -> ManagedIndex {
        let mut label = self
            .keys
            .iter()
            .map(JsonKey::label)
            .collect::<Vec<_>>()
            .join("_");
        // The data GIN index is idx_<tview>_data_gin.
        if label == "gin" {
            label.push_str("_text");
        }
        let elements = self
            .keys
            .iter()
            .map(|k| format!("({}){}", k.expression(), if k.desc { " DESC" } else { "" }))
            .collect::<Vec<_>>()
            .join(", ");
        ManagedIndex::expressions(tview_name, format!("data_{label}"), elements)
    }
}

/// The declared indexes as `tviews.registry` publishes them.
#[must_use]
pub fn canonical(indexes: &[JsonIndex]) -> Value {
    Value::Array(indexes.iter().map(JsonIndex::canonical).collect())
}

/// The declared indexes of `entity` that are on its table under their names, as
/// `pg_tviews`' own: what [`apply`] compares a declaration with.
///
/// # Errors
/// A failed catalog read.
pub(crate) fn present(entity: &str, table: pg_sys::Oid) -> TViewResult<Vec<JsonIndex>> {
    let tview = format!("tv_{entity}");
    let on_table = names_on(table)?;
    let recorded = crate::catalog::indexes::recorded(table)?;
    Ok(declared(entity)?
        .into_iter()
        .filter(|index| {
            let name = index.managed(&tview).name;
            on_table.contains(&name) && recorded.contains(&name)
        })
        .collect())
}

/// Bring `entity`'s table (`table`, in `schema`) to the `desired` indexes: drop
/// the declared ones no longer desired, create the missing ones, and store the
/// declaration. The others are left as they are.
///
/// # Errors
/// A TVIEW without a `data` column to index, a name another relation holds, a
/// failed `CREATE` / `DROP INDEX` or catalog write.
pub(crate) fn apply(
    entity: &str,
    schema: &str,
    table: pg_sys::Oid,
    desired: &[JsonIndex],
) -> TViewResult<()> {
    let tview = format!("tv_{entity}");
    if !desired.is_empty()
        && !crate::utils::column_types(table)?
            .iter()
            .any(|(c, _)| c == "data")
    {
        return Err(invalid(format!("{tview} has no data column to index")));
    }
    let present = present(entity, table)?;
    let wanted: Vec<ManagedIndex> = desired.iter().map(|i| i.managed(&tview)).collect();
    let mut dropped = Vec::new();
    for index in present.iter().map(|i| i.managed(&tview)) {
        if !wanted.iter().any(|w| w.name == index.name) {
            crate::utils::spi::run_ddl(&format!(
                "DROP INDEX {}.{}",
                crate::utils::ident::quoted(schema),
                crate::utils::ident::quoted(&index.name)
            ))?;
            dropped.push(index.name);
        }
    }
    crate::catalog::indexes::forget(table, &dropped)?;
    let present: Vec<String> = present.iter().map(|i| i.managed(&tview).name).collect();
    let mut created = Vec::new();
    for index in wanted.iter().filter(|w| !present.contains(&w.name)) {
        match index.create(schema, &tview)? {
            Some(name) => created.push(name),
            None => {
                return Err(TViewError::IndexNameReserved {
                    table: crate::utils::qualified_relname_from_oid(table)?,
                    index: index.name.clone(),
                });
            }
        }
    }
    crate::catalog::indexes::record(table, &created)?;
    crate::catalog::indexes::declare(entity, canonical(desired))
}

/// The `indexes` option `entity` declared.
///
/// # Errors
/// A failed catalog read, or a stored declaration this release cannot read.
pub(crate) fn declared(entity: &str) -> TViewResult<Vec<JsonIndex>> {
    parse(&crate::catalog::indexes::declared(entity)?)
}

/// The names of the indexes on `table`.
fn names_on(table: pg_sys::Oid) -> TViewResult<Vec<String>> {
    crate::utils::spi::strings(
        "SELECT c.relname::pg_catalog.text FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid WHERE i.indrelid = $1",
        &[crate::utils::spi::oid(table)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(value: &Value) -> JsonIndex {
        parse(&json!([value])).unwrap().remove(0)
    }

    #[test]
    fn expressions_are_written_as_pg_get_indexdef_writes_them() {
        let index =
            one(&json!(["status", "author.id", {"path": "age", "type": "bigint", "desc": true}]));
        let managed = index.managed("tv_item");
        assert_eq!(
            managed.name,
            "idx_tv_item_data_status_author_id_age_bigint_desc"
        );
        assert_eq!(
            managed.ddl("public", "tv_item"),
            "CREATE INDEX IF NOT EXISTS \"idx_tv_item_data_status_author_id_age_bigint_desc\" \
             ON \"public\".\"tv_item\" (((data ->> 'status'::text)), \
             (((data -> 'author'::text) ->> 'id'::text)), \
             (((data ->> 'age'::text))::bigint) DESC)"
        );
    }

    #[test]
    fn canonical_round_trips() {
        let declared = json!([["status"], [{"path": "price", "type": "numeric", "desc": true}],
                              [{"path": "a.b", "type": "text"}]]);
        let parsed = parse(&declared).unwrap();
        let published = canonical(&parsed);
        assert_eq!(
            published,
            json!([["status"], [{"path": "price", "type": "numeric", "desc": true}], ["a.b"]])
        );
        assert_eq!(parse(&published).unwrap(), parsed);
    }

    #[test]
    fn refusals() {
        assert!(parse(&json!(["status"])).is_err());
        assert!(parse(&json!([[]])).is_err());
        assert!(parse(&json!([["a..b"]])).is_err());
        assert!(parse(&json!([["a"], ["a"]])).is_err());
        assert!(parse(&json!([[{"path": "t", "type": "timestamptz"}]])).is_err());
        assert!(parse(&json!([[{"path": "t", "kind": "x"}]])).is_err());
        assert!(parse(&json!([[{"type": "bigint"}]])).is_err());
    }

    #[test]
    fn the_gin_label_is_kept_free() {
        assert_eq!(
            one(&json!(["gin"])).managed("tv_x").name,
            "idx_tv_x_data_gin_text"
        );
    }
}
