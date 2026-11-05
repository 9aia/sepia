//! Entity-doc ↔ session mappings — the one place `sepia-session`
//! semantics meet peregrin's entity wire shape.

use serde_json::Value;

/// The kind peregrin registers sepia sessions under — the op
/// namespace prefix (`sepia.<verb>`) and the `kind` field on every
/// session entity doc.
pub const ENTITY_KIND: &str = "sepia-session";

/// An entity doc's `indexDoc` projected as a
/// [`sepia_session::SessionSummary`]. Returns `None` for entities of
/// other kinds or docs whose indexDoc doesn't decode.
pub fn entity_to_summary(doc: &Value) -> Option<sepia_session::SessionSummary> {
    if doc.get("kind").and_then(Value::as_str) != Some(ENTITY_KIND) {
        return None;
    }
    doc.get("indexDoc")
        .cloned()
        .and_then(|d| serde_json::from_value(d).ok())
}

/// The `id` field of an entity doc — `None` when missing/non-string.
pub fn entity_id_of(doc: &Value) -> Option<&str> {
    doc.get("id").and_then(Value::as_str)
}

/// Wrap a session summary as the entity doc a node writes —
/// `{id, kind, indexDoc, updatedAt, locked, live}`. Substrate columns
/// lift out of the indexDoc so the projection can order/filter
/// without parsing session fields.
pub fn summary_to_entity(summary: &Value) -> Value {
    let get = |k: &str| summary.get(k).cloned().unwrap_or(Value::Null);
    serde_json::json!({
        "id": get("id"),
        "kind": ENTITY_KIND,
        "indexDoc": summary,
        "updatedAt": get("updatedAt"),
        "locked": get("locked"),
        "live": summary.get("live").cloned().unwrap_or_else(|| get("busy")),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn summary_doc() -> Value {
        json!({
            "id": "s1", "title": "T", "cwd": "/w", "agent": "devin",
            "updatedAt": "2024-01-01T00:00:00Z", "locked": false,
            "source": "devin", "busy": true, "canRewind": false,
        })
    }

    #[test]
    fn summary_decodes_through_the_entity_wrap() {
        let doc = summary_to_entity(&summary_doc());
        assert_eq!(entity_id_of(&doc), Some("s1"));
        assert_eq!(doc["kind"], ENTITY_KIND);
        assert_eq!(doc["live"], true);
        let s = entity_to_summary(&doc).unwrap();
        assert_eq!(s.title, "T");
        assert!(s.busy);
    }

    #[test]
    fn foreign_kinds_dont_decode() {
        let doc = json!({"id":"c1","kind":"campaign","indexDoc":{}});
        assert!(entity_to_summary(&doc).is_none());
        let non_session = json!({"id":"x","kind":"sepia-session","indexDoc":{"id":"x"}});
        // indexDoc too thin to be a summary → None, not a half-decode.
        assert!(entity_to_summary(&non_session).is_none());
    }
}
