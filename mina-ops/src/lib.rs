//! `mina-ops` — a read-only view across the systems a Mina release touches.
//!
//! Buildkite, the Debian repositories and the Docker registries each answer
//! only for themselves. This crate joins them on the commit, which is the one
//! identifier they share, and reports what exists where.
//!
//! It reads. It does not publish, promote or delete: that is
//! `release-manager`'s work, and the package-naming conventions are imported
//! from it rather than copied.

pub mod adapters;
pub mod config;
pub mod error;
pub mod git;
pub mod hardfork;
pub mod inventory;
pub mod mcp;
pub mod model;
pub mod nightly;
pub mod report;
pub mod serve;
