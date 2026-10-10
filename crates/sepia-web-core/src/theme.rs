//! Tri-state theme — the pure state machine behind
//! `sepia_web::theme::provide_theme`. Storage (`sepia-theme` in
//! localStorage), the dark/light class on `<html>`, and the media
//! query stay in the shell; this owns the transitions and the stored
//! value mapping.

/// The localStorage key.
pub const STORAGE_KEY: &str = "sepia-theme";

/// Light / dark / follow-the-OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    /// No stored preference — `prefers-color-scheme` decides.
    #[default]
    System,
    Dark,
    Light,
}

impl Theme {
    /// Toggle-button caption.
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    /// Toggle-button glyph.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Dark => "☾",
            Self::Light => "☀",
            Self::System => "◐",
        }
    }

    /// Cycle for the toggle button: dark → light → system → dark.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::System,
            Self::System => Self::Dark,
        }
    }

    /// Decode the stored value — absent, empty, or unknown all mean
    /// `System` (the shell writes `""` to clear the key).
    pub fn from_stored(s: &str) -> Self {
        match s {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    /// What to write back — `System` stores `""` (treated as absent).
    pub fn stored(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::System => "",
        }
    }

    /// Resolve to the applied dark flag — `System` follows the
    /// `(prefers-color-scheme: dark)` media query result.
    pub fn prefers_dark(self, system_dark: bool) -> bool {
        match self {
            Self::Dark => true,
            Self::Light => false,
            Self::System => system_dark,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_cycles_dark_light_system() {
        assert_eq!(Theme::Dark.next(), Theme::Light);
        assert_eq!(Theme::Light.next(), Theme::System);
        assert_eq!(Theme::System.next(), Theme::Dark);
        // Three clicks return to the start.
        assert_eq!(Theme::Dark.next().next().next(), Theme::Dark);
    }

    #[test]
    fn storage_round_trips() {
        for t in [Theme::Dark, Theme::Light, Theme::System] {
            assert_eq!(Theme::from_stored(t.stored()), t);
        }
    }

    #[test]
    fn absent_or_garbage_storage_means_system() {
        assert_eq!(Theme::from_stored(""), Theme::System);
        assert_eq!(Theme::from_stored("sepia"), Theme::System);
        // System clears the key rather than writing "system".
        assert_eq!(Theme::System.stored(), "");
    }

    #[test]
    fn prefers_dark_resolves_system_via_media() {
        assert!(Theme::Dark.prefers_dark(false));
        assert!(!Theme::Light.prefers_dark(true));
        assert!(Theme::System.prefers_dark(true));
        assert!(!Theme::System.prefers_dark(false));
    }
}
