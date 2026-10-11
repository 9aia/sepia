//! The desktop aim — which node/agent/model/directory "New session"
//! points at. The pure half of `sepia-web`'s rail footer
//! (`desktop::DesktopMenu`): state, the node/agent pick semantics,
//! and the menu's label/reachability math. Nothing here touches the
//! DOM; the same code compiles for SSR and wasm. Persisted in
//! localStorage under [`STORAGE_KEY`] as JSON.
//!
//! Port of the old app's `settings.desktop` / `lib/focus.ts`: one
//! persisted concept the footer's menu and the create form share —
//! an unset field falls back (node → the hub's primary, agent → the
//! node's default, model → the agent default, cwd → the node's most
//! recent session dir).

use serde::{Deserialize, Serialize};

/// localStorage key for the serialized [`DesktopAim`].
pub const STORAGE_KEY: &str = "sepia-desktop";

/// How many directories the menu's Directory submenu lists — the old
/// ClientBar sliced the node's recent cwds at six.
pub const RECENT_DIR_LIMIT: usize = 6;

/// Where the next create lands. Every field is optional: `node`
/// `None` = the hub's primary node, `agent` `None` = the node's own
/// pick ("Node default"), `model` `None` = the agent's configured
/// default ("Agent default"), `cwd` `None` = the node's most recent
/// session dir ("Most recent").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopAim {
    /// Node id, or `None` for the hub's primary.
    pub node: Option<String>,
    /// Agent id on `node`, or `None` for the node's default.
    pub agent: Option<String>,
    /// Model override, or `None` for the agent default.
    pub model: Option<String>,
    /// Working directory, or `None` for the node's most recent.
    pub cwd: Option<String>,
}

impl DesktopAim {
    /// True once any field is picked — gates the menu's
    /// "Reset desktop" item (the old `desktopSet`).
    pub fn is_set(&self) -> bool {
        self.node.is_some() || self.agent.is_some() || self.model.is_some() || self.cwd.is_some()
    }

    /// Back to the unscoped aim — primary node, node pick, recents
    /// (the old `clearFocus`).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// A node/agent pick — the old ClientBar's `pick(node, agent)`:
    /// scoped fields don't port — a node change drops the cwd (dirs
    /// are node-local), and an agent change drops a model picked for
    /// the previous agent.
    pub fn pick(&mut self, node: Option<String>, agent: Option<String>) {
        let same_node = self.node == node;
        let same_agent = self.agent == agent;
        self.node = node;
        self.agent = agent;
        if !same_node {
            self.cwd = None;
        }
        if !same_node || !same_agent {
            self.model = None;
        }
    }

    /// The new-session form's agent-select value for this aim —
    /// `node|agent`, `node|` for a node pick on the node default, a
    /// bare `agent` when no node is pinned, `""` when neither is.
    /// `create` splits on `|` the same way.
    pub fn select_value(&self) -> String {
        match (&self.node, &self.agent) {
            (Some(n), Some(a)) => format!("{n}|{a}"),
            (Some(n), None) => format!("{n}|"),
            (None, Some(a)) => a.clone(),
            (None, None) => String::new(),
        }
    }

    /// Parse the stored blob — missing keys and malformed JSON both
    /// read as the unscoped aim (same tolerance as `NotifyPrefs`).
    pub fn from_stored(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_default()
    }

    /// The serialized form for localStorage.
    pub fn stored(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// The node a wire annotation belongs to. Agents/sessions carry
/// `node: None` for the primary node (and always on single-node
/// hubs), so the effective id falls back to `primary` — the hub's
/// `GET /api/node` id.
pub fn effective_node(annotation: &Option<String>, primary: &Option<String>) -> Option<String> {
    annotation.clone().or_else(|| primary.clone())
}

/// The focused node's known directories — distinct session cwds in
/// first-seen order (the merged list arrives newest-first, so
/// first-seen IS recency), capped at `limit`. Rows are `(cwd, node
/// annotation)` pairs; `node` is the RESOLVED focused id — callers
/// pass `effective_node(&aim.node, &primary)` so an unset aim lands
/// in the primary's bucket like every bare annotation does.
pub fn recent_cwds<'a>(
    rows: impl Iterator<Item = (&'a str, &'a Option<String>)>,
    node: &Option<String>,
    primary: &Option<String>,
    limit: usize,
) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    rows.filter(|(cwd, _)| !cwd.is_empty())
        .filter(|(_, n)| effective_node(n, primary) == *node)
        .filter(|(cwd, _)| seen.insert((*cwd).to_string()))
        .take(limit)
        .map(|(cwd, _)| cwd.to_string())
        .collect()
}

/// Last two path segments (`9aia/sepia`) — the menu's compact dir
/// label (the old `dirLabel`; the row keeps the full path in `title`).
pub fn dir_label(path: &str) -> String {
    let parts: Vec<&str> = path
        .trim_end_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        path.to_string()
    } else {
        parts[parts.len().saturating_sub(2)..].join("/")
    }
}

/// The footer dot's reachability state — also drives the trigger's
/// summary line. The old ClientBar had a fourth derived state
/// ("No nodes connected") — that's `Offline` here, and the caller
/// swaps the label when `any_up` is false.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DotState {
    /// The focused node is reachable.
    Online,
    /// The focused node is down/missing — or every node is.
    Offline,
    /// The status probe hasn't landed (or the node reports
    /// `"unknown"`) — the summary skeletons/dims rather than
    /// flashing a stale answer.
    Checking,
}

/// Resolve the dot. `loaded` = the node-status query resolved at all;
/// `focused` = the focused node's status string (`"up" | "down" |
/// "unknown"`), `None` when it isn't in the registry (a stale or
/// disabled pick → offline); `any_up` = at least one registered node
/// is reachable (`false` → the "No nodes connected" state).
pub fn dot_state(loaded: bool, any_up: bool, focused: Option<&str>) -> DotState {
    if !loaded {
        return DotState::Checking;
    }
    if !any_up {
        return DotState::Offline;
    }
    match focused {
        Some("up") => DotState::Online,
        Some("unknown") => DotState::Checking,
        _ => DotState::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aim(node: Option<&str>, agent: Option<&str>) -> DesktopAim {
        DesktopAim {
            node: node.map(str::to_string),
            agent: agent.map(str::to_string),
            model: Some("m1".to_string()),
            cwd: Some("/w/app".to_string()),
        }
    }

    #[test]
    fn defaults_are_unscoped() {
        let a = DesktopAim::default();
        assert!(!a.is_set());
        assert_eq!(a.select_value(), "");
    }

    #[test]
    fn stored_blob_round_trips_and_tolerates_garbage() {
        let a = aim(Some("tower"), Some("claude"));
        assert_eq!(DesktopAim::from_stored(&a.stored()), a);
        assert_eq!(DesktopAim::from_stored(""), DesktopAim::default());
        assert_eq!(DesktopAim::from_stored("not json"), DesktopAim::default());
        // Partial blobs default the rest.
        let p = DesktopAim::from_stored(r#"{"agent":"devin"}"#);
        assert_eq!(p.agent.as_deref(), Some("devin"));
        assert_eq!(p.node, None);
    }

    #[test]
    fn select_value_encodes_node_and_agent() {
        assert_eq!(aim(Some("n1"), Some("devin")).select_value(), "n1|devin");
        assert_eq!(aim(Some("n1"), None).select_value(), "n1|");
        assert_eq!(aim(None, Some("devin")).select_value(), "devin");
        assert_eq!(aim(None, None).select_value(), "");
    }

    #[test]
    fn a_node_change_drops_the_scoped_fields() {
        let mut a = aim(Some("n1"), Some("devin"));
        a.pick(Some("tower".to_string()), None);
        assert_eq!(a.node.as_deref(), Some("tower"));
        assert_eq!(a.agent, None);
        assert_eq!(a.cwd, None, "dirs are node-local");
        assert_eq!(a.model, None, "the model was the old node's agent's");
    }

    #[test]
    fn an_agent_change_keeps_cwd_but_drops_model() {
        let mut a = aim(Some("n1"), Some("devin"));
        a.pick(Some("n1".to_string()), Some("claude".to_string()));
        assert_eq!(a.cwd.as_deref(), Some("/w/app"));
        assert_eq!(a.model, None);
    }

    #[test]
    fn repicking_the_same_agent_keeps_everything() {
        let mut a = aim(Some("n1"), Some("devin"));
        a.pick(Some("n1".to_string()), Some("devin".to_string()));
        assert_eq!(a.model.as_deref(), Some("m1"));
        assert_eq!(a.cwd.as_deref(), Some("/w/app"));
    }

    #[test]
    fn clear_resets_the_aim() {
        let mut a = aim(Some("n1"), Some("devin"));
        a.clear();
        assert!(!a.is_set());
    }

    #[test]
    fn effective_node_falls_back_to_the_primary() {
        let primary = Some("n1".to_string());
        assert_eq!(
            effective_node(&Some("tower".to_string()), &primary).as_deref(),
            Some("tower")
        );
        assert_eq!(effective_node(&None, &primary).as_deref(), Some("n1"));
        assert_eq!(effective_node(&None, &None), None);
    }

    #[test]
    fn recent_cwds_are_first_seen_distinct_and_node_scoped() {
        let rows = vec![
            ("/w/a", Some("n1".to_string())),
            ("/w/b", Some("n1".to_string())),
            ("/w/a", Some("n1".to_string())),
            ("/remote/x", Some("tower".to_string())),
            ("", Some("n1".to_string())),
            // Bare annotation → the primary's bucket.
            ("/local/only", None),
        ];
        let primary = Some("n1".to_string());
        // Focused node n1 — bare rows land here, tower's don't.
        assert_eq!(
            recent_cwds(
                rows.iter().map(|(c, n)| (*c, n)),
                &Some("n1".to_string()),
                &primary,
                6
            ),
            vec!["/w/a", "/w/b", "/local/only"]
        );
        // Focused node tower — only its own.
        assert_eq!(
            recent_cwds(
                rows.iter().map(|(c, n)| (*c, n)),
                &Some("tower".to_string()),
                &primary,
                6
            ),
            vec!["/remote/x"]
        );
        // The cap slices at `limit`.
        assert_eq!(
            recent_cwds(
                rows.iter().map(|(c, n)| (*c, n)),
                &Some("n1".to_string()),
                &primary,
                1
            ),
            vec!["/w/a"]
        );
    }

    #[test]
    fn dir_label_keeps_the_last_two_segments() {
        assert_eq!(dir_label("/home/u/9aia/sepia"), "9aia/sepia");
        assert_eq!(dir_label("/work"), "work");
        assert_eq!(dir_label("/"), "/");
        assert_eq!(dir_label("/a/b/"), "a/b");
    }

    #[test]
    fn the_dot_tracks_probe_state() {
        assert_eq!(dot_state(false, false, None), DotState::Checking);
        // Nothing reachable at all → offline whatever the focus says.
        assert_eq!(dot_state(true, false, Some("up")), DotState::Offline);
        assert_eq!(dot_state(true, true, Some("up")), DotState::Online);
        assert_eq!(dot_state(true, true, Some("down")), DotState::Offline);
        assert_eq!(dot_state(true, true, Some("unknown")), DotState::Checking);
        // A stale pick absent from the registry reads offline.
        assert_eq!(dot_state(true, true, None), DotState::Offline);
    }
}
