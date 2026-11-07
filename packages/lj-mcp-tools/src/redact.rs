//! The redaction MCP servers, the user's documents anonymised on the user's
//! machine: their tools, the workbench as an MCP App view, and what a session
//! carries of its client.

use serde_json::{json, Value};

use crate::{negotiate_protocol_version, read_only_annotations};

pub const SERVER_NAME: &str = "librejustice-redact";

/// The view `anonymise` renders. Its document is the front's to write.
pub const VIEW_URI: &str = "ui://redact/workbench";
pub const VIEW_MIME: &str = "text/html;profile=mcp-app";

pub const ANONYMISE: &str = "anonymise";
pub const ATTACH: &str = "attach_anonymised";
pub const SCAN: &str = "scan";
pub const RESTORE: &str = "restore";

const UI_EXTENSION: &str = "io.modelcontextprotocol/ui";

/// ChatGPT keeps the file a view uploads, where a tool result can attach it.
const FILE_STORE_CLIENT: &str = "openai-mcp";

/// What a client can do with the workbench, read from its `initialize`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Client {
    pub views: bool,
    pub file_store: bool,
}

impl Client {
    pub fn of_initialize(params: &Value) -> Self {
        let views = params["capabilities"]["extensions"][UI_EXTENSION]["mimeTypes"]
            .as_array()
            .is_some_and(|types| types.iter().any(|t| t == VIEW_MIME));
        Self {
            views,
            file_store: views && params["clientInfo"]["name"] == FILE_STORE_CLIENT,
        }
    }

    /// The server keeps no session: the id it hands back at `initialize` is
    /// what carries these traits to the client's later requests.
    pub fn session_id(self, nonce: &str) -> Option<String> {
        let flags = match (self.views, self.file_store) {
            (false, _) => return None,
            (true, false) => "v",
            (true, true) => "vf",
        };
        Some(format!("{flags}.{nonce}"))
    }

    pub fn of_session(id: Option<&str>) -> Self {
        let flags = id.and_then(|id| id.split_once('.')).map(|(flags, _)| flags);
        Self {
            views: matches!(flags, Some("v" | "vf")),
            file_store: flags == Some("vf"),
        }
    }
}

pub fn initialize_result(params: &Value, version: &str) -> Value {
    json!({
        "protocolVersion": negotiate_protocol_version(params["protocolVersion"].as_str()),
        "capabilities": {
            "tools": {"listChanged": false},
            "resources": {"listChanged": false},
        },
        "serverInfo": {"name": SERVER_NAME, "version": version},
    })
}

/// The workbench to a client that renders views, the attachment to one with a
/// file store, the one place it takes the file from.
pub fn tool_definitions(client: Client) -> Vec<Value> {
    let mut tools = Vec::new();
    if !client.views {
        return tools;
    }
    tools.push(json!({
        "name": ANONYMISE,
        "title": "Anonymise a Document",
        "description": ANONYMISE_DESC,
        "inputSchema": {"type": "object", "additionalProperties": false, "properties": {}},
        "annotations": read_only_annotations(),
        "_meta": {"ui": {"resourceUri": VIEW_URI}},
    }));
    if client.file_store {
        tools.push(json!({
            "name": ATTACH,
            "title": "Attach the Anonymised Document",
            "description": ATTACH_DESC,
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["file_id", "name"],
                "properties": {
                    "file_id": {"type": "string", "minLength": 1},
                    "name": {"type": "string", "minLength": 1},
                },
            },
            "annotations": read_only_annotations(),
        }));
    }
    tools
}

/// The local server's tools, which work on the user's files by path.
pub fn local_tool_definitions() -> Vec<Value> {
    let paths =
        json!({"type": "array", "minItems": 1, "items": {"type": "string", "minLength": 1}});
    let writes = json!({
        "readOnlyHint": false,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    });
    vec![
        json!({
            "name": SCAN,
            "title": "Count What Would Be Anonymised",
            "description": SCAN_DESC,
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["paths"],
                "properties": {"paths": paths},
            },
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": ANONYMISE,
            "title": "Anonymise Documents",
            "description": LOCAL_ANONYMISE_DESC,
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["paths"],
                "properties": {"paths": paths, "session": {"type": "string", "minLength": 1}},
            },
            "annotations": writes,
        }),
        json!({
            "name": RESTORE,
            "title": "Restore the Real Values",
            "description": RESTORE_DESC,
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["session"],
                "properties": {
                    "session": {"type": "string", "minLength": 1},
                    "text": {"type": "string", "minLength": 1},
                    "path": {"type": "string", "minLength": 1},
                },
            },
            "annotations": writes,
        }),
    ]
}

pub fn view_resource(origin: &str) -> Value {
    json!({
        "uri": VIEW_URI,
        "name": "workbench",
        "title": "Anonymisation",
        "mimeType": VIEW_MIME,
        "_meta": view_meta(origin),
    })
}

/// `document` loads everything else from `origin`, the one place the host lets
/// the view reach.
pub fn view_contents(origin: &str, document: &str) -> Value {
    json!({
        "contents": [{
            "uri": VIEW_URI,
            "mimeType": VIEW_MIME,
            "text": document,
            "_meta": view_meta(origin),
        }],
    })
}

fn view_meta(origin: &str) -> Value {
    json!({
        "ui": {
            "csp": {"connectDomains": [origin], "resourceDomains": [origin]},
            "prefersBorder": true,
        },
    })
}

pub const OPENED: &str = "The workbench is open. The user picks the document in it.";

pub const ATTACH_ARGUMENTS: &str = "file_id and name are required.";

/// The answer's text, and a link to the file the view uploaded, which the host
/// materialises in the conversation. The model drops the scheme from the id.
pub fn attachment(file_id: &str, name: &str) -> (String, Value) {
    let id = file_id.strip_prefix("sediment://").unwrap_or(file_id);
    let link = json!({
        "type": "resource_link",
        "uri": format!("sediment://{id}"),
        "name": name,
        "mimeType": lj_vocab::files::media_type(name),
    });
    (format!("{name} is attached to the conversation."), link)
}

const ANONYMISE_DESC: &str = "Opens the anonymisation workbench, where the user \
anonymises a document in their own browser: the file never reaches you or \
any server. Use it when the user wants a document anonymised, instead of \
asking for the file.";

const SCAN_DESC: &str = "Counts, per family, what anonymise would replace in \
each file, without returning any of it. Takes absolute paths.";

const LOCAL_ANONYMISE_DESC: &str = "Anonymises files on the user's machine and \
returns their text, labels standing for names, addresses, contact details, \
identifiers and docket numbers: never a value. A label stands for one entity \
across a session's files: pass related files together, or an earlier `session` \
to add to it. Copies go to the session folder. Takes absolute paths.";

const RESTORE_DESC: &str = "Puts a session's values back in place of its \
labels, on the user's machine: pass `text`, or the `path` of a .docx, .odt or \
text file written with the labels. Returns where the result was written, never \
a value.";

const ATTACH_DESC: &str = "Adds to the conversation the anonymised file the \
workbench uploaded. Call it when the workbench asks, with the `file_id` and \
`name` it gives.";

#[cfg(test)]
mod tests {
    use super::*;

    fn initialize(name: &str, mime: &str) -> Value {
        json!({
            "capabilities": {"extensions": {UI_EXTENSION: {"mimeTypes": [mime]}}},
            "clientInfo": {"name": name, "version": "1.0.0"},
        })
    }

    /// Both hosts declare the extension and send the session id back on the
    /// `tools/list` that follows `initialize`.
    #[test]
    fn a_session_carries_what_the_client_renders() {
        let claude = Client::of_initialize(&initialize("Anthropic/ClaudeAI", VIEW_MIME));
        let chatgpt = Client::of_initialize(&initialize("openai-mcp", VIEW_MIME));
        let plain = Client::of_initialize(&json!({"clientInfo": {"name": "openai-mcp"}}));
        let other_mime = Client::of_initialize(&initialize("x", "text/html"));
        for client in [claude, chatgpt, plain, other_mime] {
            let id = client.session_id("4f2a");
            assert_eq!(Client::of_session(id.as_deref()), client, "{client:?}");
        }
        assert_eq!(
            (claude.views, claude.file_store, chatgpt.file_store),
            (true, false, true)
        );
        assert_eq!(plain, Client::default());
        assert_eq!(other_mime, Client::default());
        assert_eq!(Client::of_session(Some("a1b2c3")), Client::default());
    }

    #[test]
    fn each_client_is_offered_what_it_can_use() {
        let names = |client| -> Vec<String> {
            tool_definitions(client)
                .iter()
                .map(|tool| tool["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert!(names(Client::default()).is_empty());
        let views = Client {
            views: true,
            file_store: false,
        };
        assert_eq!(names(views), [ANONYMISE]);
        let store = Client {
            views: true,
            file_store: true,
        };
        assert_eq!(names(store), [ANONYMISE, ATTACH]);
    }

    #[test]
    fn the_attachment_links_the_stored_file_whatever_the_model_kept_of_its_id() {
        for id in ["file_00ab", "sediment://file_00ab"] {
            let (_, link) = attachment(id, "jugement-anonymise.pdf");
            assert_eq!(link["uri"], "sediment://file_00ab");
            assert_eq!(link["mimeType"], "application/pdf");
        }
    }
}
