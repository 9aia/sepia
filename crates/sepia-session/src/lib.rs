//! sepia-session — the local session plane: live-session ownership,
//! lock probing, attach/takeover, prompt/cancel/permission,
//! restore/rewind, plus [`local`], the composition that assembles it
//! from this machine's drivers and agent CLIs. Composes `sepia-core`
//! (IR + planners), `sepia-acp` (live agents), `sepia-host` (driver
//! binaries), `sepia-meta` (the overlay), and `sepia-proto` (the event
//! stream).

pub mod exec;
pub mod local;
pub mod merged;
pub mod plane;
pub mod translate;
pub mod types;

pub use exec::DefaultRestoreExec;
pub use local::{Local, NodeIdentity, Paths, base_options, build, sepia_home};
pub use merged::{MergedRepository, agent_for_backend};
pub use plane::{ControlPlane, CreateResult};
pub use sepia_meta::{MetaStore, Project, SessionMeta};
pub use translate::Translator;
pub use types::*;
