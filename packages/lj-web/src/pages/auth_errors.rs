//! The message of an auth error code. No I/O.
//!
//! One code, one message: the account screens receive the code in the query (`?error=…`), set
//! by `lj-api`'s 303 redirects.

use crate::i18n::AccountUi;

/// The message of an auth error code. An unknown code is not shown as such: it would come from
/// a hand-made address.
pub(crate) fn translate_auth_error(ui: &AccountUi, code: Option<&str>) -> String {
    code.and_then(ui.auth_error)
        .unwrap_or(ui.unknown_error)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn code_connu_est_traduit() {
        assert_eq!(
            translate_auth_error(&Lang::Fr.ui().account, Some("auth_required")),
            "E-mail ou mot de passe incorrect."
        );
    }

    /// Un code fabriqué ne doit pas s'afficher tel quel : ce serait une
    /// injection de texte par l'URL.
    #[test]
    fn code_inconnu_ou_absent_reste_generique() {
        let ui = &Lang::Fr.ui().account;
        assert_eq!(
            translate_auth_error(ui, Some("<script>")),
            "Erreur inconnue."
        );
        assert_eq!(translate_auth_error(ui, None), "Erreur inconnue.");
    }
}
