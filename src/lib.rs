//! AnyRouter native CLI library. Shared by the `anyr` binary and tests.

pub mod api;
pub mod artifact;
pub mod auth;
pub mod autoupdate;
pub mod buildinfo;
pub mod channel;
#[cfg(feature = "native")]
pub mod chat;
pub mod civil;
pub mod cmd;
pub mod commands;
pub mod completion;
pub mod config;
pub mod counters;
pub mod crash;
pub mod demo;
pub mod exit;
pub mod help;
pub mod http;
pub mod install;
pub mod key;
pub(crate) mod launch_cache;
pub(crate) mod mcp_wiring;
pub mod onboard;
pub mod parse;
pub mod publish;
pub mod registry;
#[cfg(feature = "native")]
pub mod relay;
pub mod report;
pub mod schema;
pub mod sort;
pub mod spawn;
pub mod spinner;
pub mod telemetry;
pub mod term;
#[cfg(feature = "native")]
pub mod tui;
pub mod upgrade;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

pub use commands::run;
pub use config::{parse_config, serialize_config};
pub use parse::parse_cli_args;
pub use spawn::{build_tool_env, redact_value, render_dry_run};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
