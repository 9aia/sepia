//! Pages — one file per route.

mod agents;
mod login;
mod nodes;
mod projects;
mod session_detail;
mod session_list;
mod settings;

pub use agents::AgentsPage;
pub use login::LoginPage;
pub use nodes::NodesPage;
pub use projects::ProjectsPage;
pub use session_detail::{SessionDetailPage, SessionPanel};
pub use session_list::{RelativeTime, SessionListPage};
pub use settings::SettingsPage;
