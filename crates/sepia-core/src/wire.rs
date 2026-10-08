//! The session wire payload — `session` object served by export and
//! accepted by import. Serde derives on the IR types already implement the
//! shape; these functions exist so the wire contract has one named surface.

use serde_json::Value;

use crate::domain::Session;

/// Encode a full `Session` into the wire payload — the faithful
/// counterpart of `session_from_json`.
///
/// # Errors
/// Returns the `serde_json` error if the value fails to serialize (the IR
/// only contains serializable fields, so this is effectively infallible).
pub fn session_to_json(session: &Session) -> Result<Value, serde_json::Error> {
    serde_json::to_value(session)
}

/// Decode a wire payload back into a `Session`, preserving tool-call
/// ids/args, thinking, usage, `tool_call_id` links and the
/// `parent_node_id` tree. Errors when the payload is not a session IR.
pub fn session_from_json(input: &Value) -> Result<Session, serde_json::Error> {
    serde_json::from_value(input.clone())
}
