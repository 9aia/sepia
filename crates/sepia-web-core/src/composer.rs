//! Composer + session-ops shaping — the pure half of `session_detail`'s
//! prompt box and details sheet, and the list page's import card.
//! Every shape here mirrors `sepia_http::routes::sessions` exactly:
//! attachments are ACP `ContentBlock`s (the `PromptPart` union the
//! node's prompt validator accepts), convert/import targets are the
//! `{agent: "cline"|"devin"}` enum, and the export link rides the
//! hub's generic `/api/*` passthrough.

use serde_json::{Value, json};

use crate::filter::url_escape;

/// The node's prompt payload caps — mirrors `MAX_PROMPT_PARTS` /
/// `MAX_PROMPT_PART_CHARS` in `routes/sessions.rs` so the composer can
/// refuse early instead of round-tripping a 400.
pub const MAX_ATTACHMENTS: usize = 16;
/// Base64 chars, post-inflation — the node's 8MB budget.
pub const MAX_ATTACHMENT_CHARS: usize = 8 * 1024 * 1024;

/// One staged attachment — a chip above the textarea plus the base64
/// payload the browser's `FileReader` produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attachment {
    /// Display name (the file's name, `paste` for clipboard drops).
    pub name: String,
    /// MIME type the file/paste advertised; `""` when unknown.
    pub mime_type: String,
    /// Base64 payload — no `data:` prefix.
    pub data_b64: String,
    /// Pre-encoding byte count, for the chip's size label.
    pub size: f64,
}

/// `Attachment` → one ACP content block. `image/*` MIME maps to the
/// `image` part (`{type, data, mimeType}`); everything else rides a
/// `resource` blob (`{type: "resource", resource: {uri, mimeType,
/// blob}}`) — both are wire shapes the node's `is_prompt_part`
/// accepts. There is no audio path: the composer never records audio.
pub fn attachment_block(a: &Attachment) -> Value {
    if a.mime_type.starts_with("image/") {
        json!({
            "type": "image",
            "data": a.data_b64,
            "mimeType": a.mime_type,
        })
    } else {
        // The resource `uri` is a label, not a real path — the browser
        // can't see the filesystem location of a picked/pasted file.
        // `file:///<name>` keeps it non-empty and self-describing.
        let mime = if a.mime_type.is_empty() {
            "application/octet-stream"
        } else {
            a.mime_type.as_str()
        };
        json!({
            "type": "resource",
            "resource": {
                "uri": format!("file:///{}", url_escape(&a.name)),
                "mimeType": mime,
                "blob": a.data_b64,
            }
        })
    }
}

/// The `attachments` array for `POST /api/sessions/{id}/prompt` —
/// every staged item as a block, capped at the node's part limit.
/// The UI refuses past the cap already; this is belt-and-braces.
pub fn attachments_payload(items: &[Attachment]) -> Vec<Value> {
    items
        .iter()
        .take(MAX_ATTACHMENTS)
        .map(attachment_block)
        .collect()
}

/// Total base64 chars across staged items — measured against
/// [`MAX_ATTACHMENT_CHARS`], the node's payload budget.
pub fn attachment_chars(items: &[Attachment]) -> usize {
    items.iter().map(|a| a.data_b64.len()).sum()
}

/// Split a `FileReader` `data:<mime>;base64,<payload>` URL — `None`
/// for anything else (the composer only ever reads data: URLs).
pub fn split_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    Some((mime.to_string(), data.to_string()))
}

/// Byte-size label for the attachment chip — B / KB / MB.
pub fn byte_label(bytes: f64) -> String {
    if bytes < 1024.0 {
        format!("{bytes:.0} B")
    } else if bytes < 1024.0 * 1024.0 {
        format!("{:.1} KB", bytes / 1024.0)
    } else {
        format!("{:.1} MB", bytes / (1024.0 * 1024.0))
    }
}

/// Chip text — `name (size)`; a nameless item falls back to its MIME.
pub fn attachment_label(a: &Attachment) -> String {
    let name = if a.name.is_empty() {
        if a.mime_type.is_empty() {
            "attachment"
        } else {
            a.mime_type.as_str()
        }
    } else {
        a.name.as_str()
    };
    format!("{name} ({})", byte_label(a.size))
}

/// The `PATCH /api/sessions/{id}` body for the model override —
/// `{model: "x"}` to pin, `{model: null}` to restore the agent
/// default. Whitespace-only input is treated as the default too (the
/// node 400s an empty-but-present string).
pub fn model_patch(model: Option<&str>) -> Value {
    match model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => json!({ "model": m }),
        None => json!({ "model": null }),
    }
}

/// Convert/import stores the node accepts (`ImportTarget` in
/// `routes/sessions.rs` — `{agent: "cline"|"devin"}`). `(id, label)`.
pub const CONVERT_TARGETS: [(&str, &str); 2] = [("cline", "Cline"), ("devin", "Devin")];

/// Whether `target` is a store the node's convert/import routes take.
pub fn is_convert_target(target: &str) -> bool {
    CONVERT_TARGETS.iter().any(|(id, _)| *id == target)
}

/// Convert targets minus the store the session already lives in —
/// converting devin→devin is a no-op the UI shouldn't offer.
pub fn convert_targets(current_agent: &str) -> Vec<(&'static str, &'static str)> {
    CONVERT_TARGETS
        .into_iter()
        .filter(|(id, _)| *id != current_agent)
        .collect()
}

/// `GET` export URL — served by the hub's `/api/*` passthrough, so a
/// plain `<a download>` pulls the full session IR same-origin. `agent`
/// pins the store for ids that collide across agents.
pub fn export_href(id: &str, agent: Option<&str>) -> String {
    let mut href = format!("/api/sessions/{}/export", url_escape(id));
    if let Some(a) = agent.filter(|a| !a.is_empty()) {
        use std::fmt::Write;
        let _ = write!(href, "?agent={}", url_escape(a));
    }
    href
}

/// `POST /api/sessions/import` body — the UI only offers the
/// full-IR form (`{session}`), which is what `GET .../export` emits.
/// Accepts the export envelope `{session: {…}}` or a bare IR object;
/// anything else is `Err` before the round-trip. `cwd`, when given,
/// overrides the imported working directory.
pub fn import_body(file: &Value, target: &str, cwd: Option<&str>) -> Result<Value, String> {
    if !is_convert_target(target) {
        return Err(format!(
            "unknown import target {target:?} — expected 'cline' or 'devin'"
        ));
    }
    let session = file.get("session").unwrap_or(file).clone();
    if !session.is_object() {
        return Err(
            "not a session export — expected a JSON object (GET /api/sessions/:id/export)"
                .to_string(),
        );
    }
    let mut body = json!({ "agent": target, "session": session });
    if let Some(c) = cwd.map(str::trim).filter(|c| !c.is_empty()) {
        body["cwd"] = json!(c);
    }
    Ok(body)
}

/// `> `-prefixed markdown quote of `text` — the payload the
/// per-message reply/quote action inserts. Empty lines still carry
/// the marker so the quote stays a contiguous block. `""` quotes
/// nothing (an empty message has nothing to cite).
pub fn quote_block(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    text.lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Insert a quote at the end of the composer draft — a blank line
/// separates any existing text, and a trailing blank line leaves the
/// caret ready for the reply itself. Empty quotes and empty messages
/// leave the draft untouched.
pub fn draft_with_quote(draft: &str, quote: &str) -> String {
    let q = quote_block(quote);
    if q.is_empty() {
        return draft.to_string();
    }
    if draft.trim().is_empty() {
        return format!("{q}\n\n");
    }
    format!("{}\n\n{q}\n\n", draft.trim_end())
}

/// A non-empty agent query pair for links — `("?agent=x", "")` usage
/// keeps call sites ternary-free.
pub fn agent_query(agent: Option<&str>) -> String {
    agent
        .filter(|a| !a.is_empty())
        .map_or_else(String::new, |a| format!("?agent={}", url_escape(a)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn att(name: &str, mime: &str, data: &str, size: f64) -> Attachment {
        Attachment {
            name: name.into(),
            mime_type: mime.into(),
            data_b64: data.into(),
            size,
        }
    }

    #[test]
    fn image_attachment_maps_to_image_part() {
        let a = att("shot.png", "image/png", "aGVsbG8=", 42.0);
        let block = attachment_block(&a);
        assert_eq!(block["type"], "image");
        assert_eq!(block["data"], "aGVsbG8=");
        assert_eq!(block["mimeType"], "image/png");
        // The node's validator requires data + mimeType non-empty.
        assert!(block.get("uri").is_none());
    }

    #[test]
    fn non_image_attachment_maps_to_resource_blob() {
        let a = att("notes.md", "text/markdown", "aGVsbG8=", 5.0);
        let block = attachment_block(&a);
        assert_eq!(block["type"], "resource");
        assert_eq!(block["resource"]["uri"], "file:///notes.md");
        assert_eq!(block["resource"]["mimeType"], "text/markdown");
        assert_eq!(block["resource"]["blob"], "aGVsbG8=");
        // Empty MIME falls back to octet-stream, and names are escaped.
        let bare = att("my file.bin", "", "AA==", 2.0);
        let block = attachment_block(&bare);
        assert_eq!(block["resource"]["mimeType"], "application/octet-stream");
        assert_eq!(block["resource"]["uri"], "file:///my%20file.bin");
    }

    #[test]
    fn payload_caps_at_the_node_limit() {
        let items: Vec<Attachment> = (0..MAX_ATTACHMENTS + 3)
            .map(|i| att(&format!("f{i}"), "image/png", "AA==", 2.0))
            .collect();
        assert_eq!(attachments_payload(&items).len(), MAX_ATTACHMENTS);
    }

    #[test]
    fn split_data_url_parses_only_data_urls() {
        assert_eq!(
            split_data_url("data:image/png;base64,aGVsbG8="),
            Some(("image/png".to_string(), "aGVsbG8=".to_string()))
        );
        assert!(split_data_url("data:text/plain;base64,").is_some());
        assert!(split_data_url("blob:https://x/y").is_none());
        assert!(split_data_url("data:image/png;utf8,x").is_none());
        assert!(split_data_url("").is_none());
    }

    #[test]
    fn model_patch_encodes_set_and_unset() {
        assert_eq!(model_patch(Some("swe-1.6")), json!({ "model": "swe-1.6" }));
        assert_eq!(model_patch(Some("  ")), json!({ "model": null }));
        assert_eq!(model_patch(None), json!({ "model": null }));
    }

    #[test]
    fn convert_targets_exclude_the_current_store() {
        assert_eq!(convert_targets("devin"), vec![("cline", "Cline")]);
        assert_eq!(convert_targets("cline"), vec![("devin", "Devin")]);
        assert_eq!(convert_targets("claude").len(), 2);
        assert!(is_convert_target("cline"));
        assert!(!is_convert_target("claude"));
    }

    #[test]
    fn export_href_carries_the_agent() {
        assert_eq!(export_href("s1", None), "/api/sessions/s1/export");
        assert_eq!(export_href("s1", Some("")), "/api/sessions/s1/export");
        assert_eq!(
            export_href("s1", Some("devin")),
            "/api/sessions/s1/export?agent=devin"
        );
        assert_eq!(
            export_href("a/b", Some("x y")),
            "/api/sessions/a%2Fb/export?agent=x%20y"
        );
    }

    #[test]
    fn import_body_unwraps_the_export_envelope() {
        let file = json!({ "session": { "id": "s1", "nodes": [] } });
        let body = import_body(&file, "devin", None).unwrap();
        assert_eq!(body["agent"], "devin");
        assert_eq!(body["session"]["id"], "s1");
        // A bare IR object imports the same way.
        let body = import_body(&json!({ "id": "s2" }), "cline", Some("/work")).unwrap();
        assert_eq!(body["agent"], "cline");
        assert_eq!(body["session"]["id"], "s2");
        assert_eq!(body["cwd"], "/work");
        // Rejects non-session payloads and unknown targets.
        assert!(import_body(&json!([1, 2]), "devin", None).is_err());
        assert!(import_body(&json!({"id": "s"}), "claude", None).is_err());
    }

    #[test]
    fn quote_block_prefixes_every_line() {
        assert_eq!(quote_block("one\ntwo"), "> one\n> two");
        assert_eq!(quote_block("a\n\nb"), "> a\n>\n> b");
        assert_eq!(quote_block(""), "");
    }

    #[test]
    fn draft_with_quote_appends_and_separates() {
        // Empty draft → the quote stands alone, caret on a fresh line.
        assert_eq!(draft_with_quote("", "hi"), "> hi\n\n");
        assert_eq!(draft_with_quote("  ", "hi"), "> hi\n\n");
        // Existing draft keeps its text, blank line before the quote.
        assert_eq!(draft_with_quote("earlier\n", "hi"), "earlier\n\n> hi\n\n");
        // Nothing to quote → draft untouched.
        assert_eq!(draft_with_quote("keep me", ""), "keep me");
        assert_eq!(draft_with_quote("", ""), "");
    }

    #[test]
    fn labels_render_sizes() {
        assert_eq!(byte_label(512.0), "512 B");
        assert_eq!(byte_label(2048.0), "2.0 KB");
        assert_eq!(byte_label(3.0 * 1024.0 * 1024.0), "3.0 MB");
        let a = att("shot.png", "image/png", "", 2048.0);
        assert_eq!(attachment_label(&a), "shot.png (2.0 KB)");
        let nameless = att("", "image/png", "", 512.0);
        assert_eq!(attachment_label(&nameless), "image/png (512 B)");
    }
}
