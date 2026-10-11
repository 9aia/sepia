//! Focus-trap arithmetic — the pure half of the overlay Tab trap in
//! `sepia-web` (`components::track_overlay_focus`). The DOM layer
//! re-queries live focusables on every Tab press (overlay contents
//! mutate while open — palette results stream in async) and hands the
//! index math to these functions; nothing here touches the DOM, so
//! the same code compiles for SSR and wasm.

/// `querySelectorAll` selector for trap-eligible elements, applied to
/// the overlay's panel element. Disabled controls are excluded via
/// `:disabled`; `tabindex="-1"` stays programmatic-only so the panel
/// itself (which carries it) never joins the cycle. `display:none`
/// subtrees are filtered DOM-side by their layout box — no selector
/// can express that.
pub const FOCUSABLE_SELECTOR: &str = concat!(
    "a[href], area[href], button:not(:disabled), input:not(:disabled), ",
    "select:not(:disabled), textarea:not(:disabled), summary, ",
    "iframe, object, embed, audio[controls], video[controls], ",
    "[contenteditable]:not([contenteditable=\"false\"]), ",
    "[tabindex]:not([tabindex=\"-1\"])"
);

/// Which way Tab is travelling — whether `Shift` is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabDir {
    /// Plain `Tab` — advance toward the end of the list.
    Forward,
    /// `Shift+Tab` — retreat toward the start.
    Backward,
}

/// Index the next Tab should land on inside a trap.
///
/// `current` is the focused element's position within the live
/// focusable list, `None` when focus sits outside the trap (the panel
/// itself, the page behind it, browser chrome). Returns `None` when
/// nothing is focusable — the DOM layer focuses the container itself.
/// Every `Some` result wraps, so Tab can never leave the overlay.
pub fn tab_target(current: Option<usize>, len: usize, dir: TabDir) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (current, dir) {
        (None, TabDir::Forward) => 0,
        (None, TabDir::Backward) => len - 1,
        (Some(i), TabDir::Forward) => (i + 1) % len,
        (Some(i), TabDir::Backward) => i.checked_sub(1).unwrap_or(len - 1),
    })
}

/// Element to focus when an overlay opens — an explicit
/// `data-autofocus` mark wins (passed as its index within the
/// focusable list), else the first focusable. `None` when nothing is
/// focusable → the DOM layer focuses the container itself.
pub fn initial_target(autofocus: Option<usize>, len: usize) -> Option<usize> {
    autofocus.filter(|&i| i < len).or((len > 0).then_some(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_wraps_forward_past_the_last_item() {
        assert_eq!(tab_target(Some(2), 3, TabDir::Forward), Some(0));
    }

    #[test]
    fn shift_tab_wraps_backward_past_the_first_item() {
        assert_eq!(tab_target(Some(0), 3, TabDir::Backward), Some(2));
    }

    #[test]
    fn mid_list_tabs_step_normally() {
        assert_eq!(tab_target(Some(0), 3, TabDir::Forward), Some(1));
        assert_eq!(tab_target(Some(2), 3, TabDir::Backward), Some(1));
    }

    #[test]
    fn focus_outside_the_trap_enters_at_an_edge() {
        // Backdrop click, address bar, the panel itself — Tab re-enters
        // at the first item, Shift+Tab at the last.
        assert_eq!(tab_target(None, 3, TabDir::Forward), Some(0));
        assert_eq!(tab_target(None, 3, TabDir::Backward), Some(2));
    }

    #[test]
    fn empty_focusable_list_defers_to_the_container() {
        assert_eq!(tab_target(Some(0), 0, TabDir::Forward), None);
        assert_eq!(tab_target(None, 0, TabDir::Backward), None);
    }

    #[test]
    fn a_single_item_cycles_onto_itself() {
        assert_eq!(tab_target(Some(0), 1, TabDir::Forward), Some(0));
        assert_eq!(tab_target(Some(0), 1, TabDir::Backward), Some(0));
    }

    #[test]
    fn autofocus_wins_on_open() {
        assert_eq!(initial_target(Some(1), 3), Some(1));
    }

    #[test]
    fn no_autofocus_falls_back_to_the_first_item() {
        assert_eq!(initial_target(None, 3), Some(0));
    }

    #[test]
    fn a_stale_autofocus_index_falls_back_to_the_first_item() {
        // The marked element left the DOM between mount and focus.
        assert_eq!(initial_target(Some(9), 3), Some(0));
    }

    #[test]
    fn an_empty_dialog_focuses_the_container() {
        assert_eq!(initial_target(Some(0), 0), None);
        assert_eq!(initial_target(None, 0), None);
    }
}
