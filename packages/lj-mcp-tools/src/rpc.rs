//! The JSON-RPC envelope the redaction servers answer in, on the web and on the user's machine.

use serde_json::{json, Value};

/// A JSON-RPC error: its code and its message.
pub type Fault = (i32, String);

pub const PARSE_ERROR: i32 = -32700;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const RESOURCE_NOT_FOUND: i32 = -32002;

pub fn response(id: &Value, outcome: Result<Value, Fault>) -> Value {
    match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }),
    }
}

pub fn tool_text(text: &str, error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": error})
}
