//! The collections a TVIEW's rows belong to, as the change feed publishes them
//! (ADR 0231, payload version 2).
//!
//! Each reference of the TVIEW to another one ([`crate::lineage::Reference`],
//! resolved by `tviews.pg_tview_references`) makes a collection: the comments of
//! post P are the `tv_comment` rows whose `fk_post` holds P's key, and their
//! collection key is `Comment.fk_post:<P's public id>`. A write of a child row
//! returns, per reference, the parent's key and, when it recomputes the row, the
//! parent's public id ([`TviewMeta::reference_values`]); a patch returns the key
//! only ([`TviewMeta::returned_keys`]). The keys the row held
//! before come from the read that locked it ([`OldKeys`]). What a write leaves
//! unresolved, the flush resolves.

use super::{PublicId, TviewMeta};
use std::rc::Rc;

/// One reference of a TVIEW, as the change feed names its collections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// `<child type>.<column>`: `Comment.fk_post`.
    pub key: Rc<str>,
    /// The child's type: what the change feed names when it cannot list the
    /// collection's parents.
    pub child_type: Rc<str>,
    /// The child's reference column.
    pub column: String,
    /// The referenced TVIEW's table, schema-qualified and quoted.
    pub parent_table: String,
    pub parent_table_oid: pgrx::pg_sys::Oid,
    /// The referenced TVIEW's identity column, and its type (`format_type`).
    pub parent_column: String,
    pub parent_type: String,
    /// How the parent's public id is written; `None` without an `id` column.
    pub parent_id: Option<PublicId>,
    /// The parent's type, and whether its identity is its `pk_<entity>`, the key
    /// its own written rows are journaled by.
    pub parent_typename: Rc<str>,
    pub parent_keyed_by_pk: bool,
}

/// A parent a written row names: its key, and its public id as text when the
/// write read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentRef {
    pub key: String,
    pub id: Option<String>,
}

/// The reference keys a row held before a write, by `pk_<entity>`, read when the
/// rows were locked ([`TviewMeta::reference_keys`]).
pub type OldKeys = std::collections::HashMap<i64, Vec<Option<String>>>;

/// SQL over the catalog row of [`super::meta_select`]: the TVIEW's collections, a
/// `jsonb` array of `[column, parent table, parent oid, parent column, its type,
/// id kind, parent entity, parent typename option]`, the parent of each reference
/// the one `tviews.pg_tview_references` marks `chosen`, as `tviews.registry` publishes.
pub(super) fn collections_sql() -> String {
    let schema = crate::utils::ext_schema();
    format!(
        "(SELECT pg_catalog.jsonb_agg(pg_catalog.jsonb_build_array( \
                     x.column_name, pg_catalog.format('%I.%I', pn.nspname, pc.relname), \
                     x.parent_table::pg_catalog.int8, x.parent_column, \
                     pg_catalog.format_type(pa.atttypid, NULL), \
                     (SELECT CASE WHEN ia.atttypid IN ('pg_catalog.int2'::pg_catalog.regtype, \
                                                      'pg_catalog.int4'::pg_catalog.regtype, \
                                                      'pg_catalog.int8'::pg_catalog.regtype, \
                                                      'pg_catalog.numeric'::pg_catalog.regtype) \
                                  THEN 'number' ELSE 'string' END \
                      FROM pg_catalog.pg_attribute ia \
                      WHERE ia.attrelid = x.parent_table AND ia.attname = 'id' \
                        AND ia.attnum > 0 AND NOT ia.attisdropped), \
                     x.parent, pm.graphql_typename) \
                 ORDER BY x.column_name) \
          FROM {schema}.pg_tview_references x \
          JOIN {schema}.pg_tview_meta pm ON pm.entity = x.parent \
          JOIN pg_catalog.pg_class pc ON pc.oid = x.parent_table \
          JOIN pg_catalog.pg_namespace pn ON pn.oid = pc.relnamespace \
          JOIN pg_catalog.pg_attribute pa \
            ON pa.attrelid = x.parent_table AND pa.attname = x.parent_column \
          WHERE x.entity = pg_tview_meta.entity AND x.chosen) AS collections"
    )
}

/// Decode [`collections_sql`] for the TVIEW of type `child_type`.
pub(super) fn decode(
    child_type: &Rc<str>,
    json: Option<&serde_json::Value>,
) -> Vec<Rc<Collection>> {
    let Some(serde_json::Value::Array(rows)) = json else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let text = |i: usize| row.get(i)?.as_str().map(str::to_string);
            let column = text(0)?;
            let parent_column = text(3)?;
            let parent_entity = text(6)?;
            Some(Rc::new(Collection {
                key: format!("{child_type}.{column}").into(),
                child_type: Rc::clone(child_type),
                column,
                parent_table: text(1)?,
                parent_table_oid: pgrx::pg_sys::Oid::from(
                    u32::try_from(row.get(2)?.as_u64()?).ok()?,
                ),
                parent_keyed_by_pk: parent_column == format!("pk_{parent_entity}"),
                parent_column,
                parent_type: text(4)?,
                parent_id: match row.get(5)?.as_str() {
                    Some("number") => Some(PublicId::Number),
                    Some(_) => Some(PublicId::String),
                    None => None,
                },
                parent_typename: super::typename(&parent_entity, text(7)).into(),
            }))
        })
        .collect()
}

impl TviewMeta {
    /// What a write of this TVIEW's rows returns after `pk_<entity>`: the rows'
    /// public id ([`Self::id_text`]), then [`Self::reference_values`].
    #[must_use]
    pub fn returned(&self, table: &str, pre_image: bool) -> String {
        format!(
            "{}, {}",
            self.id_text(table),
            self.reference_values(table, pre_image)
        )
    }

    /// What a patch of this TVIEW's rows returns after `pk_<entity>`, read as text
    /// columns: the rows' public id, then, when the change feed names
    /// collections, one column per reference with the parent's key (a patch of
    /// `data` changes none: no id is read, the flush resolves the keys).
    #[must_use]
    pub fn returned_keys(&self, table: &str) -> String {
        let mut columns = vec![self.id_text(table)];
        if self.publishes_collections() {
            columns.extend(self.collections.iter().map(|c| {
                format!(
                    "{table}.{}::pg_catalog.text",
                    crate::utils::ident::quoted(&c.column)
                )
            }));
        }
        columns.join(", ")
    }

    /// Whether the change feed names the collections of this TVIEW's rows.
    #[must_use]
    pub fn publishes_collections(&self) -> bool {
        !self.collections.is_empty()
            && crate::config::publish_changes() == Some(crate::queue::published::Publish::Rows)
    }

    /// When the change feed names collections, per reference of the row written
    /// (`table`, its name or alias in the statement) the parent's key and its
    /// public id, read by an index probe, as a `text[]` of pairs; NULL otherwise.
    /// For the writes of a recompute, an insert or a delete, whose rows are few.
    /// With `pre_image`, the pairs of the row before the statement follow, for a
    /// write that can change them and whose rows were not read first
    /// ([`OldKeys`]): a subquery in `RETURNING` reads the statement's snapshot,
    /// which does not see the row version the statement writes.
    #[must_use]
    pub fn reference_values(&self, table: &str, pre_image: bool) -> String {
        if !self.publishes_collections() {
            return "NULL::pg_catalog.text[]".to_string();
        }
        let pairs = |row: &str| -> Vec<String> {
            self.collections
                .iter()
                .flat_map(|c| {
                    let key = format!("{row}.{}", crate::utils::ident::quoted(&c.column));
                    let id = match c.parent_id {
                        Some(_) => format!(
                            "(SELECT p.\"id\"::pg_catalog.text FROM {} p \
                             WHERE p.{} OPERATOR(pg_catalog.=) {key})",
                            c.parent_table,
                            crate::utils::ident::quoted(&c.parent_column)
                        ),
                        None => "NULL::pg_catalog.text".to_string(),
                    };
                    [format!("{key}::pg_catalog.text"), id]
                })
                .collect()
        };
        let new = format!("ARRAY[{}]", pairs(table).join(", "));
        if !pre_image {
            return new;
        }
        let key = crate::utils::ident::quoted(&self.identity.column);
        let qi_tv = crate::utils::qualified_relname_from_oid(self.tview_oid)
            .unwrap_or_else(|_| table.to_string());
        format!(
            "{new} OPERATOR(pg_catalog.||) COALESCE((SELECT ARRAY[{}] FROM {qi_tv} o \
             WHERE o.{key} OPERATOR(pg_catalog.=) {table}.{key}), '{{}}'::pg_catalog.text[])",
            pairs("o").join(", ")
        )
    }

    /// The reference keys of the row `table` as a `text[]`, for the read that
    /// locks the rows a recompute writes: the keys they held before it
    /// ([`OldKeys`]). NULL when the feed names no collection.
    #[must_use]
    pub fn reference_keys(&self, table: &str) -> String {
        if !self.publishes_collections() {
            return "NULL::pg_catalog.text[]".to_string();
        }
        let keys: Vec<String> = self
            .collections
            .iter()
            .map(|c| {
                format!(
                    "{table}.{}::pg_catalog.text",
                    crate::utils::ident::quoted(&c.column)
                )
            })
            .collect();
        format!("ARRAY[{}]", keys.join(", "))
    }

    /// The parents a written row names ([`parents`]).
    #[must_use]
    pub fn parents(
        &self,
        values: &[Option<String>],
        old: Option<&[Option<String>]>,
    ) -> Vec<(Rc<Collection>, ParentRef)> {
        parents(&self.collections, values, old)
    }
}

/// The reference keys of [`TviewMeta::returned_keys`], read back as the pairs
/// [`parents`] reads: each key without an id.
#[must_use]
pub fn key_values(keys: &[Option<String>]) -> Vec<Option<String>> {
    keys.iter().flat_map(|k| [k.clone(), None]).collect()
}

/// The parents a written row names, once each, new before old: per collection,
/// the `(key, id)` pair the write returned ([`TviewMeta::reference_values`]),
/// then the pair of the row before it (a pre-image in `values`), then the key it
/// held when it was locked (`old`).
#[must_use]
pub fn parents(
    collections: &[Rc<Collection>],
    values: &[Option<String>],
    old: Option<&[Option<String>]>,
) -> Vec<(Rc<Collection>, ParentRef)> {
    let n = collections.len();
    let pair = |at: usize| -> Option<ParentRef> {
        Some(ParentRef {
            key: values.get(at)?.clone()?,
            id: values.get(at + 1).cloned().flatten(),
        })
    };
    let mut parents: Vec<(Rc<Collection>, ParentRef)> = Vec::new();
    for (i, collection) in collections.iter().enumerate() {
        let locked = old
            .and_then(|o| o.get(i).cloned().flatten())
            .map(|key| ParentRef { key, id: None });
        for parent in [pair(2 * i), pair(2 * (n + i)), locked]
            .into_iter()
            .flatten()
        {
            if !parents
                .iter()
                .any(|(c, p)| Rc::ptr_eq(c, collection) && p.key == parent.key)
            {
                parents.push((Rc::clone(collection), parent));
            }
        }
    }
    parents
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded() -> Vec<Rc<Collection>> {
        decode(
            &Rc::from("Comment"),
            Some(&serde_json::json!([
                [
                    "fk_author",
                    "public.tv_user",
                    16400,
                    "pk_user",
                    "integer",
                    "string",
                    "user",
                    null
                ],
                [
                    "fk_post",
                    "public.tv_post",
                    16410,
                    "pk_post",
                    "integer",
                    null,
                    "post",
                    "Article"
                ],
                [
                    "fk_doc",
                    "public.tv_doc",
                    16420,
                    "id",
                    "uuid",
                    "string",
                    "doc",
                    null
                ]
            ])),
        )
    }

    fn text(values: &[Option<&str>]) -> Vec<Option<String>> {
        values.iter().map(|v| v.map(str::to_string)).collect()
    }

    fn named(
        values: &[Option<&str>],
        old: Option<&[Option<&str>]>,
    ) -> Vec<(String, String, Option<String>)> {
        let collections = decoded();
        let old = old.map(text);
        parents(&collections, &text(values), old.as_deref())
            .into_iter()
            .map(|(c, p)| (c.key.to_string(), p.key, p.id))
            .collect()
    }

    fn row(key: &str, parent: &str, id: Option<&str>) -> (String, String, Option<String>) {
        (key.to_string(), parent.to_string(), id.map(str::to_string))
    }

    #[test]
    fn a_collection_is_keyed_by_child_type_and_column() {
        let c = decoded();
        assert_eq!(&*c[0].key, "Comment.fk_author");
        assert_eq!(&*c[0].child_type, "Comment");
        assert_eq!(c[0].parent_id, Some(PublicId::String));
        assert_eq!(&*c[0].parent_typename, "User");
        assert!(c[0].parent_keyed_by_pk);
        assert_eq!(c[1].parent_id, None, "a parent without an id column");
        assert_eq!(&*c[1].parent_typename, "Article", "the typename option");
        assert!(!c[2].parent_keyed_by_pk, "a parent named by another column");
        assert!(decode(&Rc::from("Comment"), None).is_empty());
    }

    #[test]
    fn patched_keys_read_as_pairs_without_ids() {
        let pairs = key_values(&text(&[Some("7"), Some("2"), None]));
        assert_eq!(pairs, text(&[Some("7"), None, Some("2"), None, None, None]));
    }

    #[test]
    fn a_recompute_names_each_parent_with_its_id() {
        // Inserted: no old values; a NULL reference names no parent.
        assert_eq!(
            named(
                &[Some("7"), Some("u7"), None, None, Some("d"), Some("did")],
                None
            ),
            [
                row("Comment.fk_author", "7", Some("u7")),
                row("Comment.fk_doc", "d", Some("did")),
            ]
        );
    }

    #[test]
    fn a_moved_row_names_the_parent_it_left_once() {
        // Moved from post 1 to post 2, read when locked: new first, the old key to
        // resolve; the unchanged author once.
        assert_eq!(
            named(
                &[Some("7"), Some("u7"), Some("2"), None, None, None],
                Some(&[Some("7"), Some("1"), None])
            ),
            [
                row("Comment.fk_author", "7", Some("u7")),
                row("Comment.fk_post", "2", None),
                row("Comment.fk_post", "1", None),
            ]
        );
        // The same move seen through a pre-image in the returned values.
        assert_eq!(
            named(
                &[
                    Some("7"),
                    Some("u7"),
                    Some("2"),
                    None,
                    None,
                    None,
                    Some("7"),
                    Some("u7"),
                    Some("1"),
                    None,
                    None,
                    None,
                ],
                None
            ),
            [
                row("Comment.fk_author", "7", Some("u7")),
                row("Comment.fk_post", "2", None),
                row("Comment.fk_post", "1", None),
            ]
        );
    }
}
