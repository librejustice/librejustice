use super::Run;

/// The profile page.
pub struct ProfileUi {
    pub title: &'static str,
    pub heading: &'static str,
    /// The outcome of an action, from the code its redirect carries.
    pub done: fn(&str) -> &'static str,
    /// The mail an action sent, from the code its redirect carries.
    pub sent: fn(&str) -> &'static str,
    pub identity: &'static str,
    pub email_address: &'static str,
    pub display_name: &'static str,
    pub anonymous: &'static str,
    pub save: &'static str,
    pub change_email: &'static str,
    pub new_email_placeholder: &'static str,
    pub request_change: &'static str,
    pub connections: &'static str,
    pub password: &'static str,
    pub change: &'static str,
    pub create: &'static str,
    pub api_keys: &'static str,
    pub delete_heading: &'static str,
    pub delete_warning: &'static [Run],
    /// Around the word to type, which the server checks and the page writes.
    pub delete_type: &'static str,
    /// The word typed to delete the account, one of `lj_dtos::ACCOUNT_DELETE_WORDS`.
    pub delete_word: &'static str,
    pub delete_to_confirm: &'static str,
    pub delete_forever: &'static str,
    pub unlink: &'static str,
    pub link: &'static str,
    pub session_expired: &'static str,
    pub sign_in_again: &'static str,
    pub keys_intro: &'static str,
    pub keys_docs: &'static str,
    pub key_name_placeholder: &'static str,
    pub create_key: &'static str,
    pub copy_key_now: &'static str,
    pub key_failed: &'static str,
    pub revoke: &'static str,
    /// The name of a key left unnamed, from the day, the month and the year it was made.
    pub default_key_label: fn(u32, u32, u32) -> String,
}
