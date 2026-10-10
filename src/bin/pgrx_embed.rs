//! The binary `cargo pgrx` runs to generate the extension's SQL.
#![allow(missing_docs)] // Reason: pgrx_embed! expands to an undocumented `main`

::pgrx::pgrx_embed!();
