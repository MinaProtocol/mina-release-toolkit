//! # Buildkite Cache Manager
//!
//! A CLI tool for managing Buildkite CI cache on Hetzner shared storage.
//!
//! ## Commands
//!
//! - **read**: Copy cached artifacts from Hetzner storage to local filesystem
//! - **write**: Copy local artifacts to Hetzner cache storage
//! - **list**: List files and folders in the cache, with optional `--debians` flag
//!   for debian-package-aware listing (codename/architecture structure)
//! - **prune**: Remove cache folders based on age, version, or timestamp
//!
//! ## Cache Structure
//!
//! The cache is organized under a base URL (default `/var/storagebox`):
//!
//! ```text
//! /var/storagebox/
//! ├── <buildkite-build-id>/          # UUID-formatted build IDs
//! │   ├── debians/
//! │   │   ├── <codename>/            # e.g., noble, focal, bullseye, jammy, bookworm
//! │   │   │   ├── amd64/
//! │   │   │   │   └── *.deb
//! │   │   │   ├── arm64/
//! │   │   │   │   └── *.deb
//! │   │   │   └── all/
//! │   │   │       └── *.deb
//! │   │   └── ...
//! │   └── <other-artifacts>/
//! ├── legacy/                        # Legacy artifacts (non-build-id)
//! │   └── ...
//! └── ...
//! ```
//!
//! ## Environment Variables
//!
//! - `BUILDKITE_BUILD_ID` - Required for read/write commands. Used as the default
//!   cache root folder.
//! - `CACHE_BASE_URL` - Override the cache mount point (default: `/var/storagebox`).
//!
//! ## Usage with LLM
//!
//! This tool is designed to be friendly for LLM-assisted workflows:
//! - All commands support `--format json` for structured output
//! - `--dry-run` on prune lets you preview changes safely
//! - Clear error messages with actionable hints
//!
//! ## Example
//!
//! ```bash
//! # List all cache folders
//! buildkite-cache-manager list
//!
//! # List debian packages in a build
//! buildkite-cache-manager list <build-id> --debians
//!
//! # Prune builds older than 30 days
//! buildkite-cache-manager prune --older-than 30d --dry-run
//!
//! # Keep only 5 latest builds by timestamp
//! buildkite-cache-manager prune --keep-latest-timestamp 5
//! ```

pub mod cache;
pub mod cli;
pub mod commands;
pub mod mock;
