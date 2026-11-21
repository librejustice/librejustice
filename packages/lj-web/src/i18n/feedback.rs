use super::Run;

/// The form to report a problem, and the page it lands on once sent.
pub struct FeedbackUi {
    pub title: &'static str,
    pub heading: &'static str,
    pub intro: &'static [Run],
    pub subject: &'static str,
    /// The subjects' labels, in the order of the codes the form posts.
    pub subjects: [&'static str; 4],
    pub page: &'static str,
    pub message: &'static str,
    pub email: &'static str,
    pub email_placeholder: &'static str,
    pub email_hint: &'static str,
    pub send: &'static str,
    pub sent_heading: &'static str,
    pub sent_body: &'static str,
    pub back_to_page: &'static str,
    /// The message of a refusal code from the report endpoint, a generic failure for any other.
    pub error: fn(&str) -> &'static str,
}
