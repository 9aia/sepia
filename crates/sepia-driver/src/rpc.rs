//! ndjson JSON-RPC framing — one message per line on stdin/stdout, shared
//! by driver binaries (server side) and the daemon's driver host (client
//! side). Same wire machinery shape as ACP, so no second transport stack.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC_VERSION: &str = "2.0";

// Standard JSON-RPC codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

// Sepia driver application codes (-320xx reserved).
pub const SESSION_NOT_FOUND: i64 = -32001;
pub const SESSION_BUSY: i64 = -32002;
pub const SESSION_LOCKED: i64 = -32003;
pub const CAPABILITY_UNSUPPORTED: i64 = -32004;
pub const STORE_ERROR: i64 = -32005;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn store(message: impl Into<String>) -> Self {
        Self::new(STORE_ERROR, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, message)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// An inbound line — a request (has `id` + `method`), notification
/// (`method`, no `id`), or response (`id`, no `method`).
#[derive(Debug)]
pub enum Inbound {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

/// Decode one ndjson line. `Err` carries a response-shaped error when an
/// id could be recovered (`None` → nothing to reply to).
pub fn decode_line(line: &str) -> Result<Inbound, (Option<u64>, RpcError)> {
    let value: Value = serde_json::from_str(line)
        .map_err(|e| (None, RpcError::new(PARSE_ERROR, e.to_string())))?;
    let obj = value.as_object().ok_or_else(|| {
        (
            None,
            RpcError::new(INVALID_REQUEST, "message is not an object"),
        )
    })?;
    let id = obj.get("id").and_then(Value::as_u64);
    let method = obj.get("method").and_then(Value::as_str);
    match (id, method) {
        (Some(id), Some(method)) => Ok(Inbound::Request(Request {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            method: method.to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        })),
        (None, Some(method)) => Ok(Inbound::Notification(Notification {
            jsonrpc: JSONRPC_VERSION.into(),
            method: method.to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        })),
        (Some(id), None) => {
            let result = obj.get("result").cloned();
            let error = obj
                .get("error")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| (Some(id), RpcError::new(PARSE_ERROR, e.to_string())))?;
            Ok(Inbound::Response(Response {
                jsonrpc: JSONRPC_VERSION.into(),
                id,
                result,
                error,
            }))
        }
        (None, None) => Err((
            None,
            RpcError::new(INVALID_REQUEST, "message has neither id nor method"),
        )),
    }
}

pub fn encode_line(message: &impl Serialize) -> Result<String, serde_json::Error> {
    serde_json::to_string(message)
}
