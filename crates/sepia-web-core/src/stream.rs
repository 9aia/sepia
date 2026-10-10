//! Live-stream connection display logic — the pure half of
//! `session_detail`'s `ReconnectingPill`. Kept DOM-free so the policy
//! is unit-testable: the UI feeds it open/closed samples and reads
//! back whether the pill should show.

/// Decides when a silently-reconnecting `EventSource` deserves a
/// visible "Reconnecting…" affordance. The rule: an outage is only
/// worth surfacing once the stream has been `Open` at least once —
/// the initial connect is quiet (the summary already implies the
/// stream is warming up, and a brief `Connecting` flicker on page
/// load would be noise), and a detached session has no stream to
/// report at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconnectTracker {
    seen_open: bool,
}

impl ReconnectTracker {
    /// Observe a ready-state sample. `open` is `true` only for the
    /// fully-connected state; `Connecting`/`Closing`/`Closed` all
    /// count as not-open. Returns `true` while the pill should show.
    pub fn observe(&mut self, open: bool) -> bool {
        if open {
            self.seen_open = true;
        }
        self.seen_open && !open
    }

    /// Forget the history — call when a *new* stream replaces the old
    /// one (re-attach after detach), so its own initial connect is
    /// quiet too.
    pub fn reset(&mut self) {
        self.seen_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_connect_stays_quiet() {
        let mut t = ReconnectTracker::default();
        // A stream that never opened — connecting or retrying its
        // first handshake — shows nothing.
        assert!(!t.observe(false));
        assert!(!t.observe(false));
    }

    #[test]
    fn post_open_outage_shows() {
        let mut t = ReconnectTracker::default();
        assert!(!t.observe(true)); // open → no pill
        assert!(t.observe(false)); // dropped → pill
        assert!(!t.observe(true)); // recovered → pill clears
    }

    #[test]
    fn reset_quiets_the_next_stream() {
        let mut t = ReconnectTracker::default();
        t.observe(true);
        assert!(t.observe(false));
        t.reset();
        assert!(!t.observe(false));
        assert!(!t.observe(true));
        assert!(t.observe(false));
    }
}
