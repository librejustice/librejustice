//! The documents the anonymisation workbench reads and writes, and the files a view in another origin loads.

/// Folders the workbench and its readers fetch from.
pub const FETCHED: [&str; 3] = ["/pkg/", "/redactor/", "/ocr/"];
/// The MCP server whose tool opens the workbench as a view, beside its page.
pub const MCP: &str = "/redact/mcp";
/// The module a view imports to load and mount the running build.
pub const ENTRY: &str = "/redact-view.js";
pub const SCRIPTS: [&str; 5] = ["/redact.js", "/ocr.js", "/htr.js", "/htr-worker.js", ENTRY];
pub const FONTS: &str = "/fonts/";

/// Whether any origin may read the file at `path`.
pub fn shared(path: &str) -> bool {
    FETCHED.iter().any(|folder| path.starts_with(folder))
        || SCRIPTS.contains(&path)
        || path.starts_with(FONTS)
}

/// The media type of a document, by the extension of its name.
pub fn media_type(name: &str) -> &'static str {
    match name.to_ascii_lowercase().rsplit('.').next().unwrap_or("") {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "odt" => "application/vnd.oasis.opendocument.text",
        "doc" => "application/msword",
        "htm" | "html" => "text/html",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::shared;

    #[test]
    fn only_the_workbench_files_cross_origins() {
        for path in [
            "/pkg/lj-web.4f2a.wasm",
            "/redactor/lj_redact_bg.wasm",
            "/redactor/htr/models/det.onnx",
            "/ocr/fra.traineddata",
            "/redact.js",
            "/htr-worker.js",
            "/redact-view.js",
            "/fonts/source-serif-4.woff2",
        ] {
            assert!(shared(path), "{path}");
        }
        for path in [
            "/",
            "/mcp",
            "/redact/mcp",
            "/internal/me",
            "/redact",
            "/redact.json",
            "/pkgs/x",
            "/api/v1/search",
        ] {
            assert!(!shared(path), "{path}");
        }
    }
}
