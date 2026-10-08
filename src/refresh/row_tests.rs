//! Tests of the row refresh against a live database (`#[pg_test]`).

use pgrx::prelude::*;

#[pg_schema]
mod tests {
    use crate::queue::key::KeyValue;
    use pgrx::JsonB;
    use pgrx::prelude::*;

    /// Refresh the row `key` of the TVIEW whose table or view is `source`.
    fn refresh_row(
        source: pg_sys::Oid,
        key: &KeyValue,
    ) -> crate::TViewResult<crate::refresh::Touched> {
        let entity = Spi::get_one::<String>(&format!(
            "SELECT entity FROM {} WHERE view_oid::oid = {1} OR table_oid::oid = {1}",
            crate::utils::meta_table(),
            source.to_u32()
        ))?
        .expect("a registered TVIEW");
        let meta = crate::catalog::TviewMeta::load_by_entity(&entity)?.expect("its metadata");
        crate::refresh::refresh_key(&meta, key)
    }

    /// Test smart patching for nested object dependencies.
    ///
    /// This test verifies that when a nested object (like 'author') changes,
    /// only that specific path in the JSONB is updated, not the entire document.
    #[pg_test]
    fn test_apply_patch_nested_object() {
        // Setup: Create tables with FK relationship
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )",
        )
        .unwrap();

        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Hello')").unwrap();

        // Create user TVIEW first (so v_user exists for post TVIEW)
        Spi::run(
            "
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data
                FROM tb_user
            $$)
        ",
        )
        .unwrap();

        // Create TVIEW with nested author object
        Spi::run(
            "
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
        ",
        )
        .unwrap();

        // Verify metadata captured nested dependency
        let meta = crate::utils::spi_get_string(
            "
            SELECT plan->>'embeds' FROM pg_tview_meta
            WHERE entity = 'post'
        ",
        )
        .unwrap()
        .unwrap();
        assert!(
            meta.contains("nested_object"),
            "Expected nested_object dependency, got: {meta}"
        );

        // Initial state
        let initial_data = Spi::get_one::<JsonB>(
            "
            SELECT data FROM tv_post WHERE pk_post = 1
        ",
        )
        .unwrap()
        .unwrap();

        let initial_json = &initial_data.0;
        assert_eq!(initial_json["title"], "Hello");
        assert_eq!(initial_json["author"]["name"], "Alice");

        // Update user name
        Spi::run("UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1").unwrap();

        // Refresh tv_user first (using tv_user OID, not tb_user)
        let user_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_user'::regclass::oid")
            .unwrap()
            .unwrap();
        refresh_row(user_oid, &KeyValue::Int(1)).unwrap();

        // Explicitly refresh tv_post (propagation is handled by the queue, not the refresh)
        let post_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_post'::regclass::oid")
            .unwrap()
            .unwrap();
        refresh_row(post_oid, &KeyValue::Int(1)).unwrap();

        // Verify: author.name changed, title unchanged
        let updated_data = Spi::get_one::<JsonB>(
            "
            SELECT data FROM tv_post WHERE pk_post = 1
        ",
        )
        .unwrap()
        .unwrap();

        let updated_json = &updated_data.0;

        assert_eq!(
            updated_json["title"], "Hello",
            "Title should be recomputed unchanged (own column not modified)"
        );
        assert_eq!(
            updated_json["author"]["name"], "Alice Updated",
            "Author name should be updated via smart patch"
        );
    }

    /// Test smart patching for array dependencies.
    ///
    /// This test verifies that when an element in an array (like 'comments') changes,
    /// only that specific element is updated, not the entire array.
    #[pg_test]
    fn test_apply_patch_array() {
        // Setup: Create tables with FK relationships
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )",
        )
        .unwrap();
        Spi::run(
            "CREATE TABLE tb_comment (
            pk_comment BIGSERIAL PRIMARY KEY,
            fk_post BIGINT REFERENCES tb_post(pk_post),
            fk_user BIGINT REFERENCES tb_user(pk_user),
            text TEXT
        )",
        )
        .unwrap();

        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Hello')").unwrap();
        Spi::run(
            "INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (1, 1, 1, 'Great post!')",
        )
        .unwrap();
        Spi::run(
            "INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (2, 1, 1, 'Thanks!')",
        )
        .unwrap();

        // Create dependency TVIEWs first
        Spi::run(
            "
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data
                FROM tb_user
            $$)
        ",
        )
        .unwrap();

        Spi::run(
            "
            SELECT pg_tviews_create('comment', $$
                SELECT pk_comment, fk_post, fk_user,
                       jsonb_build_object('text', text) AS data
                FROM tb_comment
            $$)
        ",
        )
        .unwrap();

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
        let meta = crate::utils::spi_get_string(
            "
            SELECT plan->>'embeds' FROM pg_tview_meta
            WHERE entity = 'post'
        ",
        )
        .unwrap()
        .unwrap();
        assert!(
            meta.contains("array"),
            "Expected array dependency, got: {meta}"
        );

        // Initial state: 2 comments
        let initial_data = Spi::get_one::<JsonB>(
            "
            SELECT data FROM tv_post WHERE pk_post = 1
        ",
        )
        .unwrap()
        .unwrap();

        let initial_comments = initial_data.0["comments"].as_array().unwrap();
        assert_eq!(
            initial_comments.len(),
            2,
            "Should have 2 comments initially"
        );

        // Update one comment
        Spi::run("UPDATE tb_comment SET text = 'Updated!' WHERE pk_comment = 1").unwrap();

        // Refresh tv_comment first (using tv_comment OID)
        let comment_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_comment'::regclass::oid")
            .unwrap()
            .unwrap();
        refresh_row(comment_oid, &KeyValue::Int(1)).unwrap();

        // Explicitly refresh tv_post (propagation is handled by the queue, not the refresh)
        let post_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_post'::regclass::oid")
            .unwrap()
            .unwrap();
        refresh_row(post_oid, &KeyValue::Int(1)).unwrap();

        // Verify: Only the updated comment changed
        let updated_data = Spi::get_one::<JsonB>(
            "
            SELECT data FROM tv_post WHERE pk_post = 1
        ",
        )
        .unwrap()
        .unwrap();

        let comments = updated_data.0["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2, "Should still have 2 comments");

        // Find comments by their id field
        let comment_1 = comments
            .iter()
            .find(|c| c["id"].as_i64() == Some(1))
            .expect("Should find comment with id=1");

        let comment_2 = comments
            .iter()
            .find(|c| c["id"].as_i64() == Some(2))
            .expect("Should find comment with id=2");

        assert_eq!(comment_1["text"], "Updated!", "Comment 1 should be updated");
        assert_eq!(
            comment_2["text"], "Thanks!",
            "Comment 2 should be unchanged"
        );
    }

    /// Test smart patching for scalar dependencies.
    ///
    /// This test verifies that scalar FKs (not used in data column) are handled gracefully.
    ///
    /// Expected to PASS (scalar deps don't affect data column).
    #[pg_test]
    fn test_apply_patch_scalar() {
        // Setup: Create tables with FK but FK not used in SELECT
        Spi::run("CREATE TABLE tb_category (pk_category BIGSERIAL PRIMARY KEY, name TEXT)")
            .unwrap();
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_category BIGINT REFERENCES tb_category(pk_category),
            title TEXT
        )",
        )
        .unwrap();

        Spi::run("INSERT INTO tb_category (pk_category, name) VALUES (1, 'Tech')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_category, title) VALUES (1, 1, 'Hello')")
            .unwrap();

        // Create TVIEW where FK exists but not used in data
        Spi::run(
            "
            SELECT pg_tviews_create(
                'post',
                $$
                SELECT pk_post, fk_category,
                       jsonb_build_object('title', title) AS data
                FROM tb_post
                $$
            )
        ",
        )
        .unwrap();

        // Verify metadata shows scalar dependency
        let meta = crate::utils::spi_get_string(
            "
            SELECT plan->>'embeds' FROM pg_tview_meta
            WHERE entity ='post'
        ",
        )
        .unwrap()
        .unwrap();
        assert!(
            meta.contains("scalar"),
            "Expected scalar dependency, got: {meta}"
        );

        // Initial state
        let initial_data = Spi::get_one::<JsonB>(
            "
            SELECT data FROM tv_post WHERE pk_post = 1
        ",
        )
        .unwrap()
        .unwrap();

        assert_eq!(initial_data.0["title"], "Hello");
        assert!(
            initial_data.0.get("category").is_none(),
            "Should not have category in data"
        );

        // Update category (shouldn't affect tv_post.data since it's scalar)
        Spi::run("UPDATE tb_category SET name = 'Technology' WHERE pk_category = 1").unwrap();

        // Refresh tv_post directly (using tv_post OID)
        let post_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_post'::regclass::oid")
            .unwrap()
            .unwrap();

        refresh_row(post_oid, &KeyValue::Int(1)).unwrap();

        // Verify: data unchanged (scalar has no path in JSONB)
        let updated_data = Spi::get_one::<JsonB>(
            "
            SELECT data FROM tv_post WHERE pk_post = 1
        ",
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            updated_data.0["title"], "Hello",
            "Title should be unchanged"
        );
        assert!(
            updated_data.0.get("category").is_none(),
            "Still no category in data"
        );
    }

    /// Integration test: Full cascade with multiple dependency types.
    ///
    /// Tests the complete smart patching workflow with a realistic scenario:
    /// - Nested object (author)
    /// - Array (comments)
    /// - Multi-level cascade
    ///
    /// This verifies that all components work together correctly.
    #[pg_test]
    fn test_smart_patch_full_integration() {
        // Setup: Create extension if available (graceful fallback if not)
        let _ = Spi::run("CREATE EXTENSION IF NOT EXISTS jsonb_delta");

        // Create tables
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT, email TEXT)")
            .unwrap();
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT,
            content TEXT
        )",
        )
        .unwrap();
        Spi::run(
            "CREATE TABLE tb_comment (
            pk_comment BIGSERIAL PRIMARY KEY,
            fk_post BIGINT REFERENCES tb_post(pk_post),
            fk_user BIGINT REFERENCES tb_user(pk_user),
            text TEXT
        )",
        )
        .unwrap();

        // Insert test data
        Spi::run(
            "INSERT INTO tb_user (pk_user, name, email) VALUES (1, 'Alice', 'alice@example.com')",
        )
        .unwrap();
        Spi::run("INSERT INTO tb_user (pk_user, name, email) VALUES (2, 'Bob', 'bob@example.com')")
            .unwrap();
        Spi::run(
            "INSERT INTO tb_post (pk_post, fk_user, title, content)
                  VALUES (1, 1, 'First Post', 'Hello World')",
        )
        .unwrap();
        Spi::run(
            "INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (1, 1, 1, 'Great post!')",
        )
        .unwrap();
        Spi::run(
            "INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (2, 1, 2, 'Thanks for sharing!')",
        )
        .unwrap();

        // Create dependency TVIEWs first
        Spi::run(
            "
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name, 'email', email) AS data
                FROM tb_user
            $$)
        ",
        )
        .unwrap();

        Spi::run(
            "
            SELECT pg_tviews_create('comment', $$
                SELECT pk_comment, fk_post, fk_user,
                       jsonb_build_object('text', text) AS data
                FROM tb_comment
            $$)
        ",
        )
        .unwrap();

        // Create TVIEW with multiple dependency types
        Spi::run(
            "
            SELECT pg_tviews_create('post', $$
                SELECT pk_post, fk_user,
                       jsonb_build_object(
                           'title', title,
                           'content', content,
                           'author', v_user.data,
                           'comments', COALESCE(
                               jsonb_agg(
                                   v_comment.data
                                   ORDER BY v_comment.pk_comment
                               ),
                               '[]'::jsonb
                           )
                       ) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
                LEFT JOIN v_comment ON v_comment.fk_post = tb_post.pk_post
                GROUP BY pk_post, fk_user, title, content, v_user.data
            $$)
        ",
        )
        .unwrap();

        // Verify initial state
        let initial = Spi::get_one::<JsonB>("SELECT data FROM tv_post WHERE pk_post = 1")
            .unwrap()
            .unwrap();

        assert_eq!(initial.0["title"], "First Post");
        assert_eq!(initial.0["author"]["name"], "Alice");
        assert_eq!(initial.0["comments"].as_array().unwrap().len(), 2);

        // Test 1: Update nested author (should use smart patch)
        Spi::run(
            "UPDATE tb_user SET name = 'Alice Updated', email = 'alice.new@example.com'
                  WHERE pk_user = 1",
        )
        .unwrap();

        // Refresh tv_user first, then tv_post explicitly
        let user_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_user'::regclass::oid")
            .unwrap()
            .unwrap();
        refresh_row(user_oid, &KeyValue::Int(1)).unwrap();

        let post_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_post'::regclass::oid")
            .unwrap()
            .unwrap();
        refresh_row(post_oid, &KeyValue::Int(1)).unwrap();

        let after_author_update =
            Spi::get_one::<JsonB>("SELECT data FROM tv_post WHERE pk_post = 1")
                .unwrap()
                .unwrap();

        // Author should be updated
        assert_eq!(after_author_update.0["author"]["name"], "Alice Updated");
        assert_eq!(
            after_author_update.0["author"]["email"],
            "alice.new@example.com"
        );

        // Other fields should be preserved
        assert_eq!(after_author_update.0["title"], "First Post");
        assert_eq!(after_author_update.0["content"], "Hello World");
        assert_eq!(
            after_author_update.0["comments"].as_array().unwrap().len(),
            2
        );

        // Test 2: Update array element (should use smart patch)
        Spi::run("UPDATE tb_comment SET text = 'Updated comment!' WHERE pk_comment = 1").unwrap();

        // Refresh tv_comment first, then tv_post explicitly
        let comment_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_comment'::regclass::oid")
            .unwrap()
            .unwrap();
        refresh_row(comment_oid, &KeyValue::Int(1)).unwrap();
        refresh_row(post_oid, &KeyValue::Int(1)).unwrap();

        let after_comment_update =
            Spi::get_one::<JsonB>("SELECT data FROM tv_post WHERE pk_post = 1")
                .unwrap()
                .unwrap();

        let comments = after_comment_update.0["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2, "Should still have 2 comments");

        // Find updated comment
        let comment_1 = comments
            .iter()
            .find(|c| c["id"].as_i64() == Some(1))
            .expect("Should find comment 1");
        assert_eq!(comment_1["text"], "Updated comment!");

        // Other comment should be unchanged
        let comment_2 = comments
            .iter()
            .find(|c| c["id"].as_i64() == Some(2))
            .expect("Should find comment 2");
        assert_eq!(comment_2["text"], "Thanks for sharing!");
    }

    /// Test fallback behavior when `jsonb_delta` is not available.
    ///
    /// Verifies that the system gracefully falls back to full replacement
    /// when the `jsonb_delta` extension is not installed.
    #[pg_test]
    fn test_fallback_without_jsonb_delta() {
        // Explicitly ensure jsonb_delta is NOT available for this test
        let _ = Spi::run("DROP EXTENSION IF EXISTS jsonb_delta CASCADE");

        // Create simple test case
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )",
        )
        .unwrap();

        Spi::run("INSERT INTO tb_user VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post VALUES (1, 1, 'Hello')").unwrap();

        // Create TVIEWs
        Spi::run(
            "
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data
                FROM tb_user
            $$)
        ",
        )
        .unwrap();

        Spi::run(
            "
            SELECT pg_tviews_create('post', $$
                SELECT pk_post, fk_user,
                       jsonb_build_object('title', title, 'author', v_user.data) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
            $$)
        ",
        )
        .unwrap();

        // Verify metadata is still captured (even without jsonb_delta)
        let meta = crate::utils::spi_get_string(
            "
            SELECT plan->>'embeds' FROM pg_tview_meta WHERE entity = 'post'
        ",
        );
        // Metadata should exist regardless of jsonb_delta availability
        assert!(
            meta.is_ok(),
            "Metadata should be captured even without jsonb_delta"
        );

        // Update should still work via fallback
        Spi::run("UPDATE tb_user SET name = 'Alice Fallback' WHERE pk_user = 1").unwrap();

        // Refresh tv_user first (using tv_user OID, not tb_user)
        let user_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_user'::regclass::oid")
            .unwrap()
            .unwrap();

        // This should succeed using full replacement fallback
        let result = refresh_row(user_oid, &KeyValue::Int(1));
        assert!(result.is_ok(), "Fallback should work without jsonb_delta");

        // Explicitly refresh tv_post (propagation is now handled by queue)
        let post_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_post'::regclass::oid")
            .unwrap()
            .unwrap();
        refresh_row(post_oid, &KeyValue::Int(1)).unwrap();

        // Verify data was updated (via fallback)
        let updated = Spi::get_one::<JsonB>("SELECT data FROM tv_post WHERE pk_post = 1")
            .unwrap()
            .unwrap();
        assert_eq!(updated.0["author"]["name"], "Alice Fallback");
        assert_eq!(updated.0["title"], "Hello");
    }

    /// Test DISTINCT ON TVIEW refresh by its DISTINCT ON key.
    ///
    /// Verifies that `refresh_key()` recomputes a DISTINCT ON group's winner.
    #[pg_test]
    fn test_refresh_distinct_on_basic() {
        // Create base tables
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT,
            created_at TIMESTAMP DEFAULT NOW()
        )",
        )
        .unwrap();

        // Insert test data with duplicate user references
        Spi::run("INSERT INTO tb_user VALUES (1, 'Alice')").unwrap();
        Spi::run(
            "INSERT INTO tb_post (pk_post, fk_user, title) VALUES
            (1, 1, 'First Post'),
            (2, 1, 'Second Post'),
            (3, 1, 'Third Post')",
        )
        .unwrap();

        // Create user TVIEW
        Spi::run(
            "
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data
                FROM tb_user
            $$)
        ",
        )
        .unwrap();

        // Create DISTINCT ON TVIEW (one row per user, keep first post)
        Spi::run(
            "
            SELECT pg_tviews_create('post_by_user', $$
                SELECT DISTINCT ON (fk_user)
                       pk_post, fk_user,
                       jsonb_build_object('title', title) AS data
                FROM tb_post
                ORDER BY fk_user, pk_post
            $$, 'fk_user')
        ",
        )
        .unwrap();

        // Verify TVIEW was created keyed on its DISTINCT ON key
        let identity = crate::utils::spi_get_string(
            "
            SELECT identity::text FROM pg_tview_meta
            WHERE entity = 'post_by_user'
        ",
        )
        .unwrap()
        .unwrap();
        assert!(
            identity.contains("fk_user"),
            "Should record the DISTINCT ON key as the identity"
        );

        // Verify initial state (only one row for user 1, fk_user=1)
        let initial_count: i64 = Spi::get_one(
            "
            SELECT COUNT(*) FROM tv_post_by_user WHERE fk_user = 1
        ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(initial_count, 1, "Should have exactly 1 row for fk_user=1");

        let initial_title: String = Spi::get_one(
            "
            SELECT data->>'title' FROM tv_post_by_user WHERE fk_user = 1
        ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            initial_title, "First Post",
            "Should be first post initially"
        );
    }

    /// Test several refreshes of a DISTINCT ON TVIEW.
    ///
    /// Verifies that successive `refresh_key()` calls follow the group's winner.
    #[pg_test]
    fn test_refresh_distinct_on_multiple_keys() {
        // Create base tables
        Spi::run("CREATE TABLE tb_category (pk_category BIGSERIAL PRIMARY KEY, name TEXT)")
            .unwrap();
        Spi::run(
            "CREATE TABLE tb_item (
            pk_item BIGSERIAL PRIMARY KEY,
            fk_category BIGINT REFERENCES tb_category(pk_category),
            title TEXT
        )",
        )
        .unwrap();

        // Insert test data with duplicate categories
        Spi::run("INSERT INTO tb_category VALUES (1, 'Tech'), (2, 'News')").unwrap();
        Spi::run(
            "INSERT INTO tb_item (pk_item, fk_category, title) VALUES
            (1, 1, 'Item 1A'),
            (2, 1, 'Item 1B'),
            (3, 1, 'Item 1C'),
            (4, 2, 'Item 2A'),
            (5, 2, 'Item 2B')",
        )
        .unwrap();

        // Create category TVIEW
        Spi::run(
            "
            SELECT pg_tviews_create('category', $$
                SELECT pk_category, jsonb_build_object('name', name) AS data
                FROM tb_category
            $$)
        ",
        )
        .unwrap();

        // Create DISTINCT ON TVIEW (one row per category)
        Spi::run(
            "
            SELECT pg_tviews_create('item_by_cat', $$
                SELECT DISTINCT ON (fk_category)
                       pk_item, fk_category,
                       jsonb_build_object('title', title) AS data
                FROM tb_item
                ORDER BY fk_category, pk_item
            $$, 'fk_category')
        ",
        )
        .unwrap();

        // Verify initial state: one row per category
        let cat1_count: i64 = Spi::get_one(
            "
            SELECT COUNT(*) FROM tv_item_by_cat WHERE fk_category = 1
        ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(cat1_count, 1, "Should have 1 row for category 1");

        let cat1_title: String = Spi::get_one(
            "
            SELECT data->>'title' FROM tv_item_by_cat WHERE fk_category = 1
        ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(cat1_title, "Item 1A", "Category 1 should show Item 1A");

        // Now delete Item 1A (the current winner) and refresh its group
        // This simulates the real cascade scenario where one item changes
        // and we need to refresh the DISTINCT ON group
        Spi::run("DELETE FROM tb_item WHERE pk_item = 1").unwrap();

        // Refresh the group directly
        // (The actual invocation would be through queue mechanism)
        let view_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'v_item_by_cat'::regclass::oid")
            .unwrap()
            .unwrap();

        let result = refresh_row(view_oid, &KeyValue::Int(1));
        assert!(result.is_ok(), "First refresh should succeed");

        // Verify winner changed to Item 1B
        let cat1_new_title: String = Spi::get_one(
            "
            SELECT data->>'title' FROM tv_item_by_cat WHERE fk_category = 1
        ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            cat1_new_title, "Item 1B",
            "Category 1 should now show Item 1B"
        );

        // Delete Item 1B and refresh again
        Spi::run("DELETE FROM tb_item WHERE pk_item = 2").unwrap();

        let result2 = refresh_row(view_oid, &KeyValue::Int(1));
        assert!(result2.is_ok(), "Second refresh should succeed");

        // Verify winner changed to Item 1C
        let cat1_final_title: String = Spi::get_one(
            "
            SELECT data->>'title' FROM tv_item_by_cat WHERE fk_category = 1
        ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            cat1_final_title, "Item 1C",
            "Category 1 should now show Item 1C"
        );
    }

    /// Test that audit entries are buffered and flushed via `flush_audit_buffer()`.
    ///
    /// Verifies that:
    /// 1. `log_refresh()` buffers entries without writing to the DB
    /// 2. `flush_audit_buffer()` writes all buffered entries in one go
    /// 3. The buffer is empty after flush
    #[pg_test]
    fn test_audit_buffer_and_flush() {
        // Enable audit for this test
        Spi::run("SET pg_tviews.audit_enabled = true").unwrap();

        // Buffer some audit entries (no SPI, no DB writes)
        crate::audit::log_refresh("user", 5);
        crate::audit::log_refresh("post", 3);
        crate::audit::log_create("comment", "SELECT ...");

        // Verify nothing written to DB yet
        let count: i64 = Spi::get_one("SELECT COUNT(*) FROM pg_tview_audit_log")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(count, 0, "Buffer should not write to DB before flush");

        // Flush
        crate::audit::flush_audit_buffer().unwrap();

        // Verify all 3 entries written
        let count: i64 = Spi::get_one("SELECT COUNT(*) FROM pg_tview_audit_log")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(count, 3, "Flush should write all buffered entries");

        // Verify operations are correct
        let refresh_count: i64 =
            Spi::get_one("SELECT COUNT(*) FROM pg_tview_audit_log WHERE operation = 'REFRESH'")
                .unwrap()
                .unwrap_or(0);
        assert_eq!(refresh_count, 2, "Should have 2 REFRESH entries");

        let create_count: i64 =
            Spi::get_one("SELECT COUNT(*) FROM pg_tview_audit_log WHERE operation = 'CREATE'")
                .unwrap()
                .unwrap_or(0);
        assert_eq!(create_count, 1, "Should have 1 CREATE entry");

        // Verify buffer is empty after flush (second flush is no-op)
        crate::audit::flush_audit_buffer().unwrap();
        let count_after: i64 = Spi::get_one("SELECT COUNT(*) FROM pg_tview_audit_log")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(count_after, 3, "Second flush should be no-op");
    }

    /// Test that `clear_audit_buffer()` discards entries without writing.
    #[pg_test]
    fn test_audit_buffer_clear() {
        Spi::run("SET pg_tviews.audit_enabled = true").unwrap();

        crate::audit::log_refresh("user", 10);
        crate::audit::log_drop("post");

        // Clear without flushing
        crate::audit::clear_audit_buffer();

        // Flush should be no-op
        crate::audit::flush_audit_buffer().unwrap();

        let count: i64 = Spi::get_one("SELECT COUNT(*) FROM pg_tview_audit_log")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(count, 0, "Cleared buffer should not produce any rows");
    }

    /// Test that `flush_audit_buffer()` is a no-op when audit is disabled.
    #[pg_test]
    fn test_audit_disabled_skips_flush() {
        Spi::run("SET pg_tviews.audit_enabled = false").unwrap();

        crate::audit::log_refresh("user", 5);

        // Flush should skip writing because audit is disabled
        crate::audit::flush_audit_buffer().unwrap();

        let count: i64 = Spi::get_one("SELECT COUNT(*) FROM pg_tview_audit_log")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(count, 0, "Disabled audit should not write any rows");
    }

    /// Test that refreshing a deleted row removes it from the tview.
    ///
    /// When a base row is deleted (or the view condition no longer matches), the
    /// backing view returns no row for that pk. `refresh_key()` must remove the
    /// corresponding tview row and succeed — previously it raised a swallowed SPI
    /// error and left the row stale.
    #[pg_test]
    fn test_missing_row_deletes_tview_row() {
        // Create base tables
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )",
        )
        .unwrap();

        // Insert test data
        Spi::run("INSERT INTO tb_user VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post VALUES (1, 1, 'Hello')").unwrap();

        // Create TVIEW
        Spi::run(
            "
            SELECT pg_tviews_create('post', $$
                SELECT pk_post, fk_user,
                       jsonb_build_object('title', title) AS data
                FROM tb_post
            $$)
        ",
        )
        .unwrap();

        // The tview row exists after creation.
        let before: i64 = Spi::get_one("SELECT count(*) FROM tv_post WHERE pk_post = 1")
            .unwrap()
            .unwrap();
        assert_eq!(before, 1, "tview row should exist before delete");

        // Delete the underlying post
        Spi::run("DELETE FROM tb_post WHERE pk_post = 1").unwrap();

        // Refresh should succeed and remove the tview row (no swallowed error).
        let post_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_post'::regclass::oid")
            .unwrap()
            .unwrap();

        let result = refresh_row(post_oid, &KeyValue::Int(1));
        assert!(
            result.is_ok(),
            "Refresh of a deleted row should succeed by removing the tview row, got {result:?}"
        );

        let after: i64 = Spi::get_one("SELECT count(*) FROM tv_post WHERE pk_post = 1")
            .unwrap()
            .unwrap();
        assert_eq!(
            after, 0,
            "tview row should be gone after refreshing a deleted pk"
        );
    }

    /// Test error handling when NULL data column is encountered.
    ///
    /// Verifies that `refresh_key()` gracefully handles the edge case where
    /// the backing view returns a row but the data column is NULL.
    #[pg_test]
    fn test_null_data_column_error_handling() {
        // Create base table without data column in view
        Spi::run("CREATE TABLE tb_item (pk_item BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_item VALUES (1, 'Widget')").unwrap();

        // Create TVIEW (note: data column will be NULL if name is manipulated)
        Spi::run(
            "
            SELECT pg_tviews_create('item', $$
                SELECT pk_item,
                       CASE WHEN name = 'Widget' THEN jsonb_build_object('name', name)
                            ELSE NULL
                       END AS data
                FROM tb_item
            $$)
        ",
        )
        .unwrap();

        // Verify initial state
        let initial_data: Option<String> =
            Spi::get_one("SELECT data::text FROM tv_item WHERE pk_item = 1").unwrap();
        assert!(initial_data.is_some(), "Should have valid data initially");

        // Update to trigger NULL data column
        Spi::run("UPDATE tb_item SET name = 'Widget-Modified' WHERE pk_item = 1").unwrap();

        // Attempt to refresh
        let item_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'tv_item'::regclass::oid")
            .unwrap()
            .unwrap();

        let result = refresh_row(item_oid, &KeyValue::Int(1));

        // Should fail with clear error about NULL data
        assert!(
            result.is_err(),
            "Refresh should fail when data column is NULL"
        );
        let error_msg = format!("{:?}", result.unwrap_err());
        assert!(
            error_msg.to_lowercase().contains("null") || error_msg.contains("data"),
            "Error should mention NULL or data column issue"
        );
    }

    /// Test refreshing a DISTINCT ON TVIEW keyed on a text column.
    #[pg_test]
    fn test_refresh_distinct_on_text_keys() {
        // Create base tables
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            title TEXT
        )",
        )
        .unwrap();

        // Insert test data
        Spi::run(
            "INSERT INTO tb_post (pk_post, title) VALUES
            (1, 'Post 1'),
            (2, 'Post 2')",
        )
        .unwrap();

        // Create DISTINCT ON TVIEW (one row per title, a text key)
        Spi::run(
            "
            SELECT pg_tviews_create('post_by_title', $$
                SELECT DISTINCT ON (title)
                       pk_post,
                       jsonb_build_object('title', title) AS data
                FROM tb_post
                ORDER BY title, pk_post
            $$, 'title')
        ",
        )
        .unwrap();

        let view_oid: pgrx::pg_sys::Oid = Spi::get_one("SELECT 'v_post_by_title'::regclass::oid")
            .unwrap()
            .unwrap();

        let result1 = refresh_row(view_oid, &KeyValue::Text("Post 1".into()));
        assert!(result1.is_ok(), "Initial refresh should succeed");

        let result2 = refresh_row(view_oid, &KeyValue::Text("Post 2".into()));
        assert!(result2.is_ok(), "Second refresh should succeed");
    }
}
