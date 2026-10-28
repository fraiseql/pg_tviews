//! Index and storage DDL of a new TVIEW.

fn post_schema() -> crate::ddl::create::ViewColumns {
    let text = |n: &str| (n.to_string(), "text".to_string());
    crate::ddl::create::ViewColumns::classify(vec![
        text("pk_post"),
        text("id"),
        text("fk_user"),
        text("user_id"),
        text("data"),
    ])
}

#[test]
fn test_index_ddl_never_indexes_rewritten_columns_by_default() {
    let ddl =
        crate::ddl::create::indexes::tview_index_ddl("tv_post", &post_schema(), "public", false);
    assert_eq!(ddl.len(), 3, "{ddl:?}");
    for stmt in &ddl {
        assert!(!stmt.contains("\"data\""), "indexes data: {stmt}");
        assert!(!stmt.contains("GIN"), "creates a GIN: {stmt}");
    }
}

#[test]
fn test_index_ddl_gin_only_when_requested() {
    let ddl =
        crate::ddl::create::indexes::tview_index_ddl("tv_post", &post_schema(), "public", true);
    assert_eq!(
        ddl.iter()
            .filter(|s| s.contains("USING GIN (\"data\")"))
            .count(),
        1
    );
}

#[test]
fn test_storage_clauses() {
    use crate::ddl::create::{Compression, DataStorage, Storage};
    let default = Storage::DEFAULT;
    assert_eq!(default.with_clause(), " WITH (fillfactor = 85)");
    assert_eq!(
        default.data_column_clause(),
        " STORAGE MAIN COMPRESSION lz4"
    );
    let declared = Storage {
        fillfactor: 100,
        toast_tuple_target: Some(4000),
        data_compression: Compression::Pglz,
        data_storage: DataStorage::Extended,
        ..default
    };
    assert_eq!(declared.with_clause(), " WITH (toast_tuple_target = 4000)");
    assert_eq!(
        declared.data_column_clause(),
        " STORAGE EXTENDED COMPRESSION pglz"
    );
    assert_eq!(
        Storage {
            fillfactor: 100,
            ..default
        }
        .with_clause(),
        ""
    );
    assert!(declared.rewrites_rows_from(&default));
    assert!(
        !Storage {
            fillfactor: 70,
            ..default
        }
        .rewrites_rows_from(&default)
    );
}

// ── Unit tests for index naming (no database required) ─────────────────────

#[test]
fn test_index_name_short_is_verbatim() {
    assert_eq!(
        crate::ddl::create::indexes::index_name("tv_post", "fk_user_pk_post"),
        "idx_tv_post_fk_user_pk_post"
    );
}

#[test]
fn test_index_name_long_fits_and_stays_unique() {
    let entity = "a".repeat(60);
    let a = crate::ddl::create::indexes::index_name(&format!("tv_{entity}"), "fk_left_pk_x");
    let b = crate::ddl::create::indexes::index_name(&format!("tv_{entity}"), "fk_right_pk_x");
    assert_eq!(a.len(), crate::utils::MAX_IDENTIFIER_BYTES);
    assert_ne!(a, b);
    assert_eq!(
        a,
        crate::ddl::create::indexes::index_name(&format!("tv_{entity}"), "fk_left_pk_x")
    );
}

#[test]
fn test_index_name_truncates_on_char_boundary() {
    let name =
        crate::ddl::create::indexes::index_name(&format!("tv_{}", "é".repeat(40)), "fk_x_pk_y");
    assert!(name.len() <= crate::utils::MAX_IDENTIFIER_BYTES);
}

#[test]
fn test_propagation_index_ddl() {
    assert_eq!(
        crate::ddl::create::ManagedIndex::propagation("tv_post", "fk_user", "pk_post")
            .ddl("public", "tv_post"),
        "CREATE INDEX IF NOT EXISTS \"idx_tv_post_fk_user_pk_post\" \
         ON \"public\".\"tv_post\" (\"fk_user\", \"pk_post\")"
    );
}
