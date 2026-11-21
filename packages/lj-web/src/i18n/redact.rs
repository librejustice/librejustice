use crate::pages::redact::Fault;

/// The anonymisation workbench, on the site and inside a chat.
pub struct RedactUi {
    pub page_title: &'static str,
    pub page_description: &'static str,
    pub page_heading: &'static str,
    pub local_processing: &'static str,
    pub reading: &'static str,
    pub not_opened: &'static str,
    pub choose_other: &'static str,
    pub stays_local: &'static str,
    pub stays_local_body: &'static str,
    pub drop_here: &'static str,
    pub formats: &'static str,
    pub choose: &'static str,
    pub loading_tool: &'static str,
    pub public_code: &'static str,
    pub prepare: &'static str,
    pub prepare_body: &'static str,
    /// From the share of the tool fetched, 0 to 1.
    pub downloading: fn(f64) -> String,
    pub ready_offline: &'static str,
    pub ready_offline_body: &'static str,
    /// From why the download stopped.
    pub prepare_stopped: fn(&str) -> String,
    pub retry: &'static str,
    pub export: &'static str,
    pub add_to_chat: &'static str,
    pub sent_to_chat: &'static str,
    pub download: &'static str,
    pub opens_in_tab: &'static str,
    pub downloaded: &'static str,
    pub workbench: &'static str,
    pub collapse: &'static str,
    pub expand: &'static str,
    pub previous_page: &'static str,
    pub next_page: &'static str,
    pub zoom: &'static str,
    pub zoom_out: &'static str,
    pub zoom_in: &'static str,
    /// From a percentage already rounded.
    pub percent: fn(f64) -> String,
    pub fit: &'static str,
    pub fit_window: &'static str,
    pub original: &'static str,
    pub text: &'static str,
    pub mask_area: &'static str,
    pub other_file: &'static str,
    pub options: &'static str,
    pub remove_rect: &'static str,
    pub remove_rect_key: &'static str,
    /// What a label says of a string replaced by nothing.
    pub erased: &'static str,
    pub include: &'static str,
    /// From the string the button steps through.
    pub next_occurrence: fn(&str) -> String,
    pub remove_from_list: &'static str,
    pub edit_label: &'static str,
    pub reset_label: &'static str,
    pub auto_label: &'static str,
    pub label: &'static str,
    /// One heading per rule family, in the order the workbench lists them.
    pub families: [&'static str; 9],
    pub ocr_loading: &'static str,
    pub ocr_language: &'static str,
    pub ocr_reading: &'static str,
    pub use_ocr: &'static str,
    /// From why the reading stopped.
    pub reading_stopped: fn(&str) -> String,
    pub ocr_fast: &'static str,
    pub ocr_fast_title: &'static str,
    pub ocr_slow: &'static str,
    pub ocr_slow_title: &'static str,
    pub one_moment: &'static str,
    /// From why the handover stopped.
    pub send_stopped: fn(&str) -> String,
    pub add_placeholder: &'static str,
    pub add_label: &'static str,
    pub add: &'static str,
    pub nothing_yet_draw: &'static str,
    pub nothing_yet: &'static str,
    pub rectangles: &'static str,
    pub clear_all: &'static str,
    /// From the rectangles drawn.
    pub drawn: fn(usize) -> String,
    /// From the strings ticked and the rectangles drawn.
    pub tally: fn(usize, usize) -> String,
    /// From where the text reader is.
    pub checking: fn(&str) -> String,
    pub removing: &'static str,
    pub anonymise: &'static str,
    pub stale_title: &'static str,
    pub stale_body: &'static str,
    /// From the outlet's label.
    pub anyway: fn(&str) -> String,
    pub apply_again: &'static str,
    pub not_downloadable: &'static str,
    /// From the strings still readable on the pages.
    pub still_readable: fn(&[String]) -> String,
    /// From why the second reading stopped.
    pub recheck_stopped: fn(&str) -> String,
    pub checking_pages: &'static str,
    /// The word a redacted file's name ends with, in plain letters.
    pub redacted: &'static str,
    /// What a file is named in a chat, which never sees the original's name.
    pub neutral_name: &'static str,
    /// From the file's name and the host's id for it: what the view posts as the user.
    pub attach_prompt: fn(&str, &str) -> String,
    pub mcp_loading: &'static str,
    pub mcp_failed: &'static str,
    /// What stopped the workbench, worded; a message from the module passes as it came.
    pub fault: fn(&Fault) -> String,
    /// The stage a text reader is in before it names one.
    pub reading_starts: &'static str,
}
