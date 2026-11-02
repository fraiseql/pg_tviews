//! The extension's own SQL: the catalog, the views and functions written in SQL,
//! and the event triggers, one file per part in `sql/install/`, in the order
//! their `requires` give. Plain SQL, so the read contract and the plan decoders
//! call no function of the library (#133).

use pgrx::prelude::*;

extension_sql_file!(
    "../sql/install/01-extension-schema.sql",
    name = "check_extension_schema",
    bootstrap
);

extension_sql_file!(
    "../sql/install/02-catalog.sql",
    name = "create_metadata_tables",
);

extension_sql_file!(
    "../sql/install/03-tview-reads.sql",
    name = "tview_reads",
    requires = ["create_metadata_tables"],
);

extension_sql_file!(
    "../sql/install/04-plan-decoder.sql",
    name = "plan_decoder",
    requires = ["create_metadata_tables"],
);

extension_sql_file!(
    "../sql/install/05-read-contract.sql",
    name = "read_contract",
    requires = ["create_metadata_tables", "tview_reads", "plan_decoder"],
);

extension_sql_file!(
    "../sql/install/06-event-triggers.sql",
    name = "event_triggers",
    requires = ["create_metadata_tables"],
    finalize
);

extension_sql_file!("../sql/install/07-audit.sql", name = "audit_table",);

extension_sql_file!(
    "../sql/install/08-profile.sql",
    name = "profile_function",
    requires = ["create_metadata_tables"]
);

extension_sql_file!(
    "../sql/install/09-reregister-all.sql",
    name = "reregister_all",
    requires = [
        pg_tviews_reregister,
        "create_metadata_tables",
        "tview_reads"
    ],
);
