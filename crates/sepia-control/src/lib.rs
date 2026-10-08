//! sepia-control — the control plane: live-session ownership, lock
//! probing, attach/takeover, prompt/cancel/permission, restore/rewind.
//! Composes `sepia-core` (IR + planners), `sepia-acp` (live agents), and
//! `sepia-proto` (the event stream).

pub mod exec;
pub mod merged;
pub mod plane;
pub mod translate;
pub mod types;

pub use exec::DefaultRestoreExec;
pub use merged::{MergedRepository, agent_for_backend};
pub use plane::{ControlPlane, CreateResult};
pub use translate::Translator;
pub use types::*;
