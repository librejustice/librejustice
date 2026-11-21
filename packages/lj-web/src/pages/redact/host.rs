//! The MCP Apps bridge: JSON-RPC 2.0 over `postMessage` to the host framing the view.
#![cfg(feature = "hydrate")]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use leptos::prelude::*;
use serde::Serialize;
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use super::fault::Fault;

const PROTOCOL: &str = "2026-01-26";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Inline,
    Fullscreen,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Inline => "inline",
            Mode::Fullscreen => "fullscreen",
        }
    }

    fn named(name: &str) -> Option<Self> {
        match name {
            "inline" => Some(Mode::Inline),
            "fullscreen" => Some(Mode::Fullscreen),
            _ => None,
        }
    }
}

/// What the host's own controls cover of the view, in pixels.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Insets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Saved {
    Opened,
    Downloaded,
}

#[derive(Clone, Copy)]
pub struct Host {
    pub mode: RwSignal<Mode>,
    pub expands: RwSignal<bool>,
    pub insets: RwSignal<Insets>,
    downloads: RwSignal<bool>,
}

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(1) };
    static PENDING: RefCell<HashMap<u64, (js_sys::Function, js_sys::Function)>> =
        RefCell::new(HashMap::new());
    static UPLOADED: RefCell<Option<Upload>> = const { RefCell::new(None) };
    static VERSIONS: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

struct Upload {
    bytes: Vec<u8>,
    name: String,
    id: String,
}

fn window() -> web_sys::Window {
    web_sys::window().expect("a view runs in a window")
}

fn parent() -> Option<web_sys::Window> {
    window().parent().ok().flatten()
}

/// A host's error that says nothing comes back as null.
fn reason(error: JsValue) -> Fault {
    if error.is_null() {
        return Fault::HostRefused;
    }
    js_sys::Reflect::get(&error, &JsValue::from_str("message"))
        .ok()
        .and_then(|message| message.as_string())
        .or_else(|| error.as_string())
        .map_or(Fault::HostSilent, Fault::Said)
}

fn to_js(value: &Value) -> Result<JsValue, serde_wasm_bindgen::Error> {
    value.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
}

fn post(message: &Value) {
    if let (Some(parent), Ok(data)) = (parent(), to_js(message)) {
        let _ = parent.post_message(&data, "*");
    }
}

fn notify(method: &str, params: Value) {
    post(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
}

async fn request(method: &str, params: Value) -> Result<Value, Fault> {
    let id = NEXT.with(|next| next.replace(next.get() + 1));
    let answer = js_sys::Promise::new(&mut |resolve, reject| {
        PENDING.with(|pending| pending.borrow_mut().insert(id, (resolve, reject)));
    });
    post(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    let result = JsFuture::from(answer).await.map_err(reason)?;
    serde_wasm_bindgen::from_value(result).map_err(Fault::said)
}

fn settle(id: u64, message: &Value) {
    let Some((resolve, reject)) = PENDING.with(|pending| pending.borrow_mut().remove(&id)) else {
        return;
    };
    let outcome = match message.get("error") {
        Some(error) => Err(error["message"]
            .as_str()
            .map_or(JsValue::NULL, |why| js_sys::Error::new(why).into())),
        None => {
            to_js(&message["result"]).map_err(|error| js_sys::Error::new(&error.to_string()).into())
        }
    };
    let _ = match outcome {
        Ok(result) => resolve.call1(&JsValue::NULL, &result),
        Err(why) => reject.call1(&JsValue::NULL, &why),
    };
}

fn reply(id: &Value, method: &str) {
    let message = match method {
        "ui/resource-teardown" | "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("method not found: {method}")},
        }),
    };
    post(&message);
}

impl Host {
    pub fn connect() -> Self {
        let host = Self {
            mode: RwSignal::new(Mode::Inline),
            expands: RwSignal::new(false),
            insets: RwSignal::new(Insets::default()),
            downloads: RwSignal::new(false),
        };
        host.listen();
        report_height();
        leptos::task::spawn_local(async move {
            let introduction = json!({
                "protocolVersion": PROTOCOL,
                "appInfo": {"name": "librejustice-redact", "version": env!("CARGO_PKG_VERSION")},
                "appCapabilities": {"availableDisplayModes": ["inline", "fullscreen"]},
            });
            match request("ui/initialize", introduction).await {
                Ok(answer) => {
                    let context = &answer["hostContext"];
                    host.downloads
                        .set(answer["hostCapabilities"]["downloadFile"].is_object());
                    host.expands.set(
                        context["availableDisplayModes"]
                            .as_array()
                            .is_some_and(|modes| modes.iter().any(|mode| mode == "fullscreen")),
                    );
                    host.absorb(context);
                    notify("ui/notifications/initialized", json!({}));
                }
                Err(why) => {
                    leptos::logging::warn!("the host did not answer ui/initialize: {why:?}")
                }
            }
        });
        host
    }

    fn listen(self) {
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |event: web_sys::MessageEvent| {
                let from_parent = event
                    .source()
                    .zip(parent())
                    .is_some_and(|(source, parent)| JsValue::from(source) == JsValue::from(parent));
                let Ok(message) = serde_wasm_bindgen::from_value::<Value>(event.data()) else {
                    return;
                };
                if !from_parent || message["jsonrpc"] != "2.0" {
                    return;
                }
                match (message["method"].as_str(), message.get("id")) {
                    (None, Some(id)) => {
                        if let Some(id) = id.as_u64().or_else(|| id.as_f64().map(|id| id as u64)) {
                            settle(id, &message);
                        }
                    }
                    (Some("ui/notifications/host-context-changed"), None) => {
                        self.absorb(&message["params"])
                    }
                    (Some(method), Some(id)) => reply(id, method),
                    _ => {}
                }
            },
        );
        let _ = window()
            .add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref());
        on_message.forget();
    }

    fn absorb(self, context: &Value) {
        if let Some(mode) = context["displayMode"].as_str().and_then(Mode::named) {
            self.mode.set(mode);
        }
        if let Some(insets) = context.get("safeAreaInsets") {
            let side = |name: &str| insets[name].as_f64().unwrap_or(0.0);
            self.insets.set(Insets {
                top: side("top"),
                right: side("right"),
                bottom: side("bottom"),
                left: side("left"),
            });
        }
        if let Some(theme) = context["theme"].as_str() {
            if let Some(root) = window().document().and_then(|d| d.document_element()) {
                let _ = root.class_list().toggle_with_force("dark", theme == "dark");
            }
        }
    }

    pub fn show(self, mode: Mode) {
        leptos::task::spawn_local(async move {
            match request("ui/request-display-mode", json!({"mode": mode.name()})).await {
                Ok(answer) => {
                    if let Some(mode) = answer["mode"].as_str().and_then(Mode::named) {
                        self.mode.set(mode);
                    }
                }
                Err(why) => leptos::logging::warn!("display mode refused: {why:?}"),
            }
        });
    }

    pub fn open_link(self, url: String) {
        leptos::task::spawn_local(async move {
            if let Err(why) = request("ui/open-link", json!({"url": url})).await {
                leptos::logging::warn!("link refused: {why:?}");
            }
        });
    }

    pub fn stores_files(self) -> bool {
        file_store().is_some()
    }

    /// `None` when the user declines the host's confirmation.
    pub async fn download(self, name: &str, bytes: &[u8]) -> Result<Option<Saved>, Fault> {
        if let Some(store) = file_store() {
            let (id, _) = uploaded(&store, name, bytes).await?;
            let asked = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&asked, &"fileId".into(), &JsValue::from_str(&id));
            let answer = call(&store, "getFileDownloadUrl", &asked).await?;
            let url = js_sys::Reflect::get(&answer, &"downloadUrl".into())
                .ok()
                .and_then(|url| url.as_string())
                .ok_or(Fault::HostGaveNoAddress)?;
            let answer = request("ui/open-link", json!({"url": url})).await?;
            return Ok((!declined(&answer)).then_some(Saved::Opened));
        }
        if self.downloads.get_untracked() {
            let encoded = String::from(js_sys::encode_uri_component(name));
            let resource = json!({
                "uri": format!("file:///{encoded}"),
                "mimeType": lj_vocab::files::media_type(name),
                "blob": base64(bytes)?,
            });
            let answer = request(
                "ui/download-file",
                json!({"contents": [{"type": "resource", "resource": resource}]}),
            )
            .await?;
            return Ok((!declined(&answer)).then_some(Saved::Downloaded));
        }
        super::engine::save(name, bytes);
        Ok(Some(Saved::Downloaded))
    }

    /// Only a tool result the model requests puts a file in the conversation.
    pub async fn attach(
        self,
        name: &str,
        bytes: &[u8],
        prompt: fn(&str, &str) -> String,
    ) -> Result<(), Fault> {
        let store = file_store().ok_or(Fault::HostKeepsNoFile)?;
        let (id, name) = uploaded(&store, name, bytes).await?;
        let text = prompt(&name, &id);
        let answer = request(
            "ui/message",
            json!({"role": "user", "content": [{"type": "text", "text": text}]}),
        )
        .await?;
        match declined(&answer) {
            true => Err(Fault::MessageNotSent),
            false => Ok(()),
        }
    }
}

/// How a host answers a request its user turned down.
fn declined(answer: &Value) -> bool {
    answer["isError"] == true
}

fn report_height() {
    let Some(root) = window().document().and_then(|d| d.document_element()) else {
        return;
    };
    let measured = root.clone();
    let on_resize = Closure::<dyn FnMut(js_sys::Array)>::new(move |_entries: js_sys::Array| {
        let height = measured.get_bounding_client_rect().height().ceil();
        notify("ui/notifications/size-changed", json!({"height": height}));
    });
    if let Ok(observer) = web_sys::ResizeObserver::new(on_resize.as_ref().unchecked_ref()) {
        observer.observe(&root);
    }
    on_resize.forget();
}

/// ChatGPT's file API.
fn file_store() -> Option<JsValue> {
    let store = js_sys::Reflect::get(&window(), &"openai".into())
        .ok()
        .filter(JsValue::is_object)?;
    js_sys::Reflect::get(&store, &"uploadFile".into())
        .ok()
        .filter(JsValue::is_function)
        .map(|_| store)
}

async fn call(store: &JsValue, method: &str, argument: &JsValue) -> Result<JsValue, Fault> {
    let function: js_sys::Function = js_sys::Reflect::get(store, &method.into())
        .map_err(reason)?
        .dyn_into()
        .map_err(|_| Fault::HostLacks(method.to_string()))?;
    let answer = function.call1(store, argument).map_err(reason)?;
    JsFuture::from(js_sys::Promise::resolve(&answer))
        .await
        .map_err(reason)
}

/// The stored file's id and the name it was stored under.
async fn uploaded(store: &JsValue, name: &str, bytes: &[u8]) -> Result<(String, String), Fault> {
    let known = UPLOADED.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|upload| upload.bytes == bytes)
            .map(|upload| (upload.id.clone(), upload.name.clone()))
    });
    if let Some(known) = known {
        return Ok(known);
    }
    let name = versioned(name);
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let bag = web_sys::FilePropertyBag::new();
    bag.set_type(lj_vocab::files::media_type(&name));
    let file = web_sys::File::new_with_u8_array_sequence_and_options(&parts, &name, &bag)
        .map_err(reason)?;
    let answer = call(store, "uploadFile", &file).await?;
    let id = js_sys::Reflect::get(&answer, &"fileId".into())
        .ok()
        .and_then(|id| id.as_string())
        .ok_or(Fault::HostKeptNoFile)?;
    UPLOADED.with(|slot| {
        *slot.borrow_mut() = Some(Upload {
            bytes: bytes.to_vec(),
            name: name.clone(),
            id: id.clone(),
        })
    });
    Ok((id, name))
}

/// ChatGPT keeps the bytes of the first file attached under a name, so each
/// later version of the output takes its own.
fn versioned(name: &str) -> String {
    let version = VERSIONS.with(|versions| {
        let mut versions = versions.borrow_mut();
        let version = versions.entry(name.to_string()).or_insert(0);
        *version += 1;
        *version
    });
    match (version, name.rsplit_once('.')) {
        (1, _) => name.to_string(),
        (version, Some((stem, extension))) => format!("{stem}-{version}.{extension}"),
        (version, None) => format!("{name}-{version}"),
    }
}

fn base64(bytes: &[u8]) -> Result<String, Fault> {
    let binary: String = bytes.iter().map(|&byte| char::from(byte)).collect();
    window().btoa(&binary).map_err(reason)
}
