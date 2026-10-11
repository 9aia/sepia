//! Global hotkey resolution — the pure key→action mapping behind the
//! document `keydown` listeners in `app` and `pages::session_list`.
//! Each listener translates its `KeyboardEvent` into `(key, Mods,
//! KeyCtx)`, fills in the context it owns, and dispatches only the
//! `Action`s in its scope (the shell owns `ToggleHelp`/`CloseHelp`/
//! `TogglePalette`/`OpenSettings`; the session list owns the rest).
//!
//! Bindings are remappable per browser: `KeyOverrides` (persisted to
//! localStorage by `sepia-web`) replaces an action's default `Chord`s
//! — or disables it — and `Keymap` folds defaults + overrides into the
//! effective resolution table. The Escape cascade (`CloseMenu` →
//! `ClearFilter` → `CloseHelp` → `CloseSelection`) stays fixed: it's a
//! context dispatch, not a binding.

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    /// `?` — open/close the shortcut cheat-sheet (shell).
    ToggleHelp,
    /// Escape while the cheat-sheet is open (shell).
    CloseHelp,
    /// ⌘K / Ctrl-K or `/` — open (⌘K also closes) the command palette.
    TogglePalette,
    /// ⌘B / Ctrl-B — collapse/expand the list column.
    ToggleList,
    /// `n` — create a session with the desktop aim (the old
    /// `session.new` binding).
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

    /// Every remappable action, in Settings display order — Sessions
    /// group first, then App (mirrors the old TS registry). The four
    /// Escape-cascade actions are absent: they're a fixed context
    /// dispatch, not a binding.
    pub const BINDABLE: [Self; 9] = [
        Self::FocusNewSession,
        Self::NavNext,
        Self::NavPrev,
        Self::FoldGroup,
        Self::UnfoldGroup,
        Self::TogglePalette,
        Self::ToggleList,
        Self::OpenSettings,
        Self::ToggleHelp,
    ];

    /// Whether this action can be rebound (all `BINDABLE` members).
    /// The Escape cascade can't — `Escape` is reserved.
    pub fn bindable(self) -> bool {
        Self::BINDABLE.contains(&self)
    }

    /// Stable storage id (`sepia-keymap` localStorage keys). `None`
    /// for the non-bindable Escape variants.
    pub fn id(self) -> Option<&'static str> {
        Some(match self {
            Self::FocusNewSession => "session.new",
            Self::NavNext => "nav.down",
            Self::NavPrev => "nav.up",
            Self::FoldGroup => "nav.collapse",
            Self::UnfoldGroup => "nav.expand",
            Self::TogglePalette => "app.palette",
            Self::ToggleList => "app.sidebar",
            Self::OpenSettings => "app.settings",
            Self::ToggleHelp => "app.keybinds",
            _ => return None,
        })
    }

    /// Inverse of [`Self::id`] — unknown/legacy ids drop silently.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::BINDABLE.iter().copied().find(|a| a.id() == Some(id))
    }

    /// Human name for the settings list and the `?` sheet.
    pub fn label(self) -> &'static str {
        match self {
            Self::ToggleHelp => "Keyboard shortcuts",
            Self::CloseHelp => "Close the shortcuts sheet",
            Self::TogglePalette => "Command palette",
            Self::ToggleList => "Toggle session list",
            Self::FocusNewSession => "New session",
            Self::NavNext => "Next session",
            Self::NavPrev => "Previous session",
            Self::CloseMenu => "Close the row menu",
            Self::ClearFilter => "Clear the filter",
            Self::CloseSelection => "Close the session",
            Self::OpenSettings => "Settings",
            Self::FoldGroup => "Collapse group",
            Self::UnfoldGroup => "Expand group",
        }
    }

    /// Settings group — `"Sessions"` or `"App"` (the Escape cascade
    /// reports `""` and is never listed).
    pub fn group(self) -> &'static str {
        match self {
            Self::FocusNewSession
            | Self::NavNext
            | Self::NavPrev
            | Self::FoldGroup
            | Self::UnfoldGroup => "Sessions",
            Self::TogglePalette | Self::ToggleList | Self::OpenSettings | Self::ToggleHelp => "App",
            _ => "",
        }
    }

    /// The factory binding(s) — replaced wholesale by an override.
    fn default_chords(self) -> Vec<Chord> {
        let table: &[(&str, bool, bool, bool)] = match self {
            Self::FocusNewSession => &[("n", false, false, false)],
            Self::NavNext => &[("ArrowDown", false, false, false)],
            Self::NavPrev => &[("ArrowUp", false, false, false)],
            Self::FoldGroup => &[("ArrowLeft", false, false, false)],
            Self::UnfoldGroup => &[("ArrowRight", false, false, false)],
            Self::TogglePalette => &[("k", true, false, false), ("/", false, false, false)],
            Self::ToggleList => &[("b", true, false, false)],
            Self::OpenSettings => &[(",", true, false, false)],
            // `?` twice — Shift allowed or not (it's `⇧/` on US
            // layouts but a plain key elsewhere).
            Self::ToggleHelp => &[("?", false, false, true), ("?", false, false, false)],
            _ => &[],
        };
        table
            .iter()
            .map(|&(key, command, alt, shift)| Chord {
                key: key.to_string(),
                command,
                alt,
                shift,
            })
            .collect()
    }
}

/// One recorded key binding — `key` is the `KeyboardEvent.key` value
/// (verbatim for named keys like `ArrowDown`; single ASCII letters are
/// stored lowercase), `command` is the platform-agnostic Ctrl-or-⌘.
///
/// `shift` is *significant* on plain single-character chords — `⇧N` is
/// a different chord from `n`, and `⇧/` produces `?` rather than `/`,
/// so the glyph and the flag travel together. Command/alt chords
/// ignore Shift entirely (legacy behavior: `⌘⇧K` ≡ `⌘K`), and named
/// keys (arrows, F-keys) tolerate it (`⇧↓` scrolls like `↓`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: String,
    pub command: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Chord {
    /// Capture a `keydown` into a chord. `None` for bare modifier
    /// presses (a chord needs a real key) — the recorder keeps
    /// listening. `Escape` *is* captured here; the caller treats it
    /// as cancel (it's the reserved cascade key).
    pub fn from_event(key: &str, mods: Mods) -> Option<Self> {
        if matches!(key, "Shift" | "Control" | "Alt" | "Meta") {
            return None;
        }
        let command = mods.command();
        let single_char = key.chars().count() == 1;
        let alpha = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        Some(Self {
            // Letters store lowercase; the shift flag carries the case.
            key: if single_char && alpha {
                key.to_ascii_lowercase()
            } else {
                key.to_string()
            },
            command,
            alt: mods.alt,
            // Shift survives only where it's significant: plain
            // single-char chords. On modifier chords and named keys
            // it's layout noise — dropped.
            shift: mods.shift && single_char && !command && !mods.alt,
        })
    }

    /// Serialize to `"mod+alt+shift+key"` — `mod` means Ctrl-or-⌘.
    /// The trailing token is the key verbatim, so a `+` key reads `+`.
    pub fn serialize(&self) -> String {
        let mut s = String::new();
        if self.command {
            s.push_str("mod+");
        }
        if self.alt {
            s.push_str("alt+");
        }
        if self.shift {
            s.push_str("shift+");
        }
        s.push_str(&self.key);
        s
    }

    /// Inverse of [`Self::serialize`], tolerant of case on the modifier
    /// tokens. `None` for empty keys, modifier-only leftovers, and
    /// `Escape` — the reserved cascade key can never be a binding.
    pub fn parse(s: &str) -> Option<Self> {
        let mut rest = s.trim();
        let (mut command, mut alt, mut shift) = (false, false, false);
        loop {
            let lower = rest.to_ascii_lowercase();
            if lower.starts_with("mod+") {
                command = true;
                rest = &rest["mod+".len()..];
            } else if lower.starts_with("alt+") {
                alt = true;
                rest = &rest["alt+".len()..];
            } else if lower.starts_with("shift+") {
                shift = true;
                rest = &rest["shift+".len()..];
            } else {
                break;
            }
        }
        if rest.is_empty() || rest.eq_ignore_ascii_case("escape") {
            return None;
        }
        let alpha = rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        Some(Self {
            key: if rest.chars().count() == 1 && alpha {
                rest.to_ascii_lowercase()
            } else {
                rest.to_string()
            },
            command,
            alt,
            shift,
        })
    }

    /// Display label — `⌘K`, `⇧N`, `↓`, `,`. `command` renders `⌘`
    /// (the glyph the rest of the UI already uses for the chord);
    /// `⇧` shows only where it's a real part of the chord (letters —
    /// punctuation glyphs already encode it).
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.command {
            s.push('⌘');
        }
        if self.alt {
            s.push('⌥');
        }
        let alpha = self
            .key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
        if self.shift && alpha {
            s.push('⇧');
        }
        let key = key_label(&self.key);
        // Modified letters print uppercase (`⌘K`, `⇧N`); the bare
        // `n` chord stays lowercase, matching the old cheat-sheet.
        if alpha && self.key.chars().count() == 1 && (self.command || self.alt || self.shift) {
            s.push_str(&key.to_ascii_uppercase());
        } else {
            s.push_str(&key);
        }
        s
    }

    /// Would this chord fire on `(key, mods)`? The tolerances mirror
    /// the old hand-written resolver:
    /// - `command`/`alt` must match exactly (one tightening: `⌘⌥K` no
    ///   longer counts as `⌘K`);
    /// - command/alt chords ignore Shift and letter case;
    /// - plain single-char chords require the recorded Shift state,
    ///   with case-insensitive letters (caps-lock tolerance) — this
    ///   is what keeps `⇧/` (`?`) out of the `/` binding;
    /// - named keys compare exactly and ignore Shift (`⇧↓` scrolls
    ///   like `↓`).
    pub fn matches(&self, key: &str, mods: Mods) -> bool {
        if self.command != mods.command() || self.alt != mods.alt {
            return false;
        }
        if self.command || self.alt {
            return self.key.eq_ignore_ascii_case(key);
        }
        if self.significant_shift() {
            return self.shift == mods.shift && self.key.eq_ignore_ascii_case(key);
        }
        self.key == key
    }

    /// Is Shift part of this chord's identity? Only for plain
    /// single-character keys — everywhere else `matches` ignores it.
    fn significant_shift(&self) -> bool {
        !self.command && !self.alt && self.key.chars().count() == 1
    }

    /// Conflict-detection identity — two chords canonize equal when
    /// they'd fire on the same events (`N`/`n` and recorded-but-
    /// insignificant Shift collapse away).
    fn canonical(&self) -> (String, bool, bool, bool) {
        (
            if self
                .key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                self.key.to_ascii_lowercase()
            } else {
                self.key.clone()
            },
            self.command,
            self.alt,
            self.significant_shift() && self.shift,
        )
    }
}

/// Display glyph for a `KeyboardEvent.key` value.
fn key_label(key: &str) -> String {
    match key {
        "ArrowDown" => "↓".to_string(),
        "ArrowUp" => "↑".to_string(),
        "ArrowLeft" => "←".to_string(),
        "ArrowRight" => "→".to_string(),
        " " => "Space".to_string(),
        "Escape" => "Esc".to_string(),
        other => other.to_string(),
    }
}

/// Per-browser binding overrides — the shape stored in localStorage
/// under `sepia-keymap` (`{"session.new": "j", "app.palette": null}`):
/// `Some(chord)` replaces the action's defaults, `None` disables it,
/// absent keeps them. Unknown action ids, malformed chords, and
/// `Escape` entries drop silently on parse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyOverrides(pub std::collections::BTreeMap<Action, Option<Chord>>);

impl KeyOverrides {
    /// Tolerant JSON parse — anything unreadable yields an empty map
    /// (defaults everywhere), never an error.
    pub fn parse(json: &str) -> Self {
        let Ok(raw) =
            serde_json::from_str::<std::collections::BTreeMap<String, serde_json::Value>>(json)
        else {
            return Self::default();
        };
        Self::from_raw(raw)
    }

    /// Shared with the serde impl: fold a raw `{id: chord|null}` map,
    /// dropping unknown ids and malformed values key by key.
    fn from_raw(raw: std::collections::BTreeMap<String, serde_json::Value>) -> Self {
        let mut out = std::collections::BTreeMap::new();
        for (id, value) in raw {
            let Some(action) = Action::from_id(&id) else {
                continue;
            };
            if value.is_null() {
                out.insert(action, None);
            } else if let Some(chord) = value.as_str().and_then(Chord::parse) {
                out.insert(action, Some(chord));
            }
        }
        Self(out)
    }

    /// `{"action.id": "chord" | null}` — the inverse of [`Self::parse`].
    pub fn serialize(&self) -> String {
        let map: serde_json::Map<String, serde_json::Value> = self
            .0
            .iter()
            .filter_map(|(a, c)| {
                a.id().map(|id| {
                    (
                        id.to_string(),
                        c.as_ref().map_or(serde_json::Value::Null, |ch| {
                            serde_json::Value::String(ch.serialize())
                        }),
                    )
                })
            })
            .collect();
        serde_json::Value::Object(map).to_string()
    }
}

impl serde::Serialize for KeyOverrides {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (a, c) in &self.0 {
            if let Some(id) = a.id() {
                map.serialize_entry(id, &c.as_ref().map(Chord::serialize))?;
            }
        }
        map.end()
    }
}

impl<'de> serde::Deserialize<'de> for KeyOverrides {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = <std::collections::BTreeMap<String, serde_json::Value> as serde::Deserialize>::
            deserialize(d)?;
        Ok(Self::from_raw(raw))
    }
}

/// The effective key → action table: every bindable action's chords —
/// its override when one is stored (or none when disabled), else its
/// defaults — folded into one ordered list.
#[derive(Clone, Debug)]
pub struct Keymap {
    /// `(action, chord)` pairs, declaration-ordered. Command chords
    /// resolve before the Escape cascade; the rest after it.
    bindings: Vec<(Action, Chord)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::from_overrides(&KeyOverrides::default())
    }
}

impl Keymap {
    pub fn from_overrides(overrides: &KeyOverrides) -> Self {
        let mut bindings = Vec::new();
        for action in Action::BINDABLE {
            match overrides.0.get(&action) {
                Some(Some(chord)) => bindings.push((action, chord.clone())),
                Some(None) => {}
                None => bindings.extend(action.default_chords().into_iter().map(|c| (action, c))),
            }
        }
        Self { bindings }
    }

    /// Every chord currently bound to `action` (empty = disabled).
    pub fn chords_for(&self, action: Action) -> Vec<&Chord> {
        self.bindings
            .iter()
            .filter_map(|(a, c)| (*a == action).then_some(c))
            .collect()
    }

    /// Display form of an action's bindings — `"⌘K / /"`, or `None`
    /// when the action is disabled. Duplicate labels collapse (the
    /// two `?` defaults — shifted and not — render once).
    pub fn binding_label(&self, action: Action) -> Option<String> {
        let chords = self.chords_for(action);
        if chords.is_empty() {
            return None;
        }
        let mut labels: Vec<String> = Vec::new();
        for c in chords {
            let l = c.label();
            if !labels.contains(&l) {
                labels.push(l);
            }
        }
        Some(labels.join(" / "))
    }

    /// Which *other* action already owns this chord — used to reject
    /// a recording that would shadow an existing binding.
    pub fn conflict(&self, action: Action, chord: &Chord) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(a, c)| *a != action && c.canonical() == chord.canonical())
            .map(|(a, _)| *a)
    }

    /// Resolve one keydown to an action, or `None` to leave it alone.
    ///
    /// Precedence mirrors the original handlers: command chords first
    /// (they fire even while typing — the browser-level shortcuts
    /// aren't text input), then the fixed Escape cascade (menu →
    /// filter → help → selection), then the non-command chords that
    /// editable targets swallow (typing gates every one of them,
    /// alt-chords included).
    pub fn resolve(&self, key: &str, mods: Mods, ctx: KeyCtx) -> Option<Action> {
        for (action, chord) in &self.bindings {
            if chord.command && chord.matches(key, mods) {
                return Some(*action);
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
        if ctx.typing {
            return None;
        }
        for (action, chord) in &self.bindings {
            if !chord.command && chord.matches(key, mods) {
                return Some(*action);
            }
        }
        None
    }
}

/// Resolve one keydown against the *default* map — kept for the
/// call sites and tests that pin the un-overridden behavior.
pub fn resolve_key(key: &str, mods: Mods, ctx: KeyCtx) -> Option<Action> {
    Keymap::default().resolve(key, mods, ctx)
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
    #![allow(clippy::fn_params_excessive_bools, clippy::unwrap_used)]
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

    // ── Chords / overrides / Keymap ───────────────────────────────

    fn chord(key: &str, command: bool, alt: bool, shift: bool) -> Chord {
        Chord {
            key: key.to_string(),
            command,
            alt,
            shift,
        }
    }

    #[test]
    fn chord_matches_mirror_the_legacy_tolerances() {
        // Command chords: shift ignored (⌘⇧K ≡ ⌘K), letter case free.
        let cmd_k = chord("k", true, false, false);
        assert!(cmd_k.matches("k", mods(true, false, false, false)));
        assert!(cmd_k.matches("K", mods(false, true, false, true)));
        // …but a held Alt makes it a different chord now.
        assert!(!cmd_k.matches("k", mods(true, false, true, false)));
        // Plain alnum: shift is significant, caps-lock isn't.
        let n = chord("n", false, false, false);
        assert!(n.matches("n", NO_MODS));
        assert!(n.matches("N", NO_MODS));
        assert!(!n.matches("N", mods(false, false, false, true)));
        // Punctuation keeps its captured shift — `?` is `⇧/`, so the
        // unshifted `/` binding stays out of the way and vice versa.
        assert!(chord("?", false, false, true).matches("?", mods(false, false, false, true)));
        assert!(!chord("?", false, false, true).matches("?", NO_MODS));
        assert!(chord("?", false, false, false).matches("?", NO_MODS));
        assert!(!chord("/", false, false, false).matches("/", mods(false, false, false, true)));
        // Named keys: exact key, shift tolerated.
        let down = chord("ArrowDown", false, false, false);
        assert!(down.matches("ArrowDown", mods(false, false, false, true)));
        assert!(!down.matches("ArrowUp", NO_MODS));
    }

    #[test]
    fn chord_from_event_normalizes() {
        // Bare modifiers aren't chords.
        for key in ["Shift", "Control", "Alt", "Meta"] {
            assert_eq!(Chord::from_event(key, mods(true, true, true, true)), None);
        }
        // Ctrl/⌘ collapse into `command`; letters store lowercase.
        assert_eq!(
            Chord::from_event("K", mods(false, true, false, true)),
            Some(chord("k", true, false, false))
        );
        // Shift survives on every plain single-char chord — `?`
        // arrived via Shift on a US layout, and the flag is what
        // keeps `⇧/` out of the `/` binding.
        assert_eq!(
            Chord::from_event("N", mods(false, false, false, true)),
            Some(chord("n", false, false, true))
        );
        assert_eq!(
            Chord::from_event("?", mods(false, false, false, true)),
            Some(chord("?", false, false, true))
        );
        assert_eq!(
            Chord::from_event("F5", mods(false, false, false, true)),
            Some(chord("F5", false, false, false))
        );
    }

    #[test]
    fn chord_serialize_parse_round_trip() {
        for c in [
            chord("k", true, false, false),
            chord("n", false, false, true),
            chord("F5", false, true, false),
            chord("?", false, false, false),
            chord("+", false, false, false),
            chord(",", true, false, false),
            chord("ArrowDown", false, false, false),
        ] {
            assert_eq!(Chord::parse(&c.serialize()), Some(c.clone()), "{c:?}");
        }
        assert_eq!(Chord::parse("mod+k"), Some(chord("k", true, false, false)));
        assert_eq!(
            Chord::parse("shift+N"),
            Some(chord("n", false, false, true))
        );
        // Escape is reserved — it can never be a stored binding.
        assert_eq!(Chord::parse("escape"), None);
        assert_eq!(Chord::parse("mod+Escape"), None);
        assert_eq!(Chord::parse(""), None);
        assert_eq!(Chord::parse("shift+"), None);
    }

    #[test]
    fn chord_labels_render_glyphs() {
        assert_eq!(chord("k", true, false, false).label(), "⌘K");
        assert_eq!(chord("n", false, false, true).label(), "⇧N");
        assert_eq!(chord("n", false, false, false).label(), "n");
        assert_eq!(chord("ArrowDown", false, false, false).label(), "↓");
        assert_eq!(chord("F5", false, true, false).label(), "⌥F5");
        assert_eq!(chord("?", false, false, false).label(), "?");
        assert_eq!(chord(",", true, false, false).label(), "⌘,");
    }

    #[test]
    fn overrides_parse_is_tolerant() {
        // Garbage → defaults everywhere.
        assert_eq!(KeyOverrides::parse("not json"), KeyOverrides::default());
        assert_eq!(KeyOverrides::parse("[1,2]"), KeyOverrides::default());
        // Unknown ids, malformed chords, Escape entries all drop.
        let o = KeyOverrides::parse(
            r#"{"bogus.id": "x", "session.new": "mod+Escape", "nav.down": 5,
               "app.palette": null, "session.new.2": "shift+mod+k"}"#,
        );
        let mut expect = std::collections::BTreeMap::new();
        expect.insert(Action::TogglePalette, None);
        assert_eq!(o, KeyOverrides(expect));
    }

    #[test]
    fn overrides_serde_round_trip() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            Action::FocusNewSession,
            Some(chord("j", false, false, false)),
        );
        map.insert(Action::ToggleHelp, None);
        let o = KeyOverrides(map);
        let json = serde_json::to_string(&o).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap(),
            serde_json::json!({"session.new": "j", "app.keybinds": null})
        );
        assert_eq!(serde_json::from_str::<KeyOverrides>(&json).unwrap(), o);
        // The str helpers agree with the serde path.
        assert_eq!(KeyOverrides::parse(&o.serialize()), o);
    }

    #[test]
    fn keymap_default_equals_resolve_key() {
        // Spot-check a spread of events — the Keymap path must agree
        // with the shimmed default resolver on every one.
        for (key, m, c) in [
            ("k", mods(true, false, false, false), NONE),
            ("/", NO_MODS, NONE),
            ("?", mods(false, false, false, true), NONE),
            ("n", NO_MODS, ctx(true, false, false, false)),
            ("Escape", NO_MODS, ctx(false, false, true, false)),
            ("ArrowLeft", mods(false, false, true, false), NONE),
            (",", mods(false, true, false, false), NONE),
        ] {
            assert_eq!(
                Keymap::default().resolve(key, m, c),
                resolve_key(key, m, c),
                "{key}"
            );
        }
    }

    #[test]
    fn keymap_override_replaces_defaults() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            Action::FocusNewSession,
            Some(chord("j", false, false, false)),
        );
        let km = Keymap::from_overrides(&KeyOverrides(map));
        // `j` now focuses; `n` is back to plain text.
        assert_eq!(
            km.resolve("j", NO_MODS, NONE),
            Some(Action::FocusNewSession)
        );
        assert_eq!(km.resolve("n", NO_MODS, NONE), None);
        // …and plain chords still stay out of text fields.
        assert_eq!(
            km.resolve("j", NO_MODS, ctx(true, false, false, false)),
            None
        );
    }

    #[test]
    fn keymap_override_command_chord_fires_while_typing() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            Action::FocusNewSession,
            Some(chord("j", true, false, false)),
        );
        let km = Keymap::from_overrides(&KeyOverrides(map));
        assert_eq!(
            km.resolve(
                "j",
                mods(true, false, false, false),
                ctx(true, false, false, false)
            ),
            Some(Action::FocusNewSession)
        );
    }

    #[test]
    fn keymap_disabled_action_has_no_binding() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(Action::TogglePalette, None);
        let km = Keymap::from_overrides(&KeyOverrides(map));
        assert_eq!(km.resolve("k", mods(true, false, false, false), NONE), None);
        assert_eq!(km.resolve("/", NO_MODS, NONE), None);
        assert_eq!(km.binding_label(Action::TogglePalette), None);
        assert!(km.chords_for(Action::TogglePalette).is_empty());
    }

    #[test]
    fn keymap_conflict_detects_canonical_duplicates() {
        let km = Keymap::default();
        // `n` is taken by New session — case and redundant shift
        // collapse to the same canonical chord.
        assert_eq!(
            km.conflict(Action::ToggleHelp, &chord("N", false, false, false)),
            Some(Action::FocusNewSession)
        );
        assert_eq!(
            km.conflict(Action::ToggleList, &chord("k", true, false, true)),
            Some(Action::TogglePalette)
        );
        // An action never conflicts with itself.
        assert_eq!(
            km.conflict(Action::FocusNewSession, &chord("n", false, false, false)),
            None
        );
        // `⇧N` is free — shift is significant on plain alnum chords.
        assert_eq!(
            km.conflict(Action::ToggleHelp, &chord("n", false, false, true)),
            None
        );
    }

    #[test]
    fn binding_labels_render_for_the_sheet() {
        let km = Keymap::default();
        assert_eq!(
            km.binding_label(Action::TogglePalette).as_deref(),
            Some("⌘K / /")
        );
        assert_eq!(
            km.binding_label(Action::FocusNewSession).as_deref(),
            Some("n")
        );
        assert_eq!(km.binding_label(Action::NavNext).as_deref(), Some("↓"));
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
