//! Session-list filtering, sorting, and cwd grouping — the pure half
//! of the `/` page's filter bar. `SessionRow` is the lightweight view
//! model the list renders; everything here is DOM-free and linear.

use std::collections::{BTreeMap, BTreeSet};

/// The agent select's "no filter" option value.
pub const ALL_AGENTS: &str = "all";

/// One session-list row — the subset of the summary DTO the list view
/// reads, kept plain so the whole filter pipeline is testable.
/// `sepia_web::dto` converts `SessionSummaryDto` into this.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    /// Working directory — the grouping key (`""` = ungrouped).
    pub cwd: String,
    pub agent: String,
    /// RFC 3339. The node emits `Z`-suffixed UTC stamps, which order
    /// lexicographically — sorting stays a plain string compare, and
    /// unparseable values still order deterministically.
    pub updated_at: String,
    /// Another process holds the store lock.
    pub locked: bool,
    /// A live agent process is attached (feed-provided flag).
    pub live: bool,
    /// A run is in flight.
    pub busy: bool,
    pub pinned: bool,
    /// Owning node id on multi-node hubs.
    pub node: Option<String>,
}

/// Sort order for the session list (filter-bar select values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    /// `updated_at` descending.
    #[default]
    Newest,
    /// `updated_at` ascending.
    Oldest,
    /// A–Z by title, newest first on ties.
    Title,
}

impl SortMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "oldest" => Self::Oldest,
            "title" => Self::Title,
            _ => Self::Newest,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Oldest => "oldest",
            Self::Title => "title",
        }
    }
}

/// Lock-state filter for the session list (filter-bar select values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    /// Unlocked sessions only.
    Free,
    /// Locked sessions only.
    Locked,
}

impl StatusFilter {
    pub fn parse(s: &str) -> Self {
        match s {
            "free" => Self::Free,
            "locked" => Self::Locked,
            _ => Self::All,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Free => "free",
            Self::Locked => "locked",
        }
    }
}

/// Persisted collapse state — the cwd keys of closed sections. Stored
/// in localStorage (`sepia-list-collapsed`) as a plain JSON array.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct CollapsedGroups(BTreeSet<String>);

impl CollapsedGroups {
    /// Flip one section's collapsed state.
    pub fn toggle(&mut self, cwd: &str) {
        if !self.0.remove(cwd) {
            self.0.insert(cwd.to_string());
        }
    }

    pub fn is_closed(&self, cwd: &str) -> bool {
        self.0.contains(cwd)
    }
}

/// The filter bar's full state — a pure value so the same pipeline
/// drives the rendered list, the "N of M sessions" heading, and the
/// arrow-key order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionFilter {
    /// Case-insensitive substring match on title + cwd.
    pub query: String,
    pub status: StatusFilter,
    /// [`ALL_AGENTS`] or an exact agent id.
    pub agent: String,
    pub sort: SortMode,
}

impl Default for SessionFilter {
    fn default() -> Self {
        Self {
            query: String::new(),
            status: StatusFilter::All,
            agent: ALL_AGENTS.to_string(),
            sort: SortMode::Newest,
        }
    }
}

impl SessionFilter {
    /// Any constraint active — drives the "N of M sessions" heading.
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty()
            || self.status != StatusFilter::All
            || self.agent != ALL_AGENTS
    }

    /// A text query force-expands every group (so matches stay
    /// visible); the selects don't.
    pub fn force_open(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Filter + sort the rows.
    pub fn apply(&self, rows: Vec<SessionRow>) -> Vec<SessionRow> {
        let q = self.query.trim().to_lowercase();
        let mut rows: Vec<SessionRow> = rows
            .into_iter()
            .filter(|s| {
                let query_ok = q.is_empty()
                    || s.title.to_lowercase().contains(&q)
                    || s.cwd.to_lowercase().contains(&q);
                let status_ok = match self.status {
                    StatusFilter::All => true,
                    StatusFilter::Free => !s.locked,
                    StatusFilter::Locked => s.locked,
                };
                let agent_ok = self.agent == ALL_AGENTS || s.agent == self.agent;
                query_ok && status_ok && agent_ok
            })
            .collect();
        match self.sort {
            SortMode::Newest => rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
            SortMode::Oldest => rows.sort_by(|a, b| a.updated_at.cmp(&b.updated_at)),
            SortMode::Title => rows.sort_by(|a, b| {
                a.title
                    .to_lowercase()
                    .cmp(&b.title.to_lowercase())
                    .then_with(|| b.updated_at.cmp(&a.updated_at))
            }),
        }
        rows
    }

    /// The full pipeline: filter, sort, then group by cwd.
    pub fn groups(&self, rows: Vec<SessionRow>) -> Vec<Group> {
        group_by_cwd(self.apply(rows))
    }

    /// The count line above the list — `"N of M sessions"` while
    /// filtering, `"M sessions"` otherwise.
    pub fn heading(&self, shown: usize, total: usize) -> String {
        if self.is_active() {
            format!("{shown} of {total} sessions")
        } else {
            format!("{total} sessions")
        }
    }
}

/// One collapsible section of the list — a cwd bucket with its display
/// label. Collapse is deliberately NOT baked in: the view toggles it
/// per-section (class-driven, no re-render); [`group_open`] answers it.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// The cwd this section groups under (`""` = "No project").
    pub key: String,
    /// Last path segment of `key`, or `"No project"`.
    pub label: String,
    /// Rows in display order (the filter's sort is preserved).
    pub rows: Vec<SessionRow>,
}

/// Group by `cwd`. Non-empty-cwd groups come first, ordered by the
/// most recent `updated_at` within the group; the empty-cwd
/// ("uncategorized") group is always last. Row order inside each group
/// is preserved.
pub fn group_by_cwd(rows: Vec<SessionRow>) -> Vec<Group> {
    let mut map: BTreeMap<String, Vec<SessionRow>> = BTreeMap::new();
    for r in rows {
        map.entry(r.cwd.clone()).or_default().push(r);
    }
    let (mut named, unnamed): (Vec<_>, Vec<_>) =
        map.into_iter().partition(|(cwd, _)| !cwd.is_empty());
    named.sort_by(|(_, a), (_, b)| newest_updated(b).cmp(newest_updated(a)));
    named.extend(unnamed);
    named
        .into_iter()
        .map(|(key, rows)| Group {
            label: cwd_label(&key),
            key,
            rows,
        })
        .collect()
}

/// Whether a section's rows are shown — collapsed state loses to an
/// active text query, which force-expands every group.
pub fn group_open(key: &str, collapsed: &CollapsedGroups, force_open: bool) -> bool {
    force_open || !collapsed.is_closed(key)
}

/// `(id, agent)` for every row the arrow keys can reach — open
/// sections only, in display order.
pub fn visible_keys(
    groups: &[Group],
    collapsed: &CollapsedGroups,
    force_open: bool,
) -> Vec<(String, String)> {
    groups
        .iter()
        .filter(|g| group_open(&g.key, collapsed, force_open))
        .flat_map(|g| g.rows.iter().map(|r| (r.id.clone(), r.agent.clone())))
        .collect()
}

/// Distinct agent ids across the list — the filter select's options.
pub fn agent_options(rows: &[SessionRow]) -> Vec<String> {
    let mut agents: Vec<String> = rows
        .iter()
        .filter(|r| !r.agent.is_empty())
        .map(|r| r.agent.clone())
        .collect();
    agents.sort();
    agents.dedup();
    agents
}

/// `?session=<id>` (+ `&agent=` when the agent is known).
pub fn session_href(id: &str, agent: &str) -> String {
    if agent.is_empty() {
        format!("/?session={id}")
    } else {
        format!("/?session={id}&agent={agent}")
    }
}

/// Section label for a `cwd` — the last path segment, or
/// `"No project"` when the cwd is empty (or a bare root).
pub fn cwd_label(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    if base.is_empty() {
        "No project".to_string()
    } else {
        base.to_string()
    }
}

/// The newest `updated_at` in a group — RFC 3339 `Z` strings order
/// lexicographically.
fn newest_updated(rows: &[SessionRow]) -> &str {
    rows.iter()
        .map(|s| s.updated_at.as_str())
        .max()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn s(
        id: &str,
        title: &str,
        cwd: &str,
        agent: &str,
        updated_at: &str,
        locked: bool,
    ) -> SessionRow {
        SessionRow {
            id: id.into(),
            title: title.into(),
            cwd: cwd.into(),
            agent: agent.into(),
            updated_at: updated_at.into(),
            locked,
            ..SessionRow::default()
        }
    }

    fn list() -> Vec<SessionRow> {
        vec![
            s(
                "a",
                "Fix Auth",
                "/home/u/app",
                "claude",
                "2026-10-08T10:00:00Z",
                false,
            ),
            s(
                "b",
                "docs",
                "/home/u/app",
                "cline",
                "2026-10-09T10:00:00Z",
                true,
            ),
            s(
                "c",
                "API work",
                "/var/www/site",
                "claude",
                "2026-10-07T10:00:00Z",
                false,
            ),
            s("d", "", "", "cursor", "2026-10-06T10:00:00Z", false),
        ]
    }

    fn ids(rows: &[SessionRow]) -> Vec<&str> {
        rows.iter().map(|s| s.id.as_str()).collect()
    }

    fn filter(query: &str, status: StatusFilter, agent: &str, sort: SortMode) -> SessionFilter {
        SessionFilter {
            query: query.into(),
            status,
            agent: agent.into(),
            sort,
        }
    }

    #[test]
    fn query_matches_title_and_cwd_case_insensitively() {
        let out = filter("AUTH", StatusFilter::All, ALL_AGENTS, SortMode::Newest).apply(list());
        assert_eq!(ids(&out), ["a"]);
        let out = filter("site", StatusFilter::All, ALL_AGENTS, SortMode::Newest).apply(list());
        assert_eq!(ids(&out), ["c"]);
        // Whitespace-only query behaves like no filter.
        let out = filter("   ", StatusFilter::All, ALL_AGENTS, SortMode::Newest).apply(list());
        assert_eq!(out.len(), 4);
        let out = filter("nope", StatusFilter::All, ALL_AGENTS, SortMode::Newest).apply(list());
        assert!(out.is_empty());
    }

    #[test]
    fn status_and_agent_filters_apply() {
        let locked = filter("", StatusFilter::Locked, ALL_AGENTS, SortMode::Newest).apply(list());
        assert_eq!(ids(&locked), ["b"]);
        let free = filter("", StatusFilter::Free, ALL_AGENTS, SortMode::Newest).apply(list());
        assert_eq!(ids(&free), ["a", "c", "d"]);
        let agent = filter("", StatusFilter::All, "cline", SortMode::Newest).apply(list());
        assert_eq!(ids(&agent), ["b"]);
        let missing = filter("", StatusFilter::All, "ghost", SortMode::Newest).apply(list());
        assert!(missing.is_empty());
    }

    #[test]
    fn sorts_by_newest_oldest_and_title() {
        let newest = filter("", StatusFilter::All, ALL_AGENTS, SortMode::Newest).apply(list());
        assert_eq!(ids(&newest), ["b", "a", "c", "d"]);
        let oldest = filter("", StatusFilter::All, ALL_AGENTS, SortMode::Oldest).apply(list());
        assert_eq!(ids(&oldest), ["d", "c", "a", "b"]);
        // Title sort is case-insensitive; the empty title ranks first.
        let title = filter("", StatusFilter::All, ALL_AGENTS, SortMode::Title).apply(list());
        assert_eq!(ids(&title), ["d", "c", "b", "a"]);
    }

    #[test]
    fn groups_order_by_recency_with_uncategorized_last() {
        let groups = group_by_cwd(list());
        assert_eq!(groups.len(), 3);
        // `/home/u/app` has the newest row (10-09), `/var/www/site`
        // next (10-07), and the empty cwd trails.
        assert_eq!(groups[0].key, "/home/u/app");
        assert_eq!(groups[0].rows.len(), 2);
        assert_eq!(groups[0].label, "app");
        assert_eq!(groups[1].key, "/var/www/site");
        assert_eq!(groups[2].key, "");
        assert_eq!(groups[2].label, "No project");
        assert_eq!(groups[2].rows.len(), 1);
        // Empty input → no groups.
        assert!(group_by_cwd(Vec::new()).is_empty());
    }

    #[test]
    fn collapsed_groups_toggle_and_skip_rows() {
        let mut collapsed = CollapsedGroups::default();
        let groups = group_by_cwd(list());
        assert_eq!(visible_keys(&groups, &collapsed, false).len(), 4);
        collapsed.toggle("/home/u/app");
        assert!(collapsed.is_closed("/home/u/app"));
        assert!(!group_open("/home/u/app", &collapsed, false));
        // Closed section's rows drop out of the arrow-key order.
        let keys = visible_keys(&groups, &collapsed, false);
        assert_eq!(
            keys,
            [
                ("c".to_string(), "claude".to_string()),
                ("d".to_string(), "cursor".to_string())
            ]
        );
        // A text query force-expands everything.
        assert!(group_open("/home/u/app", &collapsed, true));
        assert_eq!(visible_keys(&groups, &collapsed, true).len(), 4);
        // Toggle again re-opens.
        collapsed.toggle("/home/u/app");
        assert!(!collapsed.is_closed("/home/u/app"));
    }

    #[test]
    fn collapsed_groups_persist_as_json_array() {
        // Back-compat: the stored shape is the old `Vec<String>` JSON.
        let mut collapsed = CollapsedGroups::default();
        collapsed.toggle("/a");
        collapsed.toggle("/b");
        collapsed.toggle("/a");
        assert_eq!(serde_json::to_string(&collapsed).unwrap(), r#"["/b"]"#);
        let back: CollapsedGroups = serde_json::from_str(r#"["/b","/c"]"#).unwrap();
        assert!(back.is_closed("/b") && back.is_closed("/c") && !back.is_closed("/a"));
    }

    #[test]
    fn heading_counts_and_filter_activity() {
        let f = filter("", StatusFilter::All, ALL_AGENTS, SortMode::Newest);
        assert!(!f.is_active());
        assert_eq!(f.heading(4, 4), "4 sessions");
        let f = filter("x", StatusFilter::All, ALL_AGENTS, SortMode::Newest);
        assert!(f.is_active());
        assert!(f.force_open());
        assert_eq!(f.heading(2, 4), "2 of 4 sessions");
        // Non-text filters count as active but don't force-expand.
        let f = filter("", StatusFilter::Locked, ALL_AGENTS, SortMode::Newest);
        assert!(f.is_active());
        assert!(!f.force_open());
    }

    #[test]
    fn agent_options_are_sorted_and_deduped() {
        let opts = agent_options(&list());
        assert_eq!(opts, ["claude", "cline", "cursor"]);
    }

    #[test]
    fn session_href_carries_the_agent() {
        assert_eq!(session_href("s1", ""), "/?session=s1");
        assert_eq!(session_href("s1", "claude"), "/?session=s1&agent=claude");
    }

    #[test]
    fn cwd_label_uses_last_path_segment() {
        assert_eq!(cwd_label("/home/u/app"), "app");
        assert_eq!(cwd_label("/home/u/app/"), "app");
        assert_eq!(cwd_label("app"), "app");
        assert_eq!(cwd_label(""), "No project");
        assert_eq!(cwd_label("/"), "No project");
        assert_eq!(cwd_label("C:\\src\\proj"), "proj");
    }

    #[test]
    fn select_parse_round_trips() {
        assert_eq!(SortMode::parse("oldest"), SortMode::Oldest);
        assert_eq!(SortMode::parse("bogus"), SortMode::Newest);
        assert_eq!(SortMode::parse(SortMode::Title.as_str()), SortMode::Title);
        assert_eq!(StatusFilter::parse("locked"), StatusFilter::Locked);
        assert_eq!(StatusFilter::parse("bogus"), StatusFilter::All);
        assert_eq!(
            StatusFilter::parse(StatusFilter::Free.as_str()),
            StatusFilter::Free
        );
    }
}
