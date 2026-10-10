//! Command-palette ranking — the pure half of the `⌘K`/`/` overlay in
//! `sepia-web`. The DOM layer feeds it `PaletteItem`s built from the
//! session list, the static page nav, and shell actions; `rank`
//! filters + orders them for the listbox. Matching is deliberately
//! simple: case-insensitive substring on the title (primary) then the
//! hint (secondary — cwd, agent, key hint).

/// What kind of target a palette row activates — drives the row's
/// icon and the hint text the UI prepends ("Go to", "Run").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaletteKind {
    /// Jump to a session (`target` is the `/?session=…` href).
    #[default]
    Session,
    /// Navigate to a top-level page (`target` is the route path).
    Page,
    /// Run a shell action (`target` is an `action:<name>` id).
    Action,
}

/// One selectable row in the palette.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaletteItem {
    pub kind: PaletteKind,
    /// Opaque activation target — a route href or `action:<name>` id;
    /// the DOM layer interprets it, scoring never reads it.
    pub target: String,
    /// Primary label — session title, page name, action verb.
    pub title: String,
    /// Secondary text — matched after the title and rendered dimmed
    /// (cwd, agent id, keyboard hint).
    pub hint: String,
}

impl PaletteItem {
    pub fn session(
        href: impl Into<String>,
        title: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            kind: PaletteKind::Session,
            target: href.into(),
            title: title.into(),
            hint: hint.into(),
        }
    }

    pub fn page(path: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind: PaletteKind::Page,
            target: path.into(),
            title: title.into(),
            hint: "Go to page".into(),
        }
    }

    pub fn action(
        id: impl Into<String>,
        title: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            kind: PaletteKind::Action,
            target: format!("action:{}", id.into()),
            title: title.into(),
            hint: hint.into(),
        }
    }
}

/// Score one item against an already-lowercased, trimmed query.
/// Lower is better; `None` = not a match.
///
/// - title prefix match: `position 0` band — the strongest signal.
/// - title substring: ranked by match position (earlier = better).
/// - hint substring: weaker band — the hint supports, never leads.
fn score(query: &str, item: &PaletteItem) -> Option<u32> {
    if query.is_empty() {
        // Empty query lists everything in authored order.
        return Some(0);
    }
    let title = item.title.to_lowercase();
    if title.starts_with(query) {
        // Shorter titles rank first among prefix hits.
        return Some(u32::try_from(title.len() - query.len()).unwrap_or(u32::MAX));
    }
    if let Some(pos) = title.find(query) {
        return Some(1_000 + u32::try_from(pos).unwrap_or(u32::MAX));
    }
    let hint = item.hint.to_lowercase();
    if let Some(pos) = hint.find(query) {
        return Some(2_000 + u32::try_from(pos).unwrap_or(u32::MAX));
    }
    None
}

/// Filter + order `items` for the listbox. Empty/whitespace queries
/// pass everything through in authored order (pages/actions first —
/// the caller decides that order). The sort is stable, so equal
/// scores keep their authored position too.
pub fn rank<'a>(query: &str, items: &'a [PaletteItem]) -> Vec<&'a PaletteItem> {
    let query = query.trim().to_lowercase();
    let mut scored: Vec<(u32, &PaletteItem)> = items
        .iter()
        .filter_map(|item| score(&query, item).map(|s| (s, item)))
        .collect();
    scored.sort_by_key(|(s, _)| *s);
    scored.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PaletteItem> {
        vec![
            PaletteItem::page("/", "Sessions"),
            PaletteItem::page("/settings", "Settings"),
            PaletteItem::action("theme", "Toggle theme", "appearance"),
            PaletteItem::session("/?session=s1", "Fix flaky login spec", "/work/api · devin"),
            PaletteItem::session("/?session=s2", "Docs sweep", "/work/docs · claude"),
        ]
    }

    fn titles<'a>(ranked: &[&'a PaletteItem]) -> Vec<&'a str> {
        ranked.iter().map(|i| i.title.as_str()).collect()
    }

    #[test]
    fn empty_query_returns_everything_in_order() {
        let items = items();
        let ranked = rank("", &items);
        assert_eq!(ranked.len(), 5);
        assert_eq!(ranked[0].title, "Sessions");
        let ranked = rank("   ", &items);
        assert_eq!(ranked.len(), 5);
    }

    #[test]
    fn substring_matches_title_case_insensitively() {
        let items = items();
        let ranked = rank("DOCS", &items);
        assert_eq!(titles(&ranked), vec!["Docs sweep"]);
    }

    #[test]
    fn prefix_beats_substring() {
        let list = vec![
            PaletteItem::session("/?session=a", "the fix", ""),
            PaletteItem::session("/?session=b", "fix later", ""),
        ];
        let ranked = rank("fix", &list);
        assert_eq!(titles(&ranked), vec!["fix later", "the fix"]);
    }

    #[test]
    fn hint_matches_after_title() {
        // "devin" only appears in s1's hint; "login" only in s1's
        // title — a hint hit still surfaces the row.
        let items = items();
        let ranked = rank("devin", &items);
        assert_eq!(titles(&ranked), vec!["Fix flaky login spec"]);
    }

    #[test]
    fn non_matching_query_returns_empty() {
        let items = items();
        assert!(rank("zzz", &items).is_empty());
        assert!(rank("x", &[]).is_empty());
    }

    #[test]
    fn shorter_prefix_hit_ranks_first() {
        let list = vec![
            PaletteItem::page("/a", "Settings"),
            PaletteItem::page("/b", "Set"),
        ];
        let ranked = rank("set", &list);
        assert_eq!(titles(&ranked), vec!["Set", "Settings"]);
    }
}
