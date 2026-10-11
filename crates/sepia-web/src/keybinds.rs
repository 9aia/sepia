//! Per-browser keybind overrides — the `sepia-keymap` localStorage
//! blob (`{"action.id": "mod+k" | null}`) as a reactive signal, plus
//! the "recording" flag that parks the global listeners while
//! Settings → Keyboard captures a chord.
//!
//! The pure side lives in `sepia_web_core::keymap` (`Chord`,
//! `KeyOverrides`, `Keymap`); this module is only the storage effect
//! and the context both listeners and the settings UI read.
//!
//! SSR/hydrate: the signals start empty on both targets, so the first
//! client render matches the server markup; a hydrate-only Effect
//! seeds from localStorage after mount (same seed-then-persist shape
//! as the collapsed-groups store).

use leptos::prelude::*;
use sepia_web_core::keymap::{Action, KeyCtx, KeyOverrides, Keymap, Mods};

/// localStorage key for the overrides map.
pub const STORAGE_KEY: &str = "sepia-keymap";

/// The keybind context — provided by `App` on both targets.
#[derive(Clone, Copy)]
pub struct Keybinds {
    /// Binding overrides — empty on SSR and until the hydrate seed.
    pub overrides: RwSignal<KeyOverrides>,
    /// `Some(action)` while Settings → Keyboard is recording its next
    /// chord — every global keydown listener must sit out.
    pub recording: RwSignal<Option<Action>>,
}

impl Keybinds {
    /// The effective map (defaults + overrides) — reactive: call
    /// inside a tracking scope to rebuild on change.
    pub fn keymap(self) -> Keymap {
        Keymap::from_overrides(&self.overrides.get())
    }

    /// Resolve a keydown for the global listeners — untracked, and
    /// inert while a chord recording owns the keyboard.
    pub fn resolve(self, key: &str, mods: Mods, ctx: KeyCtx) -> Option<Action> {
        if self.recording.get_untracked().is_some() {
            return None;
        }
        Keymap::from_overrides(&self.overrides.get_untracked()).resolve(key, mods, ctx)
    }

    /// `true` while `action`'s binding is disabled (overridden to
    /// null) — tracked.
    pub fn disabled(self, action: Action) -> bool {
        self.overrides
            .get()
            .0
            .get(&action)
            .is_some_and(Option::is_none)
    }
}

/// Provide the context. On hydrate, seed `overrides` from storage
/// once and persist every later write; on SSR it's just the signals.
pub fn provide_keybinds() -> Keybinds {
    let kb = Keybinds {
        overrides: RwSignal::new(KeyOverrides::default()),
        recording: RwSignal::new(None),
    };
    provide_context(kb);

    #[cfg(feature = "hydrate")]
    {
        let (stored, set_stored, _clear) = leptos_use::storage::use_local_storage::<
            KeyOverrides,
            codee::string::JsonSerdeCodec,
        >(STORAGE_KEY);
        let mut init = true;
        Effect::new(move |_| {
            let cur = kb.overrides.get();
            if std::mem::replace(&mut init, false) {
                kb.overrides.set(stored.get_untracked());
            } else {
                set_stored.set(cur);
            }
        });
    }

    kb
}

/// The context — every consumer sits under `App`'s provider.
pub fn use_keybinds() -> Keybinds {
    expect_context::<Keybinds>()
}
