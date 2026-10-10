//! Browser-local notification preferences. The node's push store
//! honors `{prefs:{done,permission}}` on `POST /api/push/subscribe`,
//! but there's no read-back endpoint — the UI keeps its copy in
//! localStorage under [`STORAGE_KEY`] and re-posts the subscription
//! when a toggle flips.

use serde::{Deserialize, Serialize};

/// localStorage key for the serialized [`NotifyPrefs`].
pub const STORAGE_KEY: &str = "sepia-notify-prefs";

/// Which notification kinds this browser's subscription wants. The
/// shape mirrors `sepia_push::PushPrefs` (`sepia-web` can't depend on
/// it — the DTO copy lives in `sepia-web::dto`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyPrefs {
    /// Agent run finished or errored.
    pub done: bool,
    /// Agent is waiting on a permission decision.
    pub permission: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        Self {
            done: true,
            permission: true,
        }
    }
}

impl NotifyPrefs {
    /// Parse the stored blob — missing keys and malformed JSON both
    /// fall back to all-on (the node-side default).
    pub fn from_stored(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_default()
    }

    /// The serialized form for localStorage.
    pub fn stored(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_all_kinds_on() {
        let p = NotifyPrefs::default();
        assert!(p.done && p.permission);
        // Empty/garbage storage reads as defaults too.
        assert_eq!(NotifyPrefs::from_stored(""), NotifyPrefs::default());
        assert_eq!(NotifyPrefs::from_stored("{}"), NotifyPrefs::default());
        assert_eq!(NotifyPrefs::from_stored("not json"), NotifyPrefs::default());
    }

    #[test]
    fn partial_blob_fills_defaults() {
        let p = NotifyPrefs::from_stored(r#"{"done":false}"#);
        assert!(!p.done && p.permission);
    }

    #[test]
    fn round_trips() {
        let p = NotifyPrefs {
            done: false,
            permission: true,
        };
        assert_eq!(NotifyPrefs::from_stored(&p.stored()), p);
    }
}
