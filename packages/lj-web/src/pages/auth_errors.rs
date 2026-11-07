//! Traduction FR des codes d'erreur d'auth. Module sans I/O.
//!
//! Un code, un message : les écrans de compte reçoivent le code en query
//! (`?error=…`), posé par les redirections 303 de `lj-api`.

/// Code d'erreur -> message FR.
fn code_map(code: &str) -> Option<&'static str> {
    Some(match code {
        "auth_required" => "E-mail ou mot de passe incorrect.",
        "mot_de_passe_trop_faible" => {
            "Mot de passe trop faible : au moins 8 caractères, dont une minuscule, une majuscule et un chiffre."
        }
        "mot_de_passe_trop_long" => "Mot de passe trop long (72 octets au plus).",
        "mots_de_passe_differents" => "Les mots de passe ne correspondent pas.",
        "email_invalide" => "Adresse e-mail invalide.",
        "email_jetable" => "Les adresses jetables ne sont pas autorisées.",
        "lien_invalide_ou_expire" => {
            "Ce lien a expiré ou a déjà servi. Demandez-en un nouveau."
        }
        "mail_non_configure" | "envoi_refuse" => {
            "L'envoi du courriel a échoué. Réessayez dans un instant."
        }
        "session_rejected" => "Votre session a expiré. Reconnectez-vous.",
        "dernier_acces" => {
            "Impossible de retirer le dernier moyen de connexion : ajoutez d'abord un mot de passe ou une autre connexion."
        }
        "meme_adresse" => "C'est déjà l'adresse de votre compte.",
        "email_deja_utilise" => {
            "Cette adresse est déjà utilisée par un autre compte. Connectez-vous à ce compte, puis liez cette connexion depuis votre profil."
        }
        "google_deja_lie" => "Ce compte Google est déjà lié à un autre compte.",
        "microsoft_deja_lie" => "Ce compte Microsoft est déjà lié à un autre compte.",
        "google_sans_email" | "microsoft_sans_email" => {
            "Ce compte n'expose pas d'adresse e-mail : impossible de créer un compte LibreJustice avec."
        }
        "id_token_invalide" | "etat_invalide" => {
            "La connexion n'a pas abouti. Réessayez."
        }
        "tour_expire" => "Cette connexion a expiré ou a déjà servi. Relancez-la.",
        "google_non_configure" | "microsoft_non_configure" => {
            "Cette connexion n'est pas disponible pour le moment."
        }
        "sans_email" => "Votre compte n'a pas d'adresse e-mail.",
        "nom_trop_long" => "Nom affiché trop long (80 caractères au plus).",
        "confirmation_suppression" => "Tapez « SUPPRIMER » pour confirmer la suppression.",
        "email_not_confirmed" => {
            "E-mail non confirmé. Cliquez sur « Renvoyer l'e-mail » pour recevoir un nouveau lien."
        }
        "access_denied" => "Accès refusé.",
        _ => return None,
    })
}

/// Traduit un code d'erreur d'auth en message FR. Un code inconnu ne s'affiche
/// pas tel quel : il viendrait d'une URL fabriquée à la main.
pub(crate) fn translate_auth_error(code: Option<&str>) -> String {
    code.and_then(code_map)
        .unwrap_or("Erreur inconnue.")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_connu_est_traduit() {
        assert_eq!(
            translate_auth_error(Some("auth_required")),
            "E-mail ou mot de passe incorrect."
        );
    }

    /// Un code fabriqué ne doit pas s'afficher tel quel : ce serait une
    /// injection de texte par l'URL.
    #[test]
    fn code_inconnu_ou_absent_reste_generique() {
        assert_eq!(translate_auth_error(Some("<script>")), "Erreur inconnue.");
        assert_eq!(translate_auth_error(None), "Erreur inconnue.");
    }
}
