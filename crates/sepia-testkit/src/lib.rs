#![allow(clippy::unwrap_used)]

//! sepia-testkit — the behavioral backbone: conformance suites run against
//! any implementation, golden fixtures extracted from the TS adapters, and
//! a numeric-tolerant JSON equality for cross-language comparisons.

pub mod contract;
pub mod fixtures;
pub mod json_eq;

pub use contract::assert_session_repository_contract;
pub use fixtures::{fixture_dir, load_expected, materialize_store};
pub use json_eq::{assert_json_eq, json_eq};
