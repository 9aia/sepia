//! Keyboard math for `role=menu` popups — the pure half of the DOM
//! wiring in `sepia-web` (`pages::session_list` context menus).
//! Menus use `aria-activedescendant` — the same pattern as the
//! command palette: the menu container keeps DOM focus and a signal
//! tracks the active descendant's index across *all* rendered items,
//! so disabled slots keep stable ids while these functions skip them
//! (an `enabled[i]` of `false` marks a `disabled`/`aria-disabled`
//! item). Nothing here touches the DOM; the same code compiles for
//! SSR and wasm.

use crate::keymap::NavDir;

/// Which end of a menu `Home`/`End` jumps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// `Home` — the first enabled item.
    First,
    /// `End` — the last enabled item.
    Last,
}

/// What one keydown inside an open menu means — `sepia-web` maps the
/// result onto its active-descendant signal + DOM focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    /// `ArrowUp`/`ArrowDown` — move the active descendant (wrapping,
    /// skipping disabled).
    Move(NavDir),
    /// `Home`/`End` — jump to the first/last enabled item.
    Jump(Edge),
    /// `Enter`/`Space` — activate the active item.
    Activate,
    /// `Escape` — close; focus returns to the trigger.
    Close,
    /// `Tab` — close, but let the press through so focus moves on
    /// (menus don't trap Tab).
    Dismiss,
    /// A bare printable char — jump to the next item starting with
    /// it (single-keystroke typeahead).
    Type(char),
    /// Anything else — let it bubble.
    Ignore,
}

/// Map `KeyboardEvent.key` to a menu action. Modifier chords are the
/// caller's job — check ctrl/meta/alt first and skip `classify` so
/// `⌘K` & friends still reach the page instead of typeahead.
pub fn classify(key: &str) -> MenuKey {
    match key {
        "ArrowDown" => MenuKey::Move(NavDir::Down),
        "ArrowUp" => MenuKey::Move(NavDir::Up),
        "Home" => MenuKey::Jump(Edge::First),
        "End" => MenuKey::Jump(Edge::Last),
        "Enter" | " " => MenuKey::Activate,
        "Escape" => MenuKey::Close,
        "Tab" => MenuKey::Dismiss,
        _ => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_control() => MenuKey::Type(c),
                _ => MenuKey::Ignore,
            }
        }
    }
}

/// The index one step in `dir` lands on, skipping `!enabled` slots
/// and wrapping at the ends. `current == None` (or a stale index)
/// starts from the matching edge: Down → first enabled, Up → last
/// enabled. `None` when nothing is enabled.
pub fn move_active(enabled: &[bool], current: Option<usize>, dir: NavDir) -> Option<usize> {
    let n = enabled.len();
    if !enabled.iter().any(|e| *e) {
        return None;
    }
    let start = current.filter(|&i| i < n);
    for step in 1..=n {
        let i = match (start, dir) {
            (Some(c), NavDir::Down) => (c + step) % n,
            (Some(c), NavDir::Up) => (c + n - step) % n,
            (None, NavDir::Down) => step - 1,
            (None, NavDir::Up) => n - step,
        };
        if enabled[i] {
            return Some(i);
        }
    }
    None
}

/// The first (`Home`) or last (`End`) enabled index — `None` when
/// nothing is enabled. Also used on open for the initial active
/// descendant.
pub fn edge(enabled: &[bool], which: Edge) -> Option<usize> {
    match which {
        Edge::First => enabled.iter().position(|e| *e),
        Edge::Last => enabled.iter().rposition(|e| *e),
    }
}

/// Next enabled item whose label starts with `c` (case-insensitive,
/// leading whitespace trimmed), scanning forward from `current` and
/// wrapping — single-keystroke typeahead, so repeating the char
/// cycles through same-letter items. `None` scans from the top;
/// `None` returned when nothing matches.
pub fn typeahead(
    labels: &[String],
    enabled: &[bool],
    c: char,
    current: Option<usize>,
) -> Option<usize> {
    let n = labels.len().min(enabled.len());
    if n == 0 {
        return None;
    }
    let start = current.filter(|&i| i < n).map_or(0, |i| (i + 1) % n);
    (0..n).map(|off| (start + off) % n).find(|&i| {
        enabled[i]
            && labels[i]
                .trim_start()
                .chars()
                .next()
                .is_some_and(|first| first.to_lowercase().eq(c.to_lowercase()))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const ALL: [bool; 4] = [true, true, true, true];

    #[test]
    fn classify_maps_the_menu_keys() {
        assert_eq!(classify("ArrowDown"), MenuKey::Move(NavDir::Down));
        assert_eq!(classify("ArrowUp"), MenuKey::Move(NavDir::Up));
        assert_eq!(classify("Home"), MenuKey::Jump(Edge::First));
        assert_eq!(classify("End"), MenuKey::Jump(Edge::Last));
        assert_eq!(classify("Enter"), MenuKey::Activate);
        assert_eq!(classify(" "), MenuKey::Activate);
        assert_eq!(classify("Escape"), MenuKey::Close);
        assert_eq!(classify("Tab"), MenuKey::Dismiss);
        assert_eq!(classify("d"), MenuKey::Type('d'));
        assert_eq!(classify("F2"), MenuKey::Ignore);
        assert_eq!(classify("Unidentified"), MenuKey::Ignore);
    }

    #[test]
    fn arrows_wrap_both_ways() {
        assert_eq!(move_active(&ALL, Some(0), NavDir::Down), Some(1));
        assert_eq!(move_active(&ALL, Some(3), NavDir::Down), Some(0));
        assert_eq!(move_active(&ALL, Some(0), NavDir::Up), Some(3));
    }

    #[test]
    fn arrows_skip_disabled_items() {
        let mask = [true, false, false, true];
        assert_eq!(move_active(&mask, Some(0), NavDir::Down), Some(3));
        assert_eq!(move_active(&mask, Some(3), NavDir::Up), Some(0));
    }

    #[test]
    fn no_active_starts_at_the_matching_edge() {
        let mask = [false, true, true, false];
        assert_eq!(move_active(&mask, None, NavDir::Down), Some(1));
        assert_eq!(move_active(&mask, None, NavDir::Up), Some(2));
    }

    #[test]
    fn a_stale_active_index_is_treated_as_none() {
        assert_eq!(move_active(&ALL, Some(9), NavDir::Down), Some(0));
    }

    #[test]
    fn all_disabled_never_moves() {
        let mask = [false, false];
        assert_eq!(move_active(&mask, Some(0), NavDir::Down), None);
        assert_eq!(edge(&mask, Edge::First), None);
    }

    #[test]
    fn home_and_end_land_on_enabled_edges() {
        let mask = [false, true, true, false];
        assert_eq!(edge(&mask, Edge::First), Some(1));
        assert_eq!(edge(&mask, Edge::Last), Some(2));
        assert_eq!(edge(&ALL, Edge::Last), Some(3));
    }

    #[test]
    fn typeahead_jumps_forward_and_wraps() {
        let labels: Vec<String> = ["Open", "Pin", "Delete…", "Details"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        // From "Open" the first 'd' lands on "Delete…"…
        assert_eq!(typeahead(&labels, &ALL, 'd', Some(0)), Some(2));
        // …and pressing 'd' again cycles to "Details", then wraps.
        assert_eq!(typeahead(&labels, &ALL, 'd', Some(2)), Some(3));
        assert_eq!(typeahead(&labels, &ALL, 'd', Some(3)), Some(2));
    }

    #[test]
    fn typeahead_is_case_insensitive_and_skips_disabled() {
        let labels: Vec<String> = ["open", "Pin", "details"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(typeahead(&labels, &ALL, 'D', None), Some(2));
        let mask = [true, true, false];
        assert_eq!(typeahead(&labels, &mask, 'd', None), None);
    }

    #[test]
    fn typeahead_handles_leading_whitespace() {
        let labels: Vec<String> = vec!["  Archive".to_string(), "Pin".to_string()];
        assert_eq!(typeahead(&labels, &ALL, 'a', None), Some(0));
    }
}
