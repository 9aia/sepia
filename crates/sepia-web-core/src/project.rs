//! Project membership — the pure side of `/projects`. Session
//! `projectIds` is the single source of truth; the UI toggles
//! membership by patching the session's meta overlay.

/// A session is a member when its `project_ids` contain the project
/// id and — when the project carries a `node` annotation (merged
/// multi-node lists) — the session lives on that node. A bare project
/// id could collide across nodes, so node-scoped rows compare both.
pub fn is_member(
    session_project_ids: &[String],
    session_node: Option<&str>,
    project_id: &str,
    project_node: Option<&str>,
) -> bool {
    session_project_ids.iter().any(|id| id == project_id)
        && (project_node.is_none() || session_node == project_node)
}

/// Add or drop `project_id` in a session's `project_ids`. Order is
/// preserved (added ids append) and the no-change cases return the
/// input verbatim — the caller can skip the patch when input equals
/// output.
pub fn with_membership(project_ids: &[String], project_id: &str, member: bool) -> Vec<String> {
    let has = project_ids.iter().any(|id| id == project_id);
    if member == has {
        return project_ids.to_vec();
    }
    if member {
        let mut ids = project_ids.to_vec();
        ids.push(project_id.to_string());
        ids
    } else {
        project_ids
            .iter()
            .filter(|id| id.as_str() != project_id)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn membership_matches_id_and_node() {
        let have = ids(&["p1", "p2"]);
        assert!(is_member(&have, None, "p1", None));
        assert!(!is_member(&have, None, "p9", None));
        // Node-scoped project: wrong node doesn't count.
        assert!(is_member(&have, Some("n1"), "p1", Some("n1")));
        assert!(!is_member(&have, Some("n2"), "p1", Some("n1")));
        assert!(!is_member(&have, None, "p1", Some("n1")));
        // Unscoped project matches whatever node the session is on.
        assert!(is_member(&have, Some("n2"), "p1", None));
    }

    #[test]
    fn toggling_membership() {
        let have = ids(&["p1", "p2"]);
        assert_eq!(with_membership(&have, "p3", true), ids(&["p1", "p2", "p3"]));
        assert_eq!(with_membership(&have, "p1", false), ids(&["p2"]));
        // Idempotent in both directions.
        assert_eq!(with_membership(&have, "p1", true), have);
        assert_eq!(with_membership(&have, "p9", false), have);
        assert_eq!(
            with_membership(&have, "p1", true),
            with_membership(&with_membership(&have, "p1", true), "p1", true)
        );
    }
}
