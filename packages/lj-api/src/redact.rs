//! The redaction MCP server on the web, without account: its tool opens the
//! anonymisation workbench as a view in the host, where the document stays.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use lj_mcp_tools::redact::{self, Client};
use lj_mcp_tools::rpc::{self, Fault};
use lj_vocab::files::MCP;
use opentelemetry::KeyValue;
use serde_json::{json, Value};

pub const PUBLISHED: bool = false;

const SESSION: &str = "mcp-session-id";

#[derive(Clone)]
struct View {
    origin: Arc<str>,
    document: Arc<str>,
}

/// `origin` serves the files `document` loads. MCP clients POST the URL as
/// typed, with or without its trailing slash.
pub fn router(origin: &str, document: String) -> Router {
    let view = View {
        origin: origin.into(),
        document: document.into(),
    };
    Router::new()
        .route(MCP, post(handle))
        .route(&format!("{MCP}/"), post(handle))
        .with_state(view)
}

async fn handle(State(view): State<View>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let method = body["method"].as_str().unwrap_or_default();
    let params = &body["params"];
    let (client, issued) = match method {
        "initialize" => {
            let client = Client::of_initialize(params);
            (client, client.session_id(&crate::oauth::token_urlsafe(16)))
        }
        _ => (
            Client::of_session(headers.get(SESSION).and_then(|v| v.to_str().ok())),
            None,
        ),
    };
    let outcome = answer(&view, method, params, client);
    let Some(id) = body.get("id") else {
        return StatusCode::ACCEPTED.into_response();
    };
    let mut resp = Json(rpc::response(id, outcome)).into_response();
    if let Some(value) = issued.and_then(|id| HeaderValue::from_str(&id).ok()) {
        resp.headers_mut().insert(SESSION, value);
    }
    resp
}

fn answer(view: &View, method: &str, params: &Value, client: Client) -> Result<Value, Fault> {
    match method {
        "initialize" => Ok(redact::initialize_result(params, env!("CARGO_PKG_VERSION"))),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": redact::tool_definitions(client) })),
        "tools/call" => call(params),
        "resources/list" => Ok(json!({ "resources": [redact::view_resource(&view.origin)] })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "resources/read" => match params["uri"].as_str() {
            Some(redact::VIEW_URI) => Ok(redact::view_contents(&view.origin, &view.document)),
            uri => Err((
                rpc::RESOURCE_NOT_FOUND,
                format!("resource not found: {}", uri.unwrap_or_default()),
            )),
        },
        other => Err((rpc::METHOD_NOT_FOUND, format!("method not found: {other}"))),
    }
}

fn call(params: &Value) -> Result<Value, Fault> {
    let args = &params["arguments"];
    let (tool, result) = match params["name"].as_str() {
        Some(redact::ANONYMISE) => ("anonymise", rpc::tool_text(redact::OPENED, false)),
        Some(redact::ATTACH) => match (args["file_id"].as_str(), args["name"].as_str()) {
            (Some(id), Some(name)) if !id.is_empty() && !name.is_empty() => {
                let (said, link) = redact::attachment(id, name);
                let mut result = rpc::tool_text(&said, false);
                result["content"]
                    .as_array_mut()
                    .expect("tool_text writes a content list")
                    .push(link);
                ("attach_anonymised", result)
            }
            _ => (
                "attach_anonymised",
                rpc::tool_text(redact::ATTACH_ARGUMENTS, true),
            ),
        },
        other => {
            return Err((
                rpc::INVALID_PARAMS,
                format!("Unknown tool: {}", other.unwrap_or_default()),
            ))
        }
    };
    lj_telemetry::meter()
        .u64_counter("redact.mcp.tool.calls")
        .with_description("Redaction MCP tool calls, per tool.")
        .build()
        .add(1, &[KeyValue::new("tool", tool)]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            origin: "https://example.test".into(),
            document: "<!doctype html>".into(),
        }
    }

    #[test]
    fn the_view_is_read_by_its_uri_alone() {
        let read = |uri: &str| {
            answer(
                &view(),
                "resources/read",
                &json!({"uri": uri}),
                Client::default(),
            )
        };
        let contents = read(redact::VIEW_URI).unwrap();
        assert_eq!(contents["contents"][0]["text"], "<!doctype html>");
        assert_eq!(
            read("ui://librejustice/workbench").unwrap_err().0,
            rpc::RESOURCE_NOT_FOUND
        );
    }

    #[test]
    fn the_attachment_needs_both_arguments() {
        let attach =
            |args: Value| call(&json!({"name": redact::ATTACH, "arguments": args})).unwrap();
        let linked = attach(json!({"file_id": "file_00ab", "name": "document-anonymise.pdf"}));
        assert_eq!(linked["isError"], false);
        assert_eq!(linked["content"][1]["uri"], "sediment://file_00ab");
        assert_eq!(attach(json!({"file_id": "file_00ab"}))["isError"], true);
        assert_eq!(
            attach(json!({"file_id": "", "name": "a.pdf"}))["isError"],
            true
        );
        assert_eq!(
            call(&json!({"name": "search_decisions"})).unwrap_err().0,
            -32602
        );
    }
}
