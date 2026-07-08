//! Traduction FR des erreurs d'auth Supabase (port pur de `lib/auth-errors.ts`).
//! Module sans I/O ; la lecture/effacement du hash d'erreur passe par le shim JS
//! `browser` (web-sys indisponible sans feature dans le crate fige).

/// Codes d'erreur Supabase -> message FR (port verbatim de `CODE_MAP`).
fn code_map(code: &str) -> Option<&'static str> {
    Some(match code {
        "invalid_credentials" => "Email ou mot de passe incorrect.",
        "email_not_confirmed" => {
            "Email non confirmé. Cliquez sur « Renvoyer l'email » pour recevoir un nouveau lien."
        }
        "user_already_exists" => {
            "Un compte existe déjà avec cet email. Essayez de vous connecter."
        }
        "signup_disabled" => "Les inscriptions sont temporairement désactivées.",
        "over_email_send_rate_limit" => {
            "Trop de demandes. Patientez quelques secondes avant de réessayer."
        }
        "over_request_rate_limit" => "Trop de tentatives. Réessayez dans un instant.",
        "weak_password" => {
            "Mot de passe trop faible : au moins 8 caractères, dont une minuscule, une majuscule et un chiffre."
        }
        "validation_failed" => "Données invalides.",
        "otp_expired" => {
            "Le lien a expiré (valable 24h). Demandez un nouvel email de confirmation ci-dessous."
        }
        "access_denied" => "Accès refusé.",
        "user_not_found" => "Aucun compte ne correspond à cet email.",
        "same_password" => "Le nouveau mot de passe doit être différent de l'ancien.",
        _ => return None,
    })
}

/// Patterns de message -> code (port de `MESSAGE_PATTERNS`). Matching
/// insensible a la casse via `contains` sur la forme minuscule (evite d'ajouter
/// `regex` au bundle wasm — aucun de ces patterns n'a de structure variable).
fn match_message_pattern(msg_lower: &str) -> Option<&'static str> {
    if msg_lower.contains("password should contain") {
        Some("weak_password")
    } else if msg_lower.contains("email not confirmed") {
        Some("email_not_confirmed")
    } else if msg_lower.contains("invalid login credentials") {
        Some("invalid_credentials")
    } else if msg_lower.contains("already registered") || msg_lower.contains("already exists") {
        Some("user_already_exists")
    } else {
        None
    }
}

/// Extrait un message FR de rate-limit `after N seconds` (port de
/// `rateLimitMessage` / `SECONDS_RE`).
fn rate_limit_message(msg: &str) -> Option<String> {
    let lower = msg.to_lowercase();
    let idx = lower.find("after ")?;
    let rest = &msg[idx + "after ".len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    // Doit etre suivi de " second" (s'?) pour matcher `after (\d+) seconds?`.
    let after_digits = rest[digits.len()..].trim_start();
    if !after_digits.to_lowercase().starts_with("second") {
        return None;
    }
    Some(format!(
        "Pour des raisons de sécurité, veuillez attendre {digits} seconde(s) avant de réessayer."
    ))
}

/// Traduit une erreur d'auth (code + message bruts) en message FR. Port fidele
/// de `translateAuthError`.
pub(crate) fn translate_auth_error(code: Option<&str>, message: &str) -> String {
    if code.is_none() && message.is_empty() {
        return "Erreur inconnue.".to_string();
    }

    if let Some(code) = code {
        if let Some(mapped) = code_map(code) {
            if code == "over_email_send_rate_limit" {
                if let Some(rl) = rate_limit_message(message) {
                    return rl;
                }
            }
            return mapped.to_string();
        }
    }

    if let Some(rl) = rate_limit_message(message) {
        return rl;
    }

    if let Some(key) = match_message_pattern(&message.to_lowercase()) {
        return code_map(key).unwrap_or(message).to_string();
    }

    if message.is_empty() {
        "Erreur inconnue.".to_string()
    } else {
        message.to_string()
    }
}

/// Erreur portee par le hash de redirection Supabase (port de `AuthHashError`).
pub(crate) struct AuthHashError {
    pub code: String,
    pub message: String,
}

/// Parse `#error=…&error_code=…&error_description=…` d'un hash brut. Port de
/// `readAuthHashError` (la lecture du hash DOM est faite par l'appelant via le
/// shim JS — ici on travaille sur la chaine brute, testable purement).
pub(crate) fn parse_auth_hash_error(hash: &str) -> Option<AuthHashError> {
    if !hash.contains("error") {
        return None;
    }
    let body = hash.strip_prefix('#').unwrap_or(hash);
    let mut code: Option<String> = None;
    let mut description: Option<String> = None;
    for pair in body.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match k {
            "error_code" => code = Some(decode_component(v)),
            "error_description" => description = Some(decode_component(v)),
            _ => {}
        }
    }
    if code.is_none() && description.is_none() {
        return None;
    }
    let fr = code.as_deref().and_then(code_map);
    let message = fr
        .map(str::to_string)
        .or_else(|| description.map(|d| d.replace('+', " ")))
        .unwrap_or_else(|| "Erreur d'authentification.".to_string());
    Some(AuthHashError {
        code: code.unwrap_or_else(|| "access_denied".to_string()),
        message,
    })
}

/// Decodage URL-component minimal d'un fragment de hash : `+` -> espace puis
/// percent-decoding. Suffisant pour les `error_*` Supabase.
fn decode_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_map_takes_precedence() {
        assert_eq!(
            translate_auth_error(Some("invalid_credentials"), "Invalid login credentials"),
            "Email ou mot de passe incorrect."
        );
    }

    #[test]
    fn rate_limit_overrides_code_map_for_email_send() {
        let out = translate_auth_error(
            Some("over_email_send_rate_limit"),
            "For security purposes, you can only request this after 42 seconds",
        );
        assert!(out.contains("42 seconde(s)"));
    }

    #[test]
    fn message_pattern_fallback() {
        assert_eq!(
            translate_auth_error(None, "Email not confirmed"),
            "Email non confirmé. Cliquez sur « Renvoyer l'email » pour recevoir un nouveau lien."
        );
    }

    #[test]
    fn empty_is_unknown() {
        assert_eq!(translate_auth_error(None, ""), "Erreur inconnue.");
    }

    #[test]
    fn hash_error_uses_code_map() {
        let err = parse_auth_hash_error(
            "#error=access_denied&error_code=otp_expired&error_description=Email+link+is+invalid",
        )
        .expect("hash error parsed");
        assert_eq!(err.code, "otp_expired");
        assert!(err.message.starts_with("Le lien a expiré"));
    }

    #[test]
    fn hash_error_falls_back_to_description() {
        let err = parse_auth_hash_error("#error_description=Something+went+wrong")
            .expect("hash error parsed");
        assert_eq!(err.code, "access_denied");
        assert_eq!(err.message, "Something went wrong");
    }

    #[test]
    fn no_error_in_hash() {
        assert!(parse_auth_hash_error("#access_token=abc").is_none());
    }
}
