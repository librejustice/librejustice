use super::Run;

/// The tools hub and the page that takes a contributed decision.
pub struct ToolsUi {
    pub title: &'static str,
    pub description: &'static str,
    pub eyebrow: &'static str,
    pub heading_lead: &'static str,
    pub heading_em: &'static str,
    pub available: &'static str,
    pub beta: &'static str,
    pub mcp: ToolCopy,
    pub api: ToolCopy,
    pub redact: ToolCopy,
    pub contribute: ToolCopy,
    pub contribute_title: &'static str,
    pub contribute_intro: &'static [Run],
    pub contribute_sent: &'static str,
    pub contribute_drop: &'static str,
    pub contribute_attest: &'static str,
    pub send: &'static str,
    /// The message of a refusal code from the upload endpoint, a generic failure for any other.
    pub contribute_error: fn(&str) -> &'static str,
}

/// A tool's card on the hub.
pub struct ToolCopy {
    pub title: &'static str,
    pub body: &'static str,
}
