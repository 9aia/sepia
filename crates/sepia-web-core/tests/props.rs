//! Property tests — the list pipeline's invariants hold for arbitrary
//! inputs: filtering never fabricates rows, sorts are real sorts, and
//! grouping is a partition.

use std::collections::BTreeMap;

use proptest::prelude::*;
use sepia_web_core::filter::{
    ALL_AGENTS, CollapsedGroups, Group, SessionFilter, SessionRow, SortMode, StatusFilter,
    group_by_cwd, visible_keys,
};
use sepia_web_core::keymap::{self, Action, KeyCtx, Mods, NavDir, nav_index};
use sepia_web_core::theme::Theme;

/// Small pools so collisions (same id, same cwd) actually happen.
fn arb_row() -> impl Strategy<Value = SessionRow> {
    (
        prop::sample::select(vec!["a", "b", "c", "d", "e"]),
        prop::sample::select(vec!["", "fix auth", "Docs", "API work", "zzz"]),
        prop::sample::select(vec!["", "/home/u/app", "/var/www", "/tmp/x"]),
        prop::sample::select(vec!["", "claude", "cline", "cursor"]),
        prop::sample::select(vec![
            "2026-10-06T10:00:00Z",
            "2026-10-08T10:00:00Z",
            "2026-10-09T10:00:00Z",
            "garbage",
            "",
        ]),
        any::<bool>(),
    )
        .prop_map(|(id, title, cwd, agent, updated_at, locked)| SessionRow {
            id: id.into(),
            title: title.into(),
            cwd: cwd.into(),
            agent: agent.into(),
            updated_at: updated_at.into(),
            locked,
            ..SessionRow::default()
        })
}

fn arb_filter() -> impl Strategy<Value = SessionFilter> {
    (
        prop::sample::select(vec!["", "  ", "fix", "DOCS", "app", "zzz"]),
        prop::sample::select(vec![
            StatusFilter::All,
            StatusFilter::Free,
            StatusFilter::Locked,
        ]),
        prop::sample::select(vec!["all", "claude", "cline", "ghost", ""]),
        prop::sample::select(vec![SortMode::Newest, SortMode::Oldest, SortMode::Title]),
    )
        .prop_map(|(query, status, agent, sort)| SessionFilter {
            query: query.into(),
            status,
            agent: agent.into(),
            sort,
        })
}

fn arb_ctx() -> impl Strategy<Value = KeyCtx> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(typing, filter_focused, menu_open, help_open)| KeyCtx {
            typing,
            filter_focused,
            menu_open,
            help_open,
        },
    )
}

fn arb_mods() -> impl Strategy<Value = Mods> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(ctrl, meta, alt, shift)| Mods {
            ctrl,
            meta,
            alt,
            shift,
        },
    )
}

/// Multiset of `(id, agent)` — the rows' identity for these tests.
fn multiset(rows: &[SessionRow]) -> BTreeMap<(String, String), usize> {
    let mut m = BTreeMap::new();
    for r in rows {
        *m.entry((r.id.clone(), r.agent.clone())).or_insert(0) += 1;
    }
    m
}

proptest! {
    /// A permissive filter is a permutation of the input — nothing
    /// lost, nothing duplicated.
    #[test]
    fn permissive_filter_loses_nothing(rows in prop::collection::vec(arb_row(), 0..24)) {
        let out = SessionFilter::default().apply(rows.clone());
        prop_assert_eq!(multiset(&out), multiset(&rows));
    }

    /// Any filter output is a sub-multiset of the input, and every
    /// kept row satisfies the predicate.
    #[test]
    fn filter_only_drops(rows in prop::collection::vec(arb_row(), 0..24), f in arb_filter()) {
        let out = f.apply(rows.clone());
        let mut available = multiset(&rows);
        for r in &out {
            let key = (r.id.clone(), r.agent.clone());
            let n = available.get_mut(&key).copied().unwrap_or(0);
            prop_assert!(n > 0, "fabricated row {key:?}");
            available.insert(key, n - 1);
        }
        let q = f.query.trim().to_lowercase();
        for r in &out {
            prop_assert!(
                q.is_empty()
                    || r.title.to_lowercase().contains(&q)
                    || r.cwd.to_lowercase().contains(&q)
            );
            let status_ok = match f.status {
                StatusFilter::All => true,
                StatusFilter::Free => !r.locked,
                StatusFilter::Locked => r.locked,
            };
            prop_assert!(status_ok);
            prop_assert!(f.agent == ALL_AGENTS || r.agent == f.agent);
        }
    }

    /// The output honors the requested order.
    #[test]
    fn output_is_sorted(rows in prop::collection::vec(arb_row(), 0..24), sort in prop::sample::select(vec![SortMode::Newest, SortMode::Oldest, SortMode::Title])) {
        let f = SessionFilter { sort, ..SessionFilter::default() };
        let out = f.apply(rows);
        for w in out.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let ok = match sort {
                SortMode::Newest => a.updated_at >= b.updated_at,
                SortMode::Oldest => a.updated_at <= b.updated_at,
                SortMode::Title => {
                    a.title.to_lowercase() < b.title.to_lowercase()
                        || (a.title.to_lowercase() == b.title.to_lowercase()
                            && a.updated_at >= b.updated_at)
                }
            };
            prop_assert!(ok, "{a:?} before {b:?} under {sort:?}");
        }
    }

    /// Grouping is a partition of the filtered rows: every row lands in
    /// exactly one group, group keys are unique, the empty-cwd group
    /// trails, and within-group order matches the input order.
    #[test]
    fn grouping_partitions(rows in prop::collection::vec(arb_row(), 0..24), f in arb_filter()) {
        let filtered = f.apply(rows);
        let groups = group_by_cwd(filtered.clone());
        // Partition — same multiset of rows.
        let flat: Vec<SessionRow> = groups.iter().flat_map(|g| g.rows.clone()).collect();
        prop_assert_eq!(multiset(&flat), multiset(&filtered));
        // Keys unique; empty key only last.
        let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
        let mut dedup = keys.clone();
        dedup.sort_unstable();
        dedup.dedup();
        prop_assert_eq!(dedup.len(), keys.len());
        if let Some(pos) = keys.iter().position(|k| k.is_empty()) {
            prop_assert_eq!(pos, keys.len() - 1);
        }
        // Within each group, rows keep the filtered order.
        for g in &groups {
            let mut it = filtered.iter().filter(|r| r.cwd == g.key);
            prop_assert!(g.rows.iter().all(|r| it.next() == Some(r)));
        }
        // Named groups ordered by newest descendant…
        let newest: Vec<&str> = groups
            .iter()
            .filter(|g| !g.key.is_empty())
            .map(|g| g.rows.iter().map(|r| r.updated_at.as_str()).max().unwrap_or_default())
            .collect();
        prop_assert!(newest.windows(2).all(|w| w[0] >= w[1]));
    }

    /// Arrow-key order covers exactly the open sections' rows.
    #[test]
    fn visible_keys_matches_open_sections(
        rows in prop::collection::vec(arb_row(), 0..24),
        closed in prop::collection::vec(prop::sample::select(vec!["", "/home/u/app", "/var/www", "/tmp/x"]), 0..3),
        force_open in any::<bool>(),
    ) {
        let groups = group_by_cwd(SessionFilter::default().apply(rows));
        let mut collapsed = CollapsedGroups::default();
        for c in closed {
            collapsed.toggle(c);
        }
        let keys = visible_keys(&groups, &collapsed, force_open);
        let expect: Vec<(String, String)> = groups
            .iter()
            .filter(|g: &&Group| force_open || !collapsed.is_closed(&g.key))
            .flat_map(|g| g.rows.iter().map(|r| (r.id.clone(), r.agent.clone())))
            .collect();
        prop_assert_eq!(keys, expect);
    }

    /// `resolve_key` never panics and obeys the structural rules:
    /// editable targets swallow plain keys; command chords ignore the
    /// typing flag; Escape always resolves to a close action.
    #[test]
    fn resolve_key_invariants(key in "\\PC{0,8}", mods in arb_mods(), ctx in arb_ctx()) {
        let action = keymap::resolve_key(&key, mods, ctx);
        // Editable targets swallow everything except Escape and the
        // command chords.
        if ctx.typing && !mods.command() && key != "Escape" {
            prop_assert_eq!(action, None);
            return Ok(());
        }
        let Some(action) = action else {
            return Ok(());
        };
        if key == "Escape" {
            prop_assert!(matches!(
                action,
                Action::CloseMenu | Action::ClearFilter | Action::CloseHelp | Action::CloseSelection
            ));
        }
        // Beyond k/b chords and Escape, command/alt modifiers swallow
        // the key entirely.
        if (mods.command() || mods.alt)
            && !key.eq_ignore_ascii_case("k")
            && !key.eq_ignore_ascii_case("b")
            && key != "Escape"
        {
            prop_assert!(false, "modifier chord produced {action:?} for {key:?}");
        }
        if action == Action::ToggleHelp {
            prop_assert_eq!(key, "?");
        }
    }

    /// `nav_index` stays in bounds and steps exactly ±1 (mod len).
    #[test]
    fn nav_index_stays_in_bounds(cur in prop::option::of(0usize..10), len in 0usize..10, up in any::<bool>()) {
        let dir = if up { NavDir::Up } else { NavDir::Down };
        match nav_index(cur, len, dir) {
            None => prop_assert_eq!(len, 0),
            Some(i) => {
                prop_assert!(i < len);
                if let Some(c) = cur.filter(|c| *c < len) {
                    prop_assert_eq!(i, if up { (c + len - 1) % len } else { (c + 1) % len });
                }
            }
        }
    }

    /// Storage decode is idempotent — re-encoding a decoded value is a
    /// fixed point — and `next()` is a 3-cycle permutation.
    #[test]
    fn theme_round_trips(s in "\\PC{0,12}") {
        let t = Theme::from_stored(&s);
        prop_assert_eq!(Theme::from_stored(t.stored()), t);
        prop_assert_eq!(t.next().next().next(), t);
    }
}
