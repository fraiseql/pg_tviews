use pgrx::prelude::*;
use pgrx::pg_sys::Oid;
use pgrx::JsonB;

use crate::catalog::TviewMeta;
use crate::propagate::propagate_from_row;
use crate::utils::{lookup_view_for_source, relname_from_oid};

/// Represents a materialized view row pulled from v_entity.
pub struct ViewRow {
    pub entity_name: String,
    pub pk: i64,
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub data: JsonB,
    pub fk_values: Vec<(String, i64)>,    // e.g. [("fk_user", 7)]
    pub uuid_fk_values: Vec<(String, String)>, // e.g. [("user_id", "...")]
}

pub fn refresh_pk(source_oid: Oid, pk: i64) -> spi::Result<()> {
    // 1. Find TVIEW metadata (tview_oid, view_oid, entity_name, etc.)
    let meta = TviewMeta::load_for_source(source_oid)?;
    let meta = match meta {
        Some(m) => m,
        None => {
            error!("No TVIEW metadata for source_oid: {:?}", source_oid);
        }
    };

    // 2. Recompute row from v_entity
    let view_row = recompute_view_row(&meta, pk)?;

    // 3. Patch tv_entity using jsonb_ivm
    apply_patch(&view_row)?;

    // 4. Propagate to parent entities
    propagate_from_row(&view_row)?;

    Ok(())
}

/// Recompute view row from v_entity WHERE pk = $1
fn recompute_view_row(meta: &TviewMeta, pk: i64) -> spi::Result<ViewRow> {
    let view_name = lookup_view_for_source(meta.view_oid)?;
    let pk_col = format!("pk_{}", meta.entity_name); // e.g. pk_post

    let sql = format!(
        "SELECT * FROM {} WHERE {} = $1",
        view_name, pk_col,
    );

    Spi::connect(|client| {
        let rows = client.select(
            &sql,
            None,
            Some(vec![(PgOid::BuiltIn(PgBuiltInOids::INT8OID), pk.into_datum())]),
        )?;

        let mut row_data = None;
        for r in rows {
            row_data = Some(r);
            break;
        }
        let row_data = match row_data {
            Some(r) => r,
            None => error!("No row in v_* for given pk: {}", pk),
        };

        // Extract data column
        let data: JsonB = row_data["data"].value().unwrap().unwrap();

        // Extract FK columns
        let fk_values = extract_fk_columns(meta, &row_data)?;
        let uuid_fk_values = extract_uuid_fk_columns(meta, &row_data)?;

        Ok(ViewRow {
            entity_name: meta.entity_name.clone(),
            pk,
            tview_oid: meta.tview_oid,
            view_oid: meta.view_oid,
            data,
            fk_values,
            uuid_fk_values,
        })
    })
}

/// Extract FK column values (integer FKs) from a view row
fn extract_fk_columns(
    meta: &TviewMeta,
    row_data: &spi::SpiHeapTupleData,
) -> spi::Result<Vec<(String, i64)>> {
    let mut fk_values = Vec::new();

    for fk_col in &meta.fk_columns {
        // Try to extract the FK value
        if let Ok(Some(val)) = row_data[fk_col.as_str()].value::<i64>() {
            fk_values.push((fk_col.clone(), val));
        }
    }

    Ok(fk_values)
}

/// Extract UUID FK column values from a view row
fn extract_uuid_fk_columns(
    meta: &TviewMeta,
    row_data: &spi::SpiHeapTupleData,
) -> spi::Result<Vec<(String, String)>> {
    let mut uuid_fk_values = Vec::new();

    for uuid_col in &meta.uuid_fk_columns {
        // Try to extract the UUID FK value as String
        if let Ok(Some(val)) = row_data[uuid_col.as_str()].value::<String>() {
            uuid_fk_values.push((uuid_col.clone(), val));
        }
    }

    Ok(uuid_fk_values)
}

/// Apply JSON patch to tv_entity for pk using jsonb_ivm_patch.
/// For now, this stub replaces the JSON instead of calling jsonb_ivm_patch.
fn apply_patch(row: &ViewRow) -> spi::Result<()> {
    let tv_name = relname_from_oid(row.tview_oid)?;
    let pk_col = format!("pk_{}", row.entity_name);

    // TODO: call jsonb_ivm_patch(data, $1) instead of direct replacement
    let sql = format!(
        "UPDATE {} \
         SET data = $1, updated_at = now() \
         WHERE {} = $2",
        tv_name, pk_col
    );

    Spi::connect(|mut client| {
        client.update(
            &sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), JsonB(row.data.0.clone()).into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::INT8OID), row.pk.into_datum()),
            ]),
        )?;
        Ok(())
    })
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;
    use pgrx::JsonB;

    /// Test smart patching for nested object dependencies.
    ///
    /// This test verifies that when a nested object (like 'author') changes,
    /// only that specific path in the JSONB is updated, not the entire document.
    ///
    /// Expected to FAIL initially because apply_patch() does full replacement.
    #[pg_test]
    fn test_apply_patch_nested_object() {
        // Setup: Create tables with FK relationship
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )").unwrap();

        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Hello')").unwrap();

        // Create TVIEW with nested author object
        Spi::run("
            SELECT pg_tviews_create(
                'post',
                $$
                SELECT pk_post, fk_user,
                       jsonb_build_object(
                           'title', title,
                           'author', v_user.data
                       ) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
                $$
            )
        ").unwrap();

        // Verify metadata captured nested dependency
        let meta = Spi::get_one::<String>("
            SELECT dependency_types::text FROM pg_tview_meta
            WHERE entity_name = 'post'
        ").unwrap().unwrap();
        assert!(meta.contains("nested_object"), "Expected nested_object dependency, got: {}", meta);

        // Initial state
        let initial_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();

        let initial_json = &initial_data.0;
        assert_eq!(initial_json["title"], "Hello");
        assert_eq!(initial_json["author"]["name"], "Alice");

        // Update user name
        Spi::run("UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1").unwrap();

        // Trigger cascade by calling refresh_pk directly
        let source_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tb_user'::regclass::oid")
            .unwrap()
            .unwrap();

        crate::refresh::refresh_pk(source_oid, 1).unwrap();

        // Verify: author.name changed, title unchanged
        let updated_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();

        let updated_json = &updated_data.0;

        // These assertions will FAIL with current full-replacement code
        // because full replacement may reorder keys or lose unchanged values
        assert_eq!(updated_json["title"], "Hello",
            "Title should NOT be touched by smart patch");
        assert_eq!(updated_json["author"]["name"], "Alice Updated",
            "Author name should be updated via smart patch");
    }

    /// Test smart patching for array dependencies.
    ///
    /// This test verifies that when an element in an array (like 'comments') changes,
    /// only that specific element is updated, not the entire array.
    ///
    /// Expected to FAIL initially because apply_patch() does full replacement.
    #[pg_test]
    fn test_apply_patch_array() {
        // Setup: Create tables with FK relationships
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )").unwrap();
        Spi::run("CREATE TABLE tb_comment (
            pk_comment BIGSERIAL PRIMARY KEY,
            fk_post BIGINT REFERENCES tb_post(pk_post),
            fk_user BIGINT REFERENCES tb_user(pk_user),
            text TEXT
        )").unwrap();

        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Hello')").unwrap();
        Spi::run("INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (1, 1, 1, 'Great post!')").unwrap();
        Spi::run("INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (2, 1, 1, 'Thanks!')").unwrap();

        // Create TVIEW with array of comments
        Spi::run("
            SELECT pg_tviews_create(
                'post',
                $$
                SELECT pk_post, fk_user,
                       jsonb_build_object(
                           'title', title,
                           'author', v_user.data,
                           'comments', COALESCE(jsonb_agg(v_comment.data ORDER BY v_comment.pk_comment), '[]'::jsonb)
                       ) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
                LEFT JOIN v_comment ON v_comment.fk_post = tb_post.pk_post
                GROUP BY pk_post, fk_user, title, v_user.data
                $$
            )
        ").unwrap();

        // Verify metadata captured array dependency
        let meta = Spi::get_one::<String>("
            SELECT dependency_types::text FROM pg_tview_meta
            WHERE entity_name = 'post'
        ").unwrap().unwrap();
        assert!(meta.contains("array"), "Expected array dependency, got: {}", meta);

        // Initial state: 2 comments
        let initial_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();

        let initial_comments = initial_data.0["comments"].as_array().unwrap();
        assert_eq!(initial_comments.len(), 2, "Should have 2 comments initially");

        // Update one comment
        Spi::run("UPDATE tb_comment SET text = 'Updated!' WHERE pk_comment = 1").unwrap();

        // Trigger cascade
        let source_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tb_comment'::regclass::oid")
            .unwrap()
            .unwrap();

        crate::refresh::refresh_pk(source_oid, 1).unwrap();

        // Verify: Only the updated comment changed
        let updated_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();

        let comments = updated_data.0["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2, "Should still have 2 comments");

        // Find comments by their id field
        let comment_1 = comments.iter()
            .find(|c| c["id"].as_i64() == Some(1))
            .expect("Should find comment with id=1");

        let comment_2 = comments.iter()
            .find(|c| c["id"].as_i64() == Some(2))
            .expect("Should find comment with id=2");

        // This will FAIL with current full-replacement code
        assert_eq!(comment_1["text"], "Updated!", "Comment 1 should be updated");
        assert_eq!(comment_2["text"], "Thanks!", "Comment 2 should be unchanged");
    }

    /// Test smart patching for scalar dependencies.
    ///
    /// This test verifies that scalar FKs (not used in data column) are handled gracefully.
    ///
    /// Expected to PASS (scalar deps don't affect data column).
    #[pg_test]
    fn test_apply_patch_scalar() {
        // Setup: Create tables with FK but FK not used in SELECT
        Spi::run("CREATE TABLE tb_category (pk_category BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_category BIGINT REFERENCES tb_category(pk_category),
            title TEXT
        )").unwrap();

        Spi::run("INSERT INTO tb_category (pk_category, name) VALUES (1, 'Tech')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_category, title) VALUES (1, 1, 'Hello')").unwrap();

        // Create TVIEW where FK exists but not used in data
        Spi::run("
            SELECT pg_tviews_create(
                'post',
                $$
                SELECT pk_post, fk_category,
                       jsonb_build_object('title', title) AS data
                FROM tb_post
                $$
            )
        ").unwrap();

        // Verify metadata shows scalar dependency
        let meta = Spi::get_one::<String>("
            SELECT dependency_types::text FROM pg_tview_meta
            WHERE entity_name = 'post'
        ").unwrap().unwrap();
        assert!(meta.contains("scalar"), "Expected scalar dependency, got: {}", meta);

        // Initial state
        let initial_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();

        assert_eq!(initial_data.0["title"], "Hello");
        assert!(initial_data.0.get("category").is_none(), "Should not have category in data");

        // Update category (shouldn't affect tv_post.data since it's scalar)
        Spi::run("UPDATE tb_category SET name = 'Technology' WHERE pk_category = 1").unwrap();

        // Trigger cascade
        let source_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tb_category'::regclass::oid")
            .unwrap()
            .unwrap();

        crate::refresh::refresh_pk(source_oid, 1).unwrap();

        // Verify: data unchanged (scalar has no path in JSONB)
        let updated_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();

        assert_eq!(updated_data.0["title"], "Hello", "Title should be unchanged");
        assert!(updated_data.0.get("category").is_none(), "Still no category in data");
    }
}

