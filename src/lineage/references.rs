//! The references of a TVIEW: its output columns that a `FOREIGN KEY` of its root
//! table proves to hold another table's key.
//!
//! A reference is derived, never guessed from a name: the output column is traced
//! to a column of the table holding the identity, and that column carries a
//! single-column foreign key. The plan stores what the catalog proves about the
//! child; `tviews.registry.references` publishes those whose referenced column is
//! the source of another registered TVIEW's identity, resolved live, so a parent
//! registered later is found.

use super::QueryGraph;

/// An output column of the TVIEW traced to a column of its root table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootColumn {
    /// The output column.
    pub output: String,
    /// The root table's column it stands for.
    pub column: String,
    pub attnum: i16,
}

/// A foreign key of the root table, as `pg_constraint` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub name: String,
    /// The constrained columns of the root table.
    pub attnums: Vec<i16>,
    /// The referenced table, schema-qualified and quoted.
    pub table: String,
    /// The referenced columns.
    pub columns: Vec<String>,
}

/// What the catalog says about the root table's columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootConstraints {
    pub foreign_keys: Vec<ForeignKey>,
    /// The columns a unique index or constraint of their own covers.
    pub unique: Vec<i16>,
    /// The columns declared `NOT NULL`.
    pub not_null: Vec<i16>,
}

/// An output column holding another table's key, proven by a foreign key.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Reference {
    /// The output column of the TVIEW.
    pub column: String,
    /// The root table's column it is traced to.
    pub source: String,
    /// The referenced table, schema-qualified and quoted.
    pub table: String,
    /// The referenced column.
    pub key: String,
    /// The foreign key that proves it.
    pub constraint: String,
    /// One child row at most per referenced key (a one-to-one relationship).
    pub unique: bool,
    pub nullable: bool,
}

impl QueryGraph {
    /// The output columns that stand for a column of the root occurrence, with the
    /// root table: none for a set operation or a key with several roots (UNION
    /// branches), whose rows come from more than one table.
    #[must_use]
    pub fn root_columns(&self) -> Option<(u32, Vec<RootColumn>)> {
        let [root] = self.roots.as_slice() else {
            return None;
        };
        if self.set_operation {
            return None;
        }
        let occ = root.key.occ;
        let columns = self
            .outputs
            .iter()
            .filter_map(|(output, column)| {
                column
                    .as_ref()
                    .filter(|c| c.occ == occ)
                    .map(|c| RootColumn {
                        output: output.clone(),
                        column: c.name.clone(),
                        attnum: c.attnum,
                    })
            })
            .collect();
        Some((self.occurrences[occ].relid, columns))
    }
}

/// The references among `columns`: each output column whose root column carries a
/// single-column foreign key, one per key, in output order.
#[must_use]
pub fn references(columns: &[RootColumn], constraints: &RootConstraints) -> Vec<Reference> {
    columns
        .iter()
        .flat_map(|c| {
            constraints
                .foreign_keys
                .iter()
                .filter(move |fk| fk.attnums == [c.attnum])
                .filter_map(move |fk| {
                    let [key] = fk.columns.as_slice() else {
                        return None;
                    };
                    Some(Reference {
                        column: c.output.clone(),
                        source: c.column.clone(),
                        table: fk.table.clone(),
                        key: key.clone(),
                        constraint: fk.name.clone(),
                        unique: constraints.unique.contains(&c.attnum),
                        nullable: !constraints.not_null.contains(&c.attnum),
                    })
                })
        })
        .collect()
}

/// The foreign keys, unique columns and `NOT NULL` columns of table `relid`.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn root_constraints(relid: u32) -> crate::TViewResult<RootConstraints> {
    use crate::utils::spi;
    let table = [spi::oid(pgrx::pg_sys::Oid::from(relid))];
    let foreign_keys = spi::rows(
        "SELECT k.conname::pg_catalog.text, k.conkey, \
                pg_catalog.format('%I.%I', n.nspname, r.relname), \
                ARRAY(SELECT a.attname::pg_catalog.text \
                      FROM pg_catalog.unnest(k.confkey) WITH ORDINALITY u(attnum, i) \
                      JOIN pg_catalog.pg_attribute a \
                        ON a.attrelid = k.confrelid AND a.attnum = u.attnum ORDER BY u.i) \
         FROM pg_catalog.pg_constraint k \
         JOIN pg_catalog.pg_class r ON r.oid = k.confrelid \
         JOIN pg_catalog.pg_namespace n ON n.oid = r.relnamespace \
         WHERE k.conrelid = $1 AND k.contype = 'f' ORDER BY k.conname",
        &table,
        |row| {
            Ok(Some(ForeignKey {
                name: row.get::<String>(1)?.unwrap_or_default(),
                attnums: row.get::<Vec<i16>>(2)?.unwrap_or_default(),
                table: row.get::<String>(3)?.unwrap_or_default(),
                columns: row.get::<Vec<String>>(4)?.unwrap_or_default(),
            }))
        },
    )?
    .into_iter()
    .flatten()
    .collect();
    let unique = spi::rows(
        "SELECT DISTINCT i.indkey[0] FROM pg_catalog.pg_index i \
         WHERE i.indrelid = $1 AND i.indisunique AND i.indnkeyatts = 1 AND i.indkey[0] > 0 \
           AND i.indpred IS NULL AND i.indexprs IS NULL",
        &table,
        |row| Ok(row.get::<i16>(1)?),
    )?
    .into_iter()
    .flatten()
    .collect();
    let not_null = spi::rows(
        "SELECT a.attnum FROM pg_catalog.pg_attribute a \
         WHERE a.attrelid = $1 AND a.attnum > 0 AND a.attnotnull AND NOT a.attisdropped",
        &table,
        |row| Ok(row.get::<i16>(1)?),
    )?
    .into_iter()
    .flatten()
    .collect();
    Ok(RootConstraints {
        foreign_keys,
        unique,
        not_null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(output: &str, column: &str, attnum: i16) -> RootColumn {
        RootColumn {
            output: output.into(),
            column: column.into(),
            attnum,
        }
    }

    fn fk(name: &str, attnums: &[i16], table: &str, columns: &[&str]) -> ForeignKey {
        ForeignKey {
            name: name.into(),
            attnums: attnums.to_vec(),
            table: table.into(),
            columns: columns.iter().map(|c| (*c).to_string()).collect(),
        }
    }

    /// `tb_comment(pk_comment 1, fk_post 2, fk_author 3, …)`, all `NOT NULL`.
    fn comment(foreign_keys: Vec<ForeignKey>) -> RootConstraints {
        RootConstraints {
            foreign_keys,
            unique: vec![1],
            not_null: vec![1, 2, 3],
        }
    }

    fn columns() -> Vec<RootColumn> {
        vec![
            root("pk_comment", "pk_comment", 1),
            root("fk_post", "fk_post", 2),
            root("fk_author", "fk_author", 3),
        ]
    }

    #[test]
    fn a_role_named_key_references_the_table_its_constraint_names() {
        let refs = references(
            &columns(),
            &comment(vec![
                fk(
                    "tb_comment_fk_author_fkey",
                    &[3],
                    "public.tb_user",
                    &["pk_user"],
                ),
                fk(
                    "tb_comment_fk_post_fkey",
                    &[2],
                    "public.tb_post",
                    &["pk_post"],
                ),
            ]),
        );
        assert_eq!(
            refs,
            [
                Reference {
                    column: "fk_post".into(),
                    source: "fk_post".into(),
                    table: "public.tb_post".into(),
                    key: "pk_post".into(),
                    constraint: "tb_comment_fk_post_fkey".into(),
                    unique: false,
                    nullable: false,
                },
                Reference {
                    column: "fk_author".into(),
                    source: "fk_author".into(),
                    table: "public.tb_user".into(),
                    key: "pk_user".into(),
                    constraint: "tb_comment_fk_author_fkey".into(),
                    unique: false,
                    nullable: false,
                },
            ]
        );
    }

    #[test]
    fn a_column_without_a_constraint_is_no_reference() {
        assert!(references(&columns(), &comment(vec![])).is_empty());
    }

    #[test]
    fn a_composite_foreign_key_is_no_reference() {
        let refs = references(
            &columns(),
            &comment(vec![fk(
                "tb_comment_pair_fkey",
                &[2, 3],
                "public.tb_pair",
                &["a", "b"],
            )]),
        );
        assert!(refs.is_empty());
    }

    #[test]
    fn unique_and_nullable_come_from_the_root_column() {
        let mut constraints = comment(vec![fk(
            "tb_profile_fk_user_fkey",
            &[2],
            "public.tb_user",
            &["pk_user"],
        )]);
        constraints.unique.push(2);
        constraints.not_null.retain(|a| *a != 2);
        let refs = references(&columns(), &constraints);
        assert!(refs[0].unique, "a unique column is one-to-one");
        assert!(refs[0].nullable, "a column without NOT NULL is nullable");
    }

    #[test]
    fn a_root_column_projected_twice_is_two_references() {
        let refs = references(
            &[root("fk_post", "fk_post", 2), root("post_pk", "fk_post", 2)],
            &comment(vec![fk(
                "tb_comment_fk_post_fkey",
                &[2],
                "public.tb_post",
                &["pk_post"],
            )]),
        );
        let outputs: Vec<&str> = refs.iter().map(|r| r.column.as_str()).collect();
        assert_eq!(outputs, ["fk_post", "post_pk"]);
    }

    #[test]
    fn quoted_names_are_kept_as_the_catalog_writes_them() {
        let refs = references(
            &[root("Post Ref", "Post Ref", 2)],
            &comment(vec![fk(
                "Comment Post",
                &[2],
                r#""Blog"."Post""#,
                &["Post Key"],
            )]),
        );
        assert_eq!(refs[0].table, r#""Blog"."Post""#);
        assert_eq!(refs[0].key, "Post Key");
        assert_eq!(refs[0].column, "Post Ref");
    }

    mod root_columns {
        use crate::lineage::{Column, Occurrence, QueryGraph, Root};

        fn occ(relid: u32) -> Occurrence {
            Occurrence {
                relid,
                relname: format!("t{relid}"),
                qualified: format!("public.t{relid}"),
                unions: Vec::new(),
                via_view: None,
                via_tview: None,
                in_sublink: false,
                opaque_level: None,
                unwatched: None,
                tview_table: None,
            }
        }

        fn col(occ: usize, attnum: i16, name: &str) -> Column {
            Column {
                occ,
                attnum,
                name: name.into(),
            }
        }

        fn root(key: Column) -> Root {
            Root {
                key,
                expr: None,
                scope: Vec::new(),
            }
        }

        /// `tb_comment c JOIN tb_comment parent …`: outputs from both occurrences.
        fn self_join() -> QueryGraph {
            QueryGraph {
                occurrences: vec![occ(10), occ(10)],
                roots: vec![root(col(0, 1, "pk_comment"))],
                outputs: vec![
                    ("pk_comment".into(), Some(col(0, 1, "pk_comment"))),
                    ("fk_post".into(), Some(col(0, 2, "fk_post"))),
                    ("parent_post".into(), Some(col(1, 2, "fk_post"))),
                    ("data".into(), None),
                ],
                ..QueryGraph::default()
            }
        }

        #[test]
        fn only_the_root_occurrence_s_columns() {
            let (relid, columns) = self_join().root_columns().expect("one root");
            assert_eq!(relid, 10);
            let outputs: Vec<&str> = columns.iter().map(|c| c.output.as_str()).collect();
            assert_eq!(outputs, ["pk_comment", "fk_post"]);
        }

        #[test]
        fn a_union_has_none() {
            let mut g = self_join();
            g.roots.push(root(col(1, 1, "pk_comment")));
            assert!(g.root_columns().is_none());
            let mut g = self_join();
            g.set_operation = true;
            assert!(g.root_columns().is_none());
        }
    }
}
