//! Refresh Module: Smart JSONB Patching for Cascade Updates
//!
//! This module handles refreshing transformed views (TVIEWs) when underlying source
//! table rows change. It uses **smart JSONB patching** via the `jsonb_delta` extension
//! for 1.5-3× performance improvement on cascade updates.

pub mod array_ops;
pub mod main;

pub mod bulk;

// Re-export main functions for backward compatibility
pub use main::refresh_pk;
// Re-export DISTINCT ON refresh
pub use main::refresh_by_dedup_key;
// Re-export bulk functions
pub use bulk::refresh_bulk;
