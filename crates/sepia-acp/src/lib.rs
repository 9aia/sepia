//! sepia-acp — spawn ACP agents over stdio; typed session ops + tolerant
//! normalized updates. `serde_json::Value` at the wire boundary: real
//! agents emit non-conformant payloads the strict SDK would reject.

pub mod blocking;
pub mod broker;
pub mod conn;
pub mod normalize;
pub mod registry;
pub mod stderr;
pub mod types;

pub use blocking::{Client, ClientOptions, fs_read_text_file_handler};
pub use broker::PermissionBroker;
pub use conn::{AcpConnection, spawn_agent};
pub use normalize::{normalize_permission, normalize_update};
pub use registry::{builtin_agents, model_args, resolve_agent};
pub use sepia_driver::rpc;
pub use stderr::StderrTail;
pub use types::*;
