//! Scrollspy math for the settings page's section nav — the pure
//! half. The DOM side in `sepia-web` observes `[data-spy]` sections
//! with an `IntersectionObserver` banded to the top ~30% of the
//! scrollport (the old app's `0px 0px -70% 0px` root margin) and
//! hands us the intersecting ids.
//!
//! The rule mirrors the old `SettingsDialog`: the entry the observer
//! last reported intersecting wins. Between callbacks every section
//! can sit outside the band (fast scrolls, tall sections), so a
//! report with no intersecting entry means "keep the previous pick" —
//! `None` — rather than blanking the nav.

/// Pick the highlighted section, or `None` to keep the previous pick.
/// `order` is the sections' document order; `intersecting` the ids an
/// observer batch reported intersecting. When several sections share
/// the band (short sections back-to-back), the deepest one in
/// document order wins — it owns the band's leading edge next.
pub fn active<'a>(order: &[&'a str], intersecting: &[&str]) -> Option<&'a str> {
    order.iter().rfind(|id| intersecting.contains(id)).copied()
}

/// Whether a nav item for `id` renders the active style.
pub fn is_active(active: &str, id: &str) -> bool {
    active == id
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: [&str; 4] = ["a", "b", "c", "d"];

    #[test]
    fn single_intersecting_section_wins() {
        assert_eq!(active(&ORDER, &["b"]), Some("b"));
        assert_eq!(active(&ORDER, &["d"]), Some("d"));
    }

    #[test]
    fn several_intersecting_picks_the_deepest() {
        // Two short sections share the band — the later one owns the
        // band's leading edge next, matching the old "last entry
        // wins" callback.
        assert_eq!(active(&ORDER, &["a", "b"]), Some("b"));
        assert_eq!(active(&ORDER, &["c", "a"]), Some("c"));
    }

    #[test]
    fn empty_report_keeps_the_current_pick() {
        assert_eq!(active(&ORDER, &[]), None);
        // An intersecting id outside the order can't be picked.
        assert_eq!(active(&ORDER, &["zzz"]), None);
    }

    #[test]
    fn is_active_is_identity() {
        assert!(is_active("b", "b"));
        assert!(!is_active("b", "c"));
    }
}
