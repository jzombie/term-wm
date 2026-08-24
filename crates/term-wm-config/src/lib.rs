#![doc = include_str!("../README.md")]

//! Central compile-time feature flags and runtime configuration for term-wm.
//!
//! This leaf crate (std-only, bottom of the dependency graph) is the single
//! home for:
//! - the `session-persistence` Cargo feature (declared in `Cargo.toml`)
//! - process-global runtime toggles that layer on top of that feature
//! - every `TERM_WM_*` environment variable constant
//! - the application identity constants backing IPC naming ([`APP_NAME`],
//!   [`GATEWAY_NAMESPACE`])
//!
//! Downstream crates (`term-wm-core`, `term-wm-pty-engine`, `term-session*`,
//! and the `term-wm` binary) depend downward on this crate without cycles.

pub mod env;
pub mod runtime;

pub use env::{
    APP_NAME, CHANNEL_ENV_VAR, ENVIRONMENT_ENV_VAR, ESC_TRACE_ENV, Environment, GATEWAY_NAMESPACE,
    NAMESPACE_ENV_VAR, NO_SESSION_PERSISTENCE_ENV_VAR, SESSION_GATEWAY_ENV_VAR, active_environment,
    default_environment, gateway_override, parse_environment, set_gateway_override,
};
pub use runtime::{RuntimeConfig, init, session_persistence_enabled};
