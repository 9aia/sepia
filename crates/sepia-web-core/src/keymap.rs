//! Global hotkey resolution — the pure key→action mapping behind the
//! document `keydown` listeners in `app` and `pages::session_list`.
//! Each listener translates its `KeyboardEvent` into `(key, Mods,
//! KeyCtx)`, fills in the context it owns, and dispatches only the
//! `Action`s in its scope (the shell owns `ToggleHelp`/`CloseHelp`/
//! `TogglePalette`/`OpenSettings`; the session list owns the rest).

/// Modifier state for a keydown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub meta: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    /// ⌘ or Ctrl — the "command" chord modifier.
    pub fn command(self) -> bool {
        self.ctrl || self.meta
    }
}

/// What the surrounding UI looks like when the key lands. Listeners
/// fill in the flags they own; unknowns default to `false`, which
/// keeps each listener's resolution honest (e.g. the shell can't see
/// the row menu, so its Escape resolves to `CloseHelp`/`CloseSelection`
/// — and it only acts on `CloseHelp`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyCtx {
    /// The event target is a text-entry element (input, textarea,
    /// select, or contenteditable) — plain keys like `n`, `?`, and the
    /// arrows stay out of it.
    pub typing: bool,
    /// The session filter input has focus.
    pub filter_focused: bool,
    /// The row context menu is open.
    pub menu_open: bool,
    /// The `?` cheat-sheet is open.
    pub help_open: bool,
}

/// What a resolved keypress should do. The DOM layer performs it —
/// focus, navigation, signal writes stay in `sepia-web`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// `?` — open/close the shortcut cheat-sheet (shell).
    ToggleHelp,
    /// Escape while the cheat-sheet is open (shell).
    CloseHelp,
    /// ⌘K / Ctrl-K or `/` — open (⌘K also closes) the command palette.
    TogglePalette,
    /// ⌘B / Ctrl-B — collapse/expand the list column.
    ToggleList,
    /// `n` — focus the new-session cwd input.
    FocusNewSession,
    /// ArrowDown — move `?session=` to the next visible row.
    NavNext,
    /// ArrowUp — move `?session=` to the previous visible row.
    NavPrev,
    /// Escape with the row menu open — closes it first.
    CloseMenu,
    /// Escape with the filter focused — clear it and blur.
    ClearFilter,
    /// Escape otherwise — drop `?session=` (back to `/`).
    CloseSelection,
    /// ⌘, / Ctrl-, — open `/settings` (shell).
    OpenSettings,
    /// ArrowLeft — fold the selected row's section, or every section
    /// when nothing is selected.
    FoldGroup,
    /// ArrowRight — unfold the selected row's section, or all.
    UnfoldGroup,
}

impl Action {
    /// Whether the DOM handler should `prevent_default()` the event —
    /// the chords and single-letter keys need it (they'd otherwise type
    /// or trigger the browser's own binding), and the fold arrows keep
    /// the list container from scrolling sideways. Escape and the
    /// up/down nav arrows keep their default (page scroll).
    pub fn prevent_default(self) -> bool {
        matches!(
            self,
            Self::ToggleHelp
                | Self::TogglePalette
                | Self::ToggleList
                | Self::FocusNewSession
                | Self::OpenSettings
                | Self::FoldGroup
                | Self::UnfoldGroup
        )
    }
}

/// Resolve one keydown to an action, or `None` to leave it alone.
///
/// Precedence mirrors the original handlers: command chords first
/// (they fire even while typing), then Escape (which never checks the
/// typing flag — the Escape cascade is menu → filter → help →
/// selection), then the plain keys that editable targets swallow.
pub fn resolve_key(key: &str, mods: Mods, ctx: KeyCtx) -> Option<Action> {
    if mods.command() {
        if key.eq_ignore_ascii_case("k") {
            return Some(Action::TogglePalette);
        }
        if key.eq_ignore_ascii_case("b") {
            return Some(Action::ToggleList);
        }
        // `ev.key()` for ⌘, is "," on both layouts.
        if key == "," {
            return Some(Action::OpenSettings);
        }
    }
    if key == "Escape" {
        return Some(if ctx.menu_open {
            Action::CloseMenu
        } else if ctx.filter_focused {
            Action::ClearFilter
        } else if ctx.help_open {
            Action::CloseHelp
        } else {
            Action::CloseSelection
        });
    }
    if mods.command() || mods.alt {
        return None;
    }
    if key == "?" {
        // Shift allowed — it's how `?` is typed.
        return (!ctx.typing).then_some(Action::ToggleHelp);
    }
    if key == "/" && !mods.shift {
        // `/` is the palette's second opener — needs no shift.
        return (!ctx.typing).then_some(Action::TogglePalette);
    }
    if ctx.typing {
        return None;
    }
    if !mods.shift && key.eq_ignore_ascii_case("n") {
        return Some(Action::FocusNewSession);
    }
    match key {
        "ArrowDown" => Some(Action::NavNext),
        "ArrowUp" => Some(Action::NavPrev),
        "ArrowLeft" => Some(Action::FoldGroup),
        "ArrowRight" => Some(Action::UnfoldGroup),
        _ => None,
    }
}

/// Arrow direction for session cycling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDir {
    Up,
    Down,
}

/// The index `dir` lands on in a `len`-row visible order — wraps at
/// the ends; with no current index, Down starts at the first row and
/// Up at the last. `None` for an empty order.
pub fn nav_index(cur: Option<usize>, len: usize, dir: NavDir) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (dir, cur) {
        (NavDir::Down, Some(i)) => (i + 1) % len,
        (NavDir::Up, Some(i)) => (i + len - 1) % len,
        (NavDir::Down, None) => 0,
        (NavDir::Up, None) => len - 1,
    })
}

#[cfg(test)]
mod tests {
    // The `mods`/`ctx` fixture helpers take four flags each — clearer
    // here than a builder.
    #![allow(clippy::fn_params_excessive_bools)]
    use super::*;

    const NONE: KeyCtx = KeyCtx {
        typing: false,
        filter_focused: false,
        menu_open: false,
        help_open: false,
    };
    const NO_MODS: Mods = Mods {
        ctrl: false,
        meta: false,
        alt: false,
        shift: false,
    };

    fn mods(ctrl: bool, meta: bool, alt: bool, shift: bool) -> Mods {
        Mods {
            ctrl,
            meta,
            alt,
            shift,
        }
    }

    fn ctx(typing: bool, filter_focused: bool, menu_open: bool, help_open: bool) -> KeyCtx {
        KeyCtx {
            typing,
            filter_focused,
            menu_open,
            help_open,
        }
    }

    #[test]
    fn command_k_toggles_the_palette() {
        assert_eq!(
            resolve_key("k", mods(false, true, false, false), NONE),
            Some(Action::TogglePalette)
        );
        assert_eq!(
            resolve_key("K", mods(true, false, false, false), NONE),
            Some(Action::TogglePalette)
        );
        // Works while typing — ⌘K must close the palette even with
        // its input focused.
        assert_eq!(
            resolve_key(
                "k",
                mods(true, false, false, false),
                ctx(true, false, false, false)
            ),
            Some(Action::TogglePalette)
        );
        // Bare `k` does nothing.
        assert_eq!(resolve_key("k", NO_MODS, NONE), None);
    }

    #[test]
    fn slash_opens_the_palette_unless_typing() {
        assert_eq!(resolve_key("/", NO_MODS, NONE), Some(Action::TogglePalette));
        assert_eq!(
            resolve_key("/", NO_MODS, ctx(true, false, false, false)),
            None
        );
        // `?` is Shift+/ — it resolves to help, not the palette.
        assert_eq!(
            resolve_key("/", mods(false, false, false, true), NONE),
            None
        );
        for m in [
            mods(true, false, false, false),
            mods(false, true, false, false),
            mods(false, false, true, false),
        ] {
            assert_eq!(resolve_key("/", m, NONE), None);
        }
    }

    #[test]
    fn command_b_toggles_the_list() {
        assert_eq!(
            resolve_key("b", mods(false, true, false, false), NONE),
            Some(Action::ToggleList)
        );
        assert_eq!(
            resolve_key("B", mods(true, false, false, false), NONE),
            Some(Action::ToggleList)
        );
        assert_eq!(resolve_key("b", NO_MODS, NONE), None);
    }

    #[test]
    fn escape_cascades_menu_filter_help_selection() {
        assert_eq!(
            resolve_key("Escape", NO_MODS, ctx(false, false, true, false)),
            Some(Action::CloseMenu)
        );
        assert_eq!(
            resolve_key("Escape", NO_MODS, ctx(false, true, true, false)),
            Some(Action::CloseMenu)
        );
        assert_eq!(
            resolve_key("Escape", NO_MODS, ctx(false, true, false, false)),
            Some(Action::ClearFilter)
        );
        assert_eq!(
            resolve_key("Escape", NO_MODS, ctx(false, false, false, true)),
            Some(Action::CloseHelp)
        );
        assert_eq!(
            resolve_key("Escape", NO_MODS, ctx(false, true, false, true)),
            Some(Action::ClearFilter)
        );
        assert_eq!(
            resolve_key("Escape", NO_MODS, NONE),
            Some(Action::CloseSelection)
        );
        // Modifiers and typing never block the Escape cascade.
        assert_eq!(
            resolve_key("Escape", mods(true, false, false, false), NONE),
            Some(Action::CloseSelection)
        );
        assert_eq!(
            resolve_key("Escape", NO_MODS, ctx(true, false, false, true)),
            Some(Action::CloseHelp)
        );
    }

    #[test]
    fn question_mark_toggles_help_unless_typing() {
        assert_eq!(resolve_key("?", NO_MODS, NONE), Some(Action::ToggleHelp));
        // `?` arrives with shift held — allowed.
        assert_eq!(
            resolve_key("?", mods(false, false, false, true), NONE),
            Some(Action::ToggleHelp)
        );
        assert_eq!(
            resolve_key("?", NO_MODS, ctx(true, false, false, false)),
            None
        );
        for m in [
            mods(true, false, false, false),
            mods(false, true, false, false),
            mods(false, false, true, false),
        ] {
            assert_eq!(resolve_key("?", m, NONE), None);
        }
    }

    #[test]
    fn n_focuses_new_session_unless_typing() {
        assert_eq!(
            resolve_key("n", NO_MODS, NONE),
            Some(Action::FocusNewSession)
        );
        assert_eq!(
            resolve_key("n", NO_MODS, ctx(true, false, false, false)),
            None
        );
        // Shift+N types "N" — stays out.
        assert_eq!(
            resolve_key("N", mods(false, false, false, true), NONE),
            None
        );
        for m in [
            mods(true, false, false, false),
            mods(false, true, false, false),
            mods(false, false, true, false),
        ] {
            assert_eq!(resolve_key("n", m, NONE), None);
        }
    }

    #[test]
    fn arrows_navigate_unless_typing() {
        assert_eq!(
            resolve_key("ArrowDown", NO_MODS, NONE),
            Some(Action::NavNext)
        );
        assert_eq!(resolve_key("ArrowUp", NO_MODS, NONE), Some(Action::NavPrev));
        assert_eq!(
            resolve_key("ArrowDown", NO_MODS, ctx(true, false, false, false)),
            None
        );
        // Shift is allowed (arrows aren't text input here).
        assert_eq!(
            resolve_key("ArrowDown", mods(false, false, false, true), NONE),
            Some(Action::NavNext)
        );
        for m in [
            mods(true, false, false, false),
            mods(false, true, false, false),
            mods(false, false, true, false),
        ] {
            assert_eq!(resolve_key("ArrowUp", m, NONE), None);
        }
    }

    #[test]
    fn command_comma_opens_settings() {
        assert_eq!(
            resolve_key(",", mods(false, true, false, false), NONE),
            Some(Action::OpenSettings)
        );
        assert_eq!(
            resolve_key(",", mods(true, false, false, false), NONE),
            Some(Action::OpenSettings)
        );
        // Like the other chords it fires while typing (⌘, is a
        // browser-level shortcut, not text input).
        assert_eq!(
            resolve_key(
                ",",
                mods(true, false, false, false),
                ctx(true, false, false, false)
            ),
            Some(Action::OpenSettings)
        );
        // A bare comma types as usual.
        assert_eq!(resolve_key(",", NO_MODS, NONE), None);
    }

    #[test]
    fn left_right_fold_groups_unless_typing() {
        assert_eq!(
            resolve_key("ArrowLeft", NO_MODS, NONE),
            Some(Action::FoldGroup)
        );
        assert_eq!(
            resolve_key("ArrowRight", NO_MODS, NONE),
            Some(Action::UnfoldGroup)
        );
        assert_eq!(
            resolve_key("ArrowLeft", NO_MODS, ctx(true, false, false, false)),
            None
        );
        // Alt+Left stays the browser's back gesture.
        assert_eq!(
            resolve_key("ArrowLeft", mods(false, false, true, false), NONE),
            None
        );
    }

    #[test]
    fn unknown_keys_resolve_to_none() {
        for key in ["x", "Enter", "Tab", "j", "1"] {
            assert_eq!(resolve_key(key, NO_MODS, NONE), None, "{key}");
        }
    }

    #[test]
    fn prevent_default_matches_the_chords() {
        for (a, pd) in [
            (Action::TogglePalette, true),
            (Action::ToggleList, true),
            (Action::FocusNewSession, true),
            (Action::ToggleHelp, true),
            (Action::CloseMenu, false),
            (Action::ClearFilter, false),
            (Action::CloseHelp, false),
            (Action::CloseSelection, false),
            (Action::NavNext, false),
            (Action::NavPrev, false),
            (Action::OpenSettings, true),
            (Action::FoldGroup, true),
            (Action::UnfoldGroup, true),
        ] {
            assert_eq!(a.prevent_default(), pd, "{a:?}");
        }
    }

    #[test]
    fn nav_index_wraps() {
        assert_eq!(nav_index(Some(0), 3, NavDir::Down), Some(1));
        assert_eq!(nav_index(Some(2), 3, NavDir::Down), Some(0));
        assert_eq!(nav_index(Some(0), 3, NavDir::Up), Some(2));
        assert_eq!(nav_index(None, 3, NavDir::Down), Some(0));
        assert_eq!(nav_index(None, 3, NavDir::Up), Some(2));
        assert_eq!(nav_index(Some(0), 0, NavDir::Down), None);
    }
}
