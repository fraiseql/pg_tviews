#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    // ── Unit tests for index set / storage (no database required) ──────────────

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
        let ddl = crate::ddl::create::indexes::tview_index_ddl(
            "tv_post",
            &post_schema(),
            "public",
            false,
        );
        assert_eq!(ddl.len(), 3, "{ddl:?}");
        for stmt in &ddl {
            assert!(!stmt.contains("\"data\""), "indexes data: {stmt}");
            assert!(!stmt.contains("updated_at"), "indexes updated_at: {stmt}");
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
        assert!(ddl.iter().all(|s| !s.contains("updated_at")));
    }

    #[test]
    fn test_storage_clause() {
        assert_eq!(
            crate::ddl::create::indexes::storage_clause(85),
            " WITH (fillfactor = 85)"
        );
        assert_eq!(crate::ddl::create::indexes::storage_clause(100), "");
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
            crate::ddl::create::indexes::propagation_index_ddl(
                "public", "tv_post", "fk_user", "pk_post"
            ),
            "CREATE INDEX IF NOT EXISTS \"idx_tv_post_fk_user_pk_post\" \
             ON \"public\".\"tv_post\" (\"fk_user\", \"pk_post\")"
        );
    }

    // ── Integration tests requiring database access ───────────────────────────

    #[test]
    fn test_tview_exists_non_existent() {
        // Compile-time check only — live DB tests use #[pg_test] below
    }

    /// TVIEW objects are created in the schema that is first in `search_path`,
    /// not hardcoded to public.
    #[pg_test]
    fn test_create_tview_respects_search_path() {
        Spi::run("CREATE SCHEMA tview_test_ns").unwrap();
        Spi::run("SET search_path TO tview_test_ns, public").unwrap();
        Spi::run("CREATE TABLE tb_item (pk_item BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_item VALUES (1, 'Widget')").unwrap();

        Spi::run(
            "SELECT pg_tviews_create('item', $$
            SELECT pk_item, jsonb_build_object('name', name) AS data
            FROM tb_item
        $$)",
        )
        .unwrap();

        // tv_item must be in the target schema
        let in_target = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_item' AND n.nspname = 'tview_test_ns'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(in_target, "tv_item should be in tview_test_ns, not public");

        // tv_item must NOT leak into public
        let in_public = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_item' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(!in_public, "tv_item must not be created in public schema");

        // The backing view v_item must be in the same schema
        let view_in_target = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'v_item' AND n.nspname = 'tview_test_ns'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(view_in_target, "v_item should be in tview_test_ns");
    }

    /// With the default `search_path`, objects still land in public (regression guard).
    #[pg_test]
    fn test_create_tview_defaults_to_public() {
        Spi::run("SET search_path TO public").unwrap();
        Spi::run("CREATE TABLE tb_gadget (pk_gadget BIGSERIAL PRIMARY KEY, label TEXT)").unwrap();
        Spi::run("INSERT INTO tb_gadget VALUES (1, 'Gizmo')").unwrap();

        Spi::run(
            "SELECT pg_tviews_create('gadget', $$
            SELECT pk_gadget, jsonb_build_object('label', label) AS data
            FROM tb_gadget
        $$)",
        )
        .unwrap();

        let in_public = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_gadget' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            in_public,
            "tv_gadget should be in public with default search_path"
        );
    }

    /// Test CTAS (CREATE TABLE AS SELECT) with preexisting data.
    /// This reproduces the bug where initial population fails.
    #[pg_test]
    fn test_ctas_with_preexisting_data() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_ctas_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_ctas_test VALUES (1, 'Alice'), (2, 'Bob')").unwrap();

        // Do CTAS - this should create a TVIEW with the existing data
        Spi::run(
            "CREATE TABLE tv_ctas_test AS
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_ctas_test",
        )
        .unwrap();

        // Check that TVIEW was created
        let tview_exists = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_ctas_test' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(tview_exists, "tv_ctas_test should exist");

        // Check that it has the initial data (this is where the bug manifests)
        let row_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_ctas_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            row_count, 2,
            "tv_ctas_test should have 2 rows from initial population"
        );

        // Check specific data
        let alice_exists = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM tv_ctas_test WHERE data->>'name' = 'Alice'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(alice_exists, "Alice should be in the TVIEW");
    }

    /// Test that TVIEW tables respect the `unlogged_by_default` GUC.
    #[pg_test]
    fn test_tview_unlogged_guc_control() {
        Spi::run("SET search_path TO public").unwrap();

        // Test with GUC set to true (default)
        Spi::run("SET pg_tviews.unlogged_by_default TO true").unwrap();

        // Create base table
        Spi::run("CREATE TABLE tb_guc_test1 (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();

        // Create TVIEW
        Spi::run(
            "SELECT pg_tviews_create('guc_test1', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_guc_test1
        $$)",
        )
        .unwrap();

        // Check that the TVIEW table is UNLOGGED
        let is_unlogged = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'u' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_guc_test1' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            is_unlogged,
            "tv_guc_test1 should be UNLOGGED when GUC is true"
        );

        // Test with GUC set to false
        Spi::run("SET pg_tviews.unlogged_by_default TO false").unwrap();

        // Create another base table
        Spi::run("CREATE TABLE tb_guc_test2 (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();

        // Create another TVIEW
        Spi::run(
            "SELECT pg_tviews_create('guc_test2', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_guc_test2
        $$)",
        )
        .unwrap();

        // Check that this TVIEW table is LOGGED
        let is_logged = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'p' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_guc_test2' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(is_logged, "tv_guc_test2 should be LOGGED when GUC is false");

        // Reset GUC to default
        Spi::run("RESET pg_tviews.unlogged_by_default").unwrap();
    }

    /// Test ALTER TABLE SET UNLOGGED/LOGGED on TVIEWs.
    #[pg_test]
    fn test_alter_tview_unlogged_logged() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_alter_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_alter_test VALUES (1, 'Alice'), (2, 'Bob')").unwrap();

        // Create TVIEW as LOGGED first
        Spi::run("SET pg_tviews.unlogged_by_default TO false").unwrap();
        Spi::run(
            "SELECT pg_tviews_create('alter_test', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_alter_test
        $$)",
        )
        .unwrap();

        // Verify TVIEW is initially LOGGED
        let is_logged = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'p' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_alter_test' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(is_logged, "tv_alter_test should initially be LOGGED");

        // ALTER TABLE to UNLOGGED
        Spi::run("ALTER TABLE tv_alter_test SET UNLOGGED").unwrap();

        // Verify it's now UNLOGGED
        let is_unlogged = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'u' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_alter_test' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            is_unlogged,
            "tv_alter_test should be UNLOGGED after ALTER TABLE"
        );

        // ALTER TABLE back to LOGGED
        Spi::run("ALTER TABLE tv_alter_test SET LOGGED").unwrap();

        // Verify it's now LOGGED again
        let is_logged_again = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'p' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_alter_test' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            is_logged_again,
            "tv_alter_test should be LOGGED again after ALTER TABLE"
        );

        // Reset GUC
        Spi::run("RESET pg_tviews.unlogged_by_default").unwrap();
    }

    /// Test data integrity during ALTER TABLE UNLOGGED/LOGGED operations.
    #[pg_test]
    fn test_alter_tview_data_integrity() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_integrity_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)")
            .unwrap();
        Spi::run("INSERT INTO tb_integrity_test VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Charlie')")
            .unwrap();

        // Create TVIEW as LOGGED
        Spi::run("SET pg_tviews.unlogged_by_default TO false").unwrap();
        Spi::run(
            "SELECT pg_tviews_create('integrity_test', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_integrity_test
        $$)",
        )
        .unwrap();

        // Verify data is present
        let initial_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_integrity_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(initial_count, 3, "TVIEW should have 3 rows initially");

        // ALTER TABLE from LOGGED to UNLOGGED - data should be preserved
        Spi::run("ALTER TABLE tv_integrity_test SET UNLOGGED").unwrap();

        let after_unlogged_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_integrity_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_unlogged_count, 3,
            "Data should be preserved when converting LOGGED to UNLOGGED"
        );

        // ALTER TABLE from UNLOGGED to LOGGED - data is preserved (PostgreSQL behavior)
        Spi::run("ALTER TABLE tv_integrity_test SET LOGGED").unwrap();

        let after_logged_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_integrity_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_logged_count, 3,
            "Data should be preserved when converting UNLOGGED to LOGGED"
        );

        // Reset GUC
        Spi::run("RESET pg_tviews.unlogged_by_default").unwrap();
    }

    /// Test detection of post-crash empty UNLOGGED table.
    #[pg_test]
    fn test_detect_post_crash_empty_tview() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_crash_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_crash_test VALUES (1, 'Alice'), (2, 'Bob')").unwrap();

        // Create TVIEW
        Spi::run(
            "SELECT pg_tviews_create('crash_test', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_crash_test
        $$)",
        )
        .unwrap();

        // Verify TVIEW has data
        let initial_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_crash_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(initial_count, 2, "TVIEW should have 2 rows initially");

        // Before truncation, should not detect crash
        let crash_before = crate::lifecycle::detect_post_crash_truncation("crash_test").unwrap();
        assert!(!crash_before, "Should not detect crash when table has data");

        // Simulate post-crash truncation (UNLOGGED table behavior)
        Spi::run("TRUNCATE TABLE tv_crash_test").unwrap();

        // Verify table is now empty
        let after_truncate_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_crash_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_truncate_count, 0,
            "TVIEW should be empty after truncate"
        );

        // Should now detect crash (table empty but view has data)
        let crash_detected = crate::lifecycle::detect_post_crash_truncation("crash_test").unwrap();
        assert!(
            crash_detected,
            "Should detect crash when UNLOGGED table is empty but view has data"
        );

        // Verify backing view still has data
        let view_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM v_crash_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            view_count, 2,
            "Backing view should still have data after table truncate"
        );
    }

    /// Test automatic recovery after crash detection.
    #[pg_test]
    fn test_auto_recover_after_crash() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_recover_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)")
            .unwrap();
        Spi::run("INSERT INTO tb_recover_test VALUES (1, 'Alice'), (2, 'Bob')").unwrap();

        // Create TVIEW
        Spi::run(
            "SELECT pg_tviews_create('recover_test', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_recover_test
        $$)",
        )
        .unwrap();

        // Verify TVIEW has data initially
        let initial_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_recover_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(initial_count, 2, "TVIEW should have 2 rows initially");

        // Simulate post-crash truncation
        Spi::run("TRUNCATE TABLE tv_recover_test").unwrap();

        // Verify table is now empty
        let after_truncate_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_recover_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_truncate_count, 0,
            "TVIEW should be empty after truncate"
        );

        // Call auto-recovery function
        let recovery_performed =
            Spi::get_one::<bool>("SELECT pg_tviews_recover_after_crash('recover_test')")
                .unwrap()
                .unwrap_or(false);
        assert!(
            recovery_performed,
            "Recovery should be performed when crash is detected"
        );

        // Verify TVIEW has data again after recovery
        let after_recovery_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_recover_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_recovery_count, 2,
            "TVIEW should have 2 rows after recovery"
        );

        // Call recovery again - should return false (no recovery needed)
        let second_recovery =
            Spi::get_one::<bool>("SELECT pg_tviews_recover_after_crash('recover_test')")
                .unwrap()
                .unwrap_or(true);
        assert!(
            !second_recovery,
            "Second recovery call should return false when no crash detected"
        );
    }
}
