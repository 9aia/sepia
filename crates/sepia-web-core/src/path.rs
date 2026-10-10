//! Cwd-input helpers for the new-session form — typed-path splitting
//! and basename-prefix completion against `/api/fs` directory lists.

/// A trimmed string as `Option` — `None` for empty/whitespace.
pub fn non_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.trim().to_string())
}

/// `(parent dir, basename prefix)` for a typed absolute path —
/// `/home/us` → `("/home", "us")`, `/home/` → `("/home", "")`,
/// `/` → `("/", "")`. `None` for relative/empty input (the node's
/// `/api/fs` requires absolute paths anyway).
pub fn path_parts(typed: &str) -> Option<(String, String)> {
    if !typed.starts_with('/') {
        return None;
    }
    let (dir, base) = typed.rsplit_once('/')?;
    let parent = if dir.is_empty() { "/" } else { dir };
    Some((parent.to_string(), base.to_string()))
}

/// Filter `dirs` — the children of `typed`'s parent, as `/api/fs`
/// returns them (full paths) — to those whose basename starts with the
/// typed final segment, case-insensitively. Empty when `typed` isn't
/// absolute.
pub fn complete_path(typed: &str, dirs: &[String]) -> Vec<String> {
    let Some((_, prefix)) = path_parts(typed) else {
        return Vec::new();
    };
    let prefix = prefix.to_lowercase();
    dirs.iter()
        .filter(|d| {
            d.rsplit('/')
                .next()
                .unwrap_or_default()
                .to_lowercase()
                .starts_with(prefix.as_str())
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_trims() {
        assert_eq!(non_empty("  x "), Some("x".to_string()));
        assert_eq!(non_empty("   "), None);
        assert_eq!(non_empty(""), None);
    }

    #[test]
    fn path_parts_split_parent_and_prefix() {
        assert_eq!(path_parts("/"), Some(("/".into(), String::new())));
        assert_eq!(path_parts("/ho"), Some(("/".into(), "ho".into())));
        assert_eq!(
            path_parts("/home/u/a"),
            Some(("/home/u".into(), "a".into()))
        );
        assert_eq!(
            path_parts("/home/u/"),
            Some(("/home/u".into(), String::new()))
        );
        // Relative and empty input never reach `/api/fs`.
        assert_eq!(path_parts("rel/path"), None);
        assert_eq!(path_parts(""), None);
    }

    #[test]
    fn complete_path_matches_basename_prefix_case_insensitively() {
        let dirs = vec![
            "/home/u/app".to_string(),
            "/home/u/Archive".to_string(),
            "/home/u/zeta".to_string(),
        ];
        assert_eq!(
            complete_path("/home/u/a", &dirs),
            vec!["/home/u/app".to_string(), "/home/u/Archive".to_string()]
        );
        // Trailing slash → every child of the parent.
        assert_eq!(complete_path("/home/u/", &dirs), dirs);
        // Root-level prefix.
        assert_eq!(
            complete_path("/zet", vec!["/zeta".to_string()].as_slice()),
            vec!["/zeta".to_string()]
        );
        // Relative/empty typed values yield nothing.
        assert!(complete_path("rel/a", &dirs).is_empty());
        assert!(complete_path("", &dirs).is_empty());
    }
}
