//! Refresh Module: Smart JSONB Patching for Cascade Updates
//!
//! This module handles refreshing transformed views (TVIEWs) when underlying source
//! table rows change. It uses **smart JSONB patching** via the `jsonb_ivm` extension
//! for 1.5-3× performance improvement on cascade updates.

pub mod main;
pub mod array_ops;
pub mod batch;
pub mod bulk;
pub mod cache;

// Re-export main functions for backward compatibility
pub use main::refresh_pk;
// Re-export bulk functions
pub use bulk::refresh_bulk;
// Re-export cache functions
pub use cache::{register_cache_invalidation_callbacks, clear_prepared_statement_cache};