//! Session-list filtering, sorting, and cwd grouping — the pure half
//! of the `/` page's filter bar. `SessionRow` is the lightweight view
//! model the list renders; everything here is DOM-free and linear.

use std::collections::{BTreeMap, BTreeSet};

/// The agent select's "no filter" option value.
pub const ALL_AGENTS: &str = "all";

/// Collapse key for the pinned section. `\0` can't appear in a
/// filesystem path, so it can't collide with a cwd group key.
pub const PINNED_KEY: &str = "\0pinned";

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
    /// Archived rows hide unless the filter's `show_archived` is set.
    pub archived: bool,
    /// Set on sub-agent sessions — the id of the session that spawned
    /// this one.
    pub parent_session_id: Option<String>,
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

    /// Force one section open (`false`) or closed (`true`) — the
    /// `←`/`→` fold hotkeys write absolute state, not a toggle.
    pub fn set_closed(&mut self, cwd: &str, closed: bool) {
        if closed {
            self.0.insert(cwd.to_string());
        } else {
            self.0.remove(cwd);
        }
    }

    /// Re-open every section — "unfold all" also drops stale keys for
    /// cwds that no longer have a group.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_closed(&self, cwd: &str) -> bool {
        self.0.contains(cwd)
    }
}

/// Recency window for the `updated_at` filter chips. Ages are fixed
/// windows (24h/7d/30d), not calendar boundaries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Recency {
    /// No age constraint.
    #[default]
    Any,
    /// `updated_at` within the last 24 hours.
    Day,
    /// Within the last 7 days.
    Week,
    /// Within the last 30 days.
    Month,
}

impl Recency {
    pub fn parse(s: &str) -> Self {
        match s {
            "day" => Self::Day,
            "week" => Self::Week,
            "month" => Self::Month,
            _ => Self::Any,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    /// The window width in milliseconds — `None` for `Any`.
    pub const fn max_age_ms(self) -> Option<f64> {
        match self {
            Self::Any => None,
            Self::Day => Some(86_400_000.0),
            Self::Week => Some(604_800_000.0),
            Self::Month => Some(2_592_000_000.0),
        }
    }
}

/// The filter bar's full state — a pure value so the same pipeline
/// drives the rendered list, the "N of M sessions" heading, the
/// arrow-key order, and the `?…` URL sync.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Case-insensitive substring match on title + cwd.
    pub query: String,
    pub status: StatusFilter,
    /// Agent multi-select — an empty set means "all agents".
    pub agents: BTreeSet<String>,
    pub sort: SortMode,
    /// `updated_at` age window.
    pub recency: Recency,
    /// Include archived rows in the list (default: hidden).
    /// Deliberately not counted by [`is_active`](Self::is_active) —
    /// it widens the list, it doesn't narrow it.
    pub show_archived: bool,
}

impl SessionFilter {
    /// Any narrowing constraint active — drives the "N of M sessions"
    /// heading.
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty()
            || self.status != StatusFilter::All
            || !self.agents.is_empty()
            || self.recency != Recency::Any
    }

    /// A text query force-expands every group (so matches stay
    /// visible); the other filters don't.
    pub fn force_open(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Filter + sort the rows. `now_ms` (epoch ms) feeds the recency
    /// window; callers pass the `Now` clock.
    pub fn apply(&self, rows: Vec<SessionRow>, now_ms: f64) -> Vec<SessionRow> {
        let q = self.query.trim().to_lowercase();
        let cutoff = self.recency.max_age_ms().map(|age| now_ms - age);
        let mut rows: Vec<SessionRow> = rows
            .into_iter()
            .filter(|s| {
                let archived_ok = self.show_archived || !s.archived;
                let query_ok = q.is_empty()
                    || s.title.to_lowercase().contains(&q)
                    || s.cwd.to_lowercase().contains(&q);
                let status_ok = match self.status {
                    StatusFilter::All => true,
                    StatusFilter::Free => !s.locked,
                    StatusFilter::Locked => s.locked,
                };
                let agent_ok = self.agents.is_empty() || self.agents.contains(&s.agent);
                // Unparseable stamps survive the age filter — missing
                // data should never hide a row.
                let recency_ok =
                    cutoff.is_none_or(|c| iso_ms(&s.updated_at).is_none_or(|t| t >= c));
                archived_ok && query_ok && status_ok && agent_ok && recency_ok
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

    /// The full pipeline: filter + sort, then a Pinned section on top
    /// (its rows are lifted out of the cwd groups — every row lives
    /// in exactly one section, which keeps the visible order and the
    /// per-row `?session=` anchors unambiguous) and the project
    /// groups below.
    pub fn sections(&self, rows: Vec<SessionRow>, now_ms: f64) -> Vec<Section> {
        let rows = self.apply(rows, now_ms);
        let (pinned, rest): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| r.pinned);
        let mut out = Vec::new();
        if !pinned.is_empty() {
            out.push(Section {
                kind: SectionKind::Pinned,
                key: PINNED_KEY.to_string(),
                label: "Pinned".to_string(),
                rows: pinned,
            });
        }
        out.extend(group_by_cwd(rest).into_iter().map(|g| Section {
            kind: SectionKind::Project,
            key: g.key,
            label: g.label,
            rows: g.rows,
        }));
        out
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

    /// Read filter state out of the `?…` params (`q`, `agents`,
    /// `status`, `sort`, `recency`, `archived`); absent/unknown values
    /// fall back to defaults so deep links can't wedge the list.
    /// `session`/`agent` are selection params, not filters — callers
    /// leave them alone.
    pub fn from_params(get: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            query: get("q").unwrap_or_default(),
            status: StatusFilter::parse(&get("status").unwrap_or_default()),
            agents: get("agents")
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            sort: SortMode::parse(&get("sort").unwrap_or_default()),
            recency: Recency::parse(&get("recency").unwrap_or_default()),
            show_archived: get("archived").is_some_and(|v| v == "1"),
        }
    }

    /// The `?…` pairs this filter contributes — only non-defaults, so
    /// a stock list keeps a clean URL.
    pub fn query_params(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if !self.query.is_empty() {
            out.push(("q".to_string(), self.query.clone()));
        }
        if !self.agents.is_empty() {
            // BTreeSet order — stable URLs for the same set.
            out.push((
                "agents".to_string(),
                self.agents.iter().cloned().collect::<Vec<_>>().join(","),
            ));
        }
        if self.status != StatusFilter::All {
            out.push(("status".to_string(), self.status.as_str().to_string()));
        }
        if self.sort != SortMode::Newest {
            out.push(("sort".to_string(), self.sort.as_str().to_string()));
        }
        if self.recency != Recency::Any {
            out.push(("recency".to_string(), self.recency.as_str().to_string()));
        }
        if self.show_archived {
            out.push(("archived".to_string(), "1".to_string()));
        }
        out
    }
}

/// One rendered section of the list — either the pinned block or a
/// cwd project group.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub kind: SectionKind,
    /// Collapse key — the cwd for projects, [`PINNED_KEY`] for pinned.
    pub key: String,
    /// Header label ("Pinned" or the cwd's last path segment).
    pub label: String,
    /// Rows in display order (the filter's sort is preserved).
    pub rows: Vec<SessionRow>,
}

/// Which kind of list section this is — drives the header glyph and
/// whether a "New session here" menu item makes sense.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// The pinned block — always first, collapse key [`PINNED_KEY`].
    Pinned,
    /// A cwd project group.
    Project,
}

/// One cwd bucket feeding [`Section`]. Collapse is deliberately NOT
/// baked in: the view toggles it per-section (class-driven, no
/// re-render); [`group_open`] answers it.
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
    sections: &[Section],
    collapsed: &CollapsedGroups,
    force_open: bool,
) -> Vec<(String, String)> {
    sections
        .iter()
        .filter(|s| group_open(&s.key, collapsed, force_open))
        .flat_map(|s| s.rows.iter().map(|r| (r.id.clone(), r.agent.clone())))
        .collect()
}

/// The section keys `←` (fold) and `→` (unfold) act on: the section
/// holding the selected row, or every section when nothing is
/// selected (fold-all / unfold-all).
pub fn fold_keys(sections: &[Section], selected: Option<&str>) -> Vec<String> {
    if let Some(id) = selected
        && let Some(s) = sections.iter().find(|s| s.rows.iter().any(|r| r.id == id))
    {
        return vec![s.key.clone()];
    }
    sections.iter().map(|s| s.key.clone()).collect()
}

/// Parent id → child count, for the `↳ N` marker on rows that spawned
/// sub-agent sessions.
pub fn child_counts(rows: &[SessionRow]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for r in rows {
        if let Some(parent) = &r.parent_session_id {
            *counts.entry(parent.clone()).or_insert(0) += 1;
        }
    }
    counts
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
        format!("/?session={}", url_escape(id))
    } else {
        format!("/?session={}&agent={}", url_escape(id), url_escape(agent))
    }
}

/// [`session_href`] plus the filter's `?…` pairs — appended after
/// `session`/`agent` so the `a[href^="/?session="]` row selector the
/// e2e suite asserts keeps matching. Deep links and reloads keep the
/// same filter bar state.
pub fn session_href_filtered(filter: &SessionFilter, id: &str, agent: &str) -> String {
    let mut pairs = vec![("session".to_string(), id.to_string())];
    if !agent.is_empty() {
        pairs.push(("agent".to_string(), agent.to_string()));
    }
    pairs.extend(filter.query_params());
    query_href("/", &pairs)
}

/// `path` + an escaped `?k=v&…` query (`path` bare when `pairs` is
/// empty). Kept dependency-free — percent-encodes values only.
pub fn query_href(path: &str, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return path.to_string();
    }
    let qs = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_escape(k), url_escape(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{qs}")
}

/// Minimal RFC 3986 unreserved-set percent-encoding — enough for
/// query values (paths, agent ids, free text).
pub fn url_escape(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Parse `"2026-10-08T06:40:34.123Z"` into epoch milliseconds —
/// `Z` required, `±HH:MM` offsets accepted leniently (ignored;
/// display-precision only). `None` on anything else, so the recency
/// filter can keep undatable rows instead of guessing.
/// (`sepia_web::time::parse_iso_ms` is the same algorithm for display.)
pub fn iso_ms(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { s.get(from..to)?.parse().ok() };
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let minute = num(14, 16)?;
    let second = num(17, 19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut frac_ms = 0.0;
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: usize = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let scale = 3_i32.saturating_sub(i32::try_from(digits).unwrap_or(i32::MAX));
        frac_ms = frac[..digits].parse::<f64>().ok()? * 10f64.powi(scale);
        rest = &frac[digits..];
    }
    if rest != "Z" && !rest.starts_with('+') && !rest.starts_with('-') {
        return None;
    }

    // Days since epoch — Howard Hinnant's civil-to-days algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    Some(
        days as f64 * 86_400_000.0
            + hour as f64 * 3_600_000.0
            + minute as f64 * 60_000.0
            + second as f64 * 1_000.0
            + frac_ms,
    )
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

    /// A fixed "now" for the recency tests — 2026-10-10T00:00:00Z.
    const NOW: f64 = 1_791_590_400_000.0;

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
                "2026-10-09T10:00:00Z",
                false,
            ),
            SessionRow {
                pinned: true,
                ..s(
                    "b",
                    "docs",
                    "/home/u/app",
                    "cline",
                    "2026-10-09T12:00:00Z",
                    true,
                )
            },
            s(
                "c",
                "API work",
                "/var/www/site",
                "claude",
                "2026-10-07T10:00:00Z",
                false,
            ),
            s("d", "", "", "cursor", "2026-10-06T10:00:00Z", false),
            SessionRow {
                archived: true,
                ..s(
                    "e",
                    "stale spike",
                    "/home/u/app",
                    "cline",
                    "2026-10-09T11:00:00Z",
                    false,
                )
            },
        ]
    }

    fn ids(rows: &[SessionRow]) -> Vec<&str> {
        rows.iter().map(|s| s.id.as_str()).collect()
    }

    fn filter(query: &str, status: StatusFilter, agents: &[&str], sort: SortMode) -> SessionFilter {
        SessionFilter {
            query: query.into(),
            status,
            agents: agents.iter().map(|a| (*a).to_string()).collect(),
            sort,
            ..SessionFilter::default()
        }
    }

    #[test]
    fn query_matches_title_and_cwd_case_insensitively() {
        let out = filter("AUTH", StatusFilter::All, &[], SortMode::Newest).apply(list(), NOW);
        assert_eq!(ids(&out), ["a"]);
        let out = filter("site", StatusFilter::All, &[], SortMode::Newest).apply(list(), NOW);
        assert_eq!(ids(&out), ["c"]);
        // Whitespace-only query behaves like no filter.
        let out = filter("   ", StatusFilter::All, &[], SortMode::Newest).apply(list(), NOW);
        assert_eq!(out.len(), 4);
        let out = filter("nope", StatusFilter::All, &[], SortMode::Newest).apply(list(), NOW);
        assert!(out.is_empty());
    }

    #[test]
    fn status_and_agent_filters_apply() {
        let locked = filter("", StatusFilter::Locked, &[], SortMode::Newest).apply(list(), NOW);
        assert_eq!(ids(&locked), ["b"]);
        let free = filter("", StatusFilter::Free, &[], SortMode::Newest).apply(list(), NOW);
        assert_eq!(ids(&free), ["a", "c", "d"]);
        let agent = filter("", StatusFilter::All, &["cline"], SortMode::Newest).apply(list(), NOW);
        assert_eq!(ids(&agent), ["b"]);
        let missing =
            filter("", StatusFilter::All, &["ghost"], SortMode::Newest).apply(list(), NOW);
        assert!(missing.is_empty());
    }

    #[test]
    fn multi_agent_filter_matches_any_member() {
        let out = filter(
            "",
            StatusFilter::All,
            &["claude", "cursor"],
            SortMode::Newest,
        )
        .apply(list(), NOW);
        assert_eq!(ids(&out), ["a", "c", "d"]);
        // An empty set means "all agents", not "none".
        let out = filter("", StatusFilter::All, &[], SortMode::Newest).apply(list(), NOW);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn archived_toggle_shows_archived_rows() {
        let mut f = filter("", StatusFilter::All, &[], SortMode::Newest);
        let hidden = f.apply(list(), NOW);
        assert_eq!(ids(&hidden), ["b", "a", "c", "d"]);
        f.show_archived = true;
        let shown = f.apply(list(), NOW);
        // Archived "e" slots back in at its updated_at position.
        assert_eq!(ids(&shown), ["b", "e", "a", "c", "d"]);
        // …and doesn't count as a narrowing filter for the heading.
        assert!(!f.is_active());
        assert_eq!(f.heading(5, 5), "5 sessions");
    }

    #[test]
    fn recency_windows_filter_by_updated_age() {
        for (recency, want) in [
            (Recency::Any, vec!["b", "a", "c", "d"]),
            (Recency::Day, vec!["b", "a"]),
            (Recency::Week, vec!["b", "a", "c", "d"]),
            (Recency::Month, vec!["b", "a", "c", "d"]),
        ] {
            let mut f = filter("", StatusFilter::All, &[], SortMode::Newest);
            f.recency = recency;
            assert_eq!(ids(&f.apply(list(), NOW)), want, "{recency:?}");
        }
        // An old row drops out under a tighter window.
        let mut f = filter("", StatusFilter::All, &[], SortMode::Newest);
        f.recency = Recency::Day;
        f.show_archived = true;
        let out = f.apply(list(), NOW);
        assert_eq!(ids(&out), ["b", "e", "a"]);
    }

    #[test]
    fn recency_keeps_rows_with_unparseable_stamps() {
        let row = s("x", "no date", "/p", "a", "garbage", false);
        let mut f = filter("", StatusFilter::All, &[], SortMode::Newest);
        f.recency = Recency::Day;
        assert_eq!(ids(&f.apply(vec![row], NOW)), ["x"]);
    }

    #[test]
    fn sorts_by_newest_oldest_and_title() {
        let newest = filter("", StatusFilter::All, &[], SortMode::Newest).apply(list(), NOW);
        assert_eq!(ids(&newest), ["b", "a", "c", "d"]);
        let oldest = filter("", StatusFilter::All, &[], SortMode::Oldest).apply(list(), NOW);
        assert_eq!(ids(&oldest), ["d", "c", "a", "b"]);
        // Title sort is case-insensitive; the empty title ranks first.
        let title = filter("", StatusFilter::All, &[], SortMode::Title).apply(list(), NOW);
        assert_eq!(ids(&title), ["d", "c", "b", "a"]);
    }

    #[test]
    fn groups_order_by_recency_with_uncategorized_last() {
        let groups = group_by_cwd(list());
        assert_eq!(groups.len(), 3);
        // `/home/u/app` has the newest row (10-09 12:00), `/var/www/site`
        // next (10-07), and the empty cwd trails.
        assert_eq!(groups[0].key, "/home/u/app");
        assert_eq!(groups[0].rows.len(), 3);
        assert_eq!(groups[0].label, "app");
        assert_eq!(groups[1].key, "/var/www/site");
        assert_eq!(groups[2].key, "");
        assert_eq!(groups[2].label, "No project");
        assert_eq!(groups[2].rows.len(), 1);
        // Empty input → no groups.
        assert!(group_by_cwd(Vec::new()).is_empty());
    }

    #[test]
    fn sections_lift_pinned_rows_out_of_project_groups() {
        let sections = filter("", StatusFilter::All, &[], SortMode::Newest).sections(list(), NOW);
        assert_eq!(sections[0].kind, SectionKind::Pinned);
        assert_eq!(sections[0].key, PINNED_KEY);
        assert_eq!(sections[0].label, "Pinned");
        assert_eq!(ids(&sections[0].rows), ["b"]);
        // "b" doesn't repeat inside /home/u/app.
        let app = &sections[1];
        assert_eq!(app.kind, SectionKind::Project);
        assert_eq!(app.key, "/home/u/app");
        assert_eq!(ids(&app.rows), ["a"]);
        // Every filtered row lands in exactly one section.
        let total: usize = sections.iter().map(|s| s.rows.len()).sum();
        assert_eq!(total, 4);
        // No pinned rows → no pinned section.
        let mut unpinned = list();
        for r in &mut unpinned {
            r.pinned = false;
        }
        let sections = filter("", StatusFilter::All, &[], SortMode::Newest).sections(unpinned, NOW);
        assert!(sections.iter().all(|s| s.kind == SectionKind::Project));
    }

    #[test]
    fn collapsed_groups_toggle_and_skip_rows() {
        let mut collapsed = CollapsedGroups::default();
        let sections = filter("", StatusFilter::All, &[], SortMode::Newest).sections(list(), NOW);
        assert_eq!(visible_keys(&sections, &collapsed, false).len(), 4);
        collapsed.toggle("/home/u/app");
        assert!(collapsed.is_closed("/home/u/app"));
        assert!(!group_open("/home/u/app", &collapsed, false));
        // Closed section's rows drop out of the arrow-key order.
        let keys = visible_keys(&sections, &collapsed, false);
        assert_eq!(
            keys,
            [
                ("b".to_string(), "cline".to_string()),
                ("c".to_string(), "claude".to_string()),
                ("d".to_string(), "cursor".to_string())
            ]
        );
        // A text query force-expands everything.
        assert!(group_open("/home/u/app", &collapsed, true));
        assert_eq!(visible_keys(&sections, &collapsed, true).len(), 4);
        // Toggle again re-opens.
        collapsed.toggle("/home/u/app");
        assert!(!collapsed.is_closed("/home/u/app"));
    }

    #[test]
    fn folded_state_writes_absolute_open_closed() {
        let mut collapsed = CollapsedGroups::default();
        collapsed.set_closed("/a", true);
        collapsed.set_closed("/a", true);
        assert!(collapsed.is_closed("/a"));
        collapsed.set_closed("/a", false);
        assert!(!collapsed.is_closed("/a"));
        collapsed.toggle("/b");
        collapsed.clear();
        assert!(!collapsed.is_closed("/b"));
    }

    #[test]
    fn fold_keys_target_selection_or_everything() {
        let sections = filter("", StatusFilter::All, &[], SortMode::Newest).sections(list(), NOW);
        // A selected row folds only its own section.
        assert_eq!(fold_keys(&sections, Some("c")), ["/var/www/site"]);
        // A selected pinned row targets the pinned section.
        assert_eq!(fold_keys(&sections, Some("b")), [PINNED_KEY]);
        // No selection (or a stale id) folds/unfolds everything.
        let all = fold_keys(&sections, None);
        assert_eq!(all.len(), sections.len());
        assert_eq!(fold_keys(&sections, Some("ghost")), all);
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
        let f = filter("", StatusFilter::All, &[], SortMode::Newest);
        assert!(!f.is_active());
        assert_eq!(f.heading(4, 4), "4 sessions");
        let f = filter("x", StatusFilter::All, &[], SortMode::Newest);
        assert!(f.is_active());
        assert!(f.force_open());
        assert_eq!(f.heading(2, 4), "2 of 4 sessions");
        // Non-text filters count as active but don't force-expand.
        let f = filter("", StatusFilter::Locked, &[], SortMode::Newest);
        assert!(f.is_active());
        assert!(!f.force_open());
        // Neither do agent/recency filters force-expand.
        let f = filter("", StatusFilter::All, &["claude"], SortMode::Newest);
        assert!(f.is_active());
        assert!(!f.force_open());
        let mut f = filter("", StatusFilter::All, &[], SortMode::Newest);
        f.recency = Recency::Day;
        assert!(f.is_active());
    }

    #[test]
    fn agent_options_are_sorted_and_deduped() {
        let opts = agent_options(&list());
        assert_eq!(opts, ["claude", "cline", "cursor"]);
    }

    #[test]
    fn child_counts_map_parent_ids() {
        let mut rows = list();
        rows[0].parent_session_id = Some("c".into());
        rows[1].parent_session_id = Some("c".into());
        rows[2].parent_session_id = Some("gone".into());
        let counts = child_counts(&rows);
        assert_eq!(counts.get("c"), Some(&2));
        assert_eq!(counts.get("gone"), Some(&1));
        assert!(!counts.contains_key("a"));
        assert!(child_counts(&list()).is_empty());
    }

    #[test]
    fn session_href_carries_the_agent() {
        assert_eq!(session_href("s1", ""), "/?session=s1");
        assert_eq!(session_href("s1", "claude"), "/?session=s1&agent=claude");
    }

    #[test]
    fn filtered_href_keeps_session_first_for_the_row_selector() {
        let f = SessionFilter {
            query: "auth flow".into(),
            agents: BTreeSet::from(["claude".to_string()]),
            ..SessionFilter::default()
        };
        let href = session_href_filtered(&f, "s1", "claude");
        // `a[href^="/?session="]` keeps matching; filters trail.
        assert!(href.starts_with("/?session=s1"));
        assert!(href.contains("&agent=claude"));
        assert!(href.contains("q=auth%20flow"));
        assert!(href.contains("agents=claude"));
    }

    #[test]
    fn query_params_round_trip_through_from_params() {
        let f = SessionFilter {
            query: "Fix Auth".into(),
            status: StatusFilter::Locked,
            agents: BTreeSet::from(["cline".to_string(), "claude".to_string()]),
            sort: SortMode::Title,
            recency: Recency::Week,
            show_archived: true,
        };
        let params: BTreeMap<String, String> = f.query_params().into_iter().collect();
        let back = SessionFilter::from_params(|k| params.get(k).cloned());
        assert_eq!(back, f);
        // Defaults contribute nothing — the stock URL stays bare.
        assert!(SessionFilter::default().query_params().is_empty());
        assert_eq!(
            SessionFilter::from_params(|_| None),
            SessionFilter::default()
        );
        // Garbage values degrade to defaults rather than wedging.
        let junk = BTreeMap::from([
            ("status".to_string(), "bogus".to_string()),
            ("sort".to_string(), "??".to_string()),
            ("recency".to_string(), "year".to_string()),
            ("agents".to_string(), ", ,a,,".to_string()),
        ]);
        let back = SessionFilter::from_params(|k| junk.get(k).cloned());
        assert_eq!(
            back,
            SessionFilter {
                agents: BTreeSet::from(["a".to_string()]),
                ..SessionFilter::default()
            }
        );
    }

    #[test]
    fn query_href_escapes_and_skips_empty() {
        assert_eq!(query_href("/", &[]), "/");
        let href = query_href("/", &[("q".into(), "a b&c".into())]);
        assert_eq!(href, "/?q=a%20b%26c");
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
    fn iso_ms_parses_z_stamps() {
        assert_eq!(iso_ms("1970-01-01T00:00:01.5Z"), Some(1500.0));
        assert_eq!(iso_ms("1970-01-01T00:00:00Z"), Some(0.0));
        let ms = iso_ms("2026-10-08T06:40:34.123Z").unwrap();
        assert!((ms - 1_791_441_634_123.0).abs() < 1.0, "got {ms}");
        assert_eq!(iso_ms(""), None);
        assert_eq!(iso_ms("not a date"), None);
        assert_eq!(iso_ms("2026-13-40T99:99:99Z"), None);
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
        assert_eq!(Recency::parse("week"), Recency::Week);
        assert_eq!(Recency::parse("bogus"), Recency::Any);
        assert_eq!(Recency::parse(Recency::Month.as_str()), Recency::Month);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// A fixed "now" — matches the unit tests' clock.
    const NOW: f64 = 1_791_590_400_000.0;

    fn arb_row() -> impl Strategy<Value = SessionRow> {
        (
            "[a-c]{1,4}",
            "[ A-Za-z]{0,10}",
            "(/[a-z]{1,6}){0,3}",
            prop_oneof![Just("claude"), Just("cline"), Just("cursor"), Just("devin")],
            prop_oneof![
                Just("2026-10-09T12:00:00Z"),
                Just("2026-10-05T00:00:00Z"),
                Just("2026-09-01T00:00:00Z"),
                Just("unparseable"),
            ],
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(id, title, cwd, agent, updated_at, locked, pinned, archived)| SessionRow {
                    id,
                    title,
                    cwd,
                    agent: agent.to_string(),
                    updated_at: updated_at.to_string(),
                    locked,
                    pinned,
                    archived,
                    ..SessionRow::default()
                },
            )
    }

    fn arb_filter() -> impl Strategy<Value = SessionFilter> {
        (
            "[a-z ]{0,8}",
            prop_oneof![
                Just(StatusFilter::All),
                Just(StatusFilter::Free),
                Just(StatusFilter::Locked)
            ],
            prop::collection::btree_set(
                prop_oneof![Just("claude"), Just("cline"), Just("cursor")].prop_map(str::to_string),
                0..3,
            ),
            prop_oneof![
                Just(SortMode::Newest),
                Just(SortMode::Oldest),
                Just(SortMode::Title)
            ],
            prop_oneof![
                Just(Recency::Any),
                Just(Recency::Day),
                Just(Recency::Week),
                Just(Recency::Month)
            ],
            any::<bool>(),
        )
            .prop_map(|(query, status, agents, sort, recency, show_archived)| {
                SessionFilter {
                    query,
                    status,
                    agents,
                    sort,
                    recency,
                    show_archived,
                }
            })
    }

    proptest! {
        /// Sections partition the filtered rows exactly — no row is
        /// lost or duplicated between the pinned block and the cwd
        /// groups.
        #[test]
        fn sections_partition_rows_exactly(
            rows in prop::collection::vec(arb_row(), 0..30),
            f in arb_filter(),
        ) {
            let filtered = f.apply(rows, NOW);
            let sections = f.sections(filtered.clone(), NOW);
            let mut in_sections: Vec<&str> = sections
                .iter()
                .flat_map(|s| s.rows.iter().map(|r| r.id.as_str()))
                .collect();
            in_sections.sort_unstable();
            let mut filtered_ids: Vec<&str> =
                filtered.iter().map(|r| r.id.as_str()).collect();
            filtered_ids.sort_unstable();
            prop_assert_eq!(in_sections, filtered_ids);
            // Pinned is unique and, when present, always first.
            let pinned_at = sections
                .iter()
                .position(|s| s.kind == SectionKind::Pinned);
            prop_assert!(pinned_at.is_none_or(|i| i == 0));
        }

        /// `visible_keys` only ever returns ids from open sections —
        /// folding can shrink the list but never invents rows.
        #[test]
        fn folding_only_removes_rows(
            rows in prop::collection::vec(arb_row(), 0..30),
            f in arb_filter(),
            folds in prop::collection::vec(any::<bool>(), 0..30),
        ) {
            let sections = f.sections(rows, NOW);
            let all = visible_keys(&sections, &CollapsedGroups::default(), false);
            let mut collapsed = CollapsedGroups::default();
            for (section, fold) in sections.iter().zip(folds.iter().cycle()) {
                collapsed.set_closed(&section.key, *fold);
            }
            let visible = visible_keys(&sections, &collapsed, false);
            prop_assert!(visible.len() <= all.len());
            let ids: std::collections::BTreeSet<_> =
                all.iter().map(|(id, _)| id.clone()).collect();
            prop_assert!(visible.iter().all(|(id, _)| ids.contains(id)));
            // Force-open restores every row.
            let forced = visible_keys(&sections, &collapsed, true);
            prop_assert_eq!(forced, all);
        }

        /// The archive toggle only ever changes the *archived* rows'
        /// membership — the visible non-archived set is identical
        /// either way.
        #[test]
        fn archive_toggle_preserves_partition(
            rows in prop::collection::vec(arb_row(), 0..30),
            f in arb_filter(),
        ) {
            let mut off = f.clone();
            off.show_archived = false;
            let mut on = f.clone();
            on.show_archived = true;
            let hidden = off.apply(rows.clone(), NOW);
            let shown = on.apply(rows, NOW);
            prop_assert!(hidden.iter().all(|r| !r.archived));
            prop_assert_eq!(
                ids_of_unarchived(&hidden),
                ids_of_unarchived(&shown)
            );
            prop_assert!(shown.len() >= hidden.len());
        }

        /// A recency window only narrows the unfiltered set.
        #[test]
        fn recency_only_narrows(
            rows in prop::collection::vec(arb_row(), 0..30),
            recency in prop_oneof![
                Just(Recency::Day),
                Just(Recency::Week),
                Just(Recency::Month)
            ],
        ) {
            let mut f = SessionFilter::default();
            let all = f.apply(rows.clone(), NOW);
            f.recency = recency;
            let windowed = f.apply(rows, NOW);
            prop_assert!(windowed.len() <= all.len());
            let kept: std::collections::BTreeSet<_> =
                all.iter().map(|r| r.id.clone()).collect();
            prop_assert!(windowed.iter().all(|r| kept.contains(&r.id)));
        }
    }

    fn ids_of_unarchived(rows: &[SessionRow]) -> Vec<&str> {
        rows.iter()
            .filter(|r| !r.archived)
            .map(|r| r.id.as_str())
            .collect()
    }
}
