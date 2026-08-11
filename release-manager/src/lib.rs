//! Library surface of the release manager.
//!
//! The binary in `main.rs` is a thin dispatcher over these modules. The modules
//! are public so sibling tools in this repo can reuse them instead of copying
//! logic that must not drift — in particular `artifacts`, which owns the
//! package-name, version and docker-tag conventions of the Mina release
//! process.

pub mod artifacts;
pub mod cli;
pub mod commands;
pub mod debian_publish;
pub mod docker_promote;
pub mod errors;
pub mod process;
pub mod reversion;
pub mod storage;
pub mod utils;
pub mod verification;
