//! Erreurs API → réponses HTTP.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("store: {0}")]
    Store(#[from] lj_store::error::StoreError),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Échec de validation d'une requête (paramètres query/path/corps hors
    /// contraintes) → 422. `BadRequest`/400 reste réservé au JSON malformé.
    ///
    /// Porte l'objet d'erreur déjà construit (`{type, loc, msg, input, ctx?}`) ;
    /// le corps rendu est `{"detail": [<obj>]}`. Cf. [`validation`].
    /// An array carries several such objects, rendered as the `detail` list.
    #[error("unprocessable")]
    Unprocessable(Value),
    #[error("not found")]
    NotFound,
    /// Article présent au référentiel, aucune version ne couvrant la date → 404
    /// portant le motif. Distinct de [`Self::NotFound`], qui dit « rien sous ce
    /// numéro » : sans ce troisième état, un article abrogé se lit comme un
    /// article inexistant.
    #[error("{0}")]
    NoVersionAtDate(String),
    #[error("unauthorized")]
    Unauthorized,
    /// Cookie de session présenté et **refusé** (inconnu, expiré, révoqué) → 401 +
    /// effacement du cookie. Distinct d'`Unauthorized`, qui dit « aucune preuve
    /// fournie » : sans ce troisième état, un refus de session est muet.
    #[error("session refusée")]
    SessionRejected,
    /// Service indisponible (ex. client résumé non instancié) → 503.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// Erreur d'un service amont (ex. génération résumé Mistral) → 502.
    #[error("bad gateway: {0}")]
    BadGateway(String),
    /// Base momentanément saturée : checkout du pool en timeout, toutes les
    /// connexions retenues → 503 + `Retry-After`. Les requêtes annulées par
    /// `statement_timeout`/`lock_timeout` sont reclassées pareillement via
    /// [`Self::is_db_busy`].
    #[error("db busy: {0}")]
    DbBusy(String),
    /// Budget de coût de l'appelant épuisé ([`crate::ratelimit`]) → 429 +
    /// `Retry-After`.
    #[error("rate limited")]
    TooManyRequests,
    #[error("internal: {0}")]
    Internal(String),
}

/// `Retry-After` (secondes) des 503 « base occupée ».
const DB_BUSY_RETRY_AFTER_SECS: &str = "3600";

/// `Retry-After` (secondes) des 429. Le seau se recharge en continu : une minute
/// rend au demandeur un plein débit-minute de jetons.
const RATE_LIMIT_RETRY_AFTER_SECS: &str = "60";

impl From<deadpool_postgres::PoolError> for ApiError {
    fn from(e: deadpool_postgres::PoolError) -> Self {
        // Un `PoolError` ne naît que du checkout (`pool.get()`). Le timeout
        // d'attente = saturation transitoire → 503 ; le reste (backend, pool
        // fermé…) est une erreur serveur 500.
        let detail = lj_pg::error::pool_detail(&e);
        match e {
            deadpool_postgres::PoolError::Timeout(_) => {
                ApiError::DbBusy(format!("checkout connexion: {detail}"))
            }
            _ => ApiError::Internal(format!("checkout connexion: {detail}")),
        }
    }
}

impl From<tokio_postgres::Error> for ApiError {
    fn from(e: tokio_postgres::Error) -> Self {
        // Conserve l'erreur PG structurée (SQLSTATE) au lieu de la stringifier : la
        // frontière HTTP ([`crate::routes::pg_error_boundary`]) la classe (class 22 →
        // 422 invalid_request_bytes, parse ParadeDB → 422 invalid_query_syntax).
        ApiError::Store(lj_store::error::StoreError::Postgres(e))
    }
}

/// Famille d'erreur Postgres reconnue à la frontière HTTP et reclassée en 422
/// (porté dans une extension de réponse, lue par [`crate::routes::pg_error_boundary`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgErrKind {
    /// Syntaxe `paradedb.parse` invalide (message ParadeDB).
    ParseSyntax,
    /// Donnée invalide au niveau bytes/cast (NUL `\x00`, date hors plage…),
    /// SQLSTATE class 22.
    DataException,
}

impl ApiError {
    /// Classe une erreur PG sous-jacente pour la reclasser en 422 à la frontière
    /// HTTP. `None` = erreur serveur opaque (reste 500). Ordre : la syntaxe
    /// `paradedb.parse` (regex sur le message) avant la class 22 (NUL/cast).
    pub fn pg_err_kind(&self) -> Option<PgErrKind> {
        let ApiError::Store(lj_store::error::StoreError::Postgres(e)) = self else {
            return None;
        };
        // On lit le `DbError` structuré (message brut + SQLSTATE) : le `Display` de
        // l'erreur masque le message ParadeDB.
        let db = e.as_db_error()?;
        let msg = db.message();
        if msg.contains("could not parse query string")
            || msg.contains("Exist query without a field")
        {
            return Some(PgErrKind::ParseSyntax);
        }
        if db.code().code().starts_with("22") {
            return Some(PgErrKind::DataException);
        }
        None
    }

    /// Vrai si l'erreur signale une base momentanément indisponible : checkout
    /// du pool en timeout ([`ApiError::DbBusy`]) ou requête annulée par
    /// `statement_timeout` (SQLSTATE 57014) / lock non obtenu (55P03) — le cas
    /// d'une fenêtre de maintenance type `VACUUM FULL`. La réponse passe en
    /// 503 + `Retry-After` au lieu d'un 500.
    fn is_db_busy(&self) -> bool {
        match self {
            ApiError::DbBusy(_) => true,
            ApiError::Store(lj_store::error::StoreError::Postgres(e)) => e
                .as_db_error()
                .is_some_and(|db| matches!(db.code().code(), "57014" | "55P03")),
            _ => false,
        }
    }
}

impl ApiError {
    /// Code HTTP de la variante — source unique partagée par [`Self::status`] et
    /// l'impl [`IntoResponse`].
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::NotFound | ApiError::NoVersionAtDate(_) => StatusCode::NOT_FOUND,
            ApiError::Unauthorized | ApiError::SessionRejected => StatusCode::UNAUTHORIZED,
            ApiError::Unavailable(_) | ApiError::DbBusy(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            ApiError::BadGateway(_) => StatusCode::BAD_GATEWAY,
            ApiError::Store(_) | ApiError::Internal(_) => {
                if self.is_db_busy() {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }
    }

    /// Code HTTP numérique de la réponse produite pour cette erreur.
    pub fn status(&self) -> u16 {
        self.status_code().as_u16()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Corps JSON : `{"detail": …}`. Pour les 4xx « détail texte », `detail`
        // est la chaîne ; pour la validation 422, `detail` est la liste d'objets
        // d'erreur ; pour 5xx, la chaîne d'erreur complète est loguée côté serveur.
        let status = self.status_code();
        let busy = self.is_db_busy();
        let detail: Value = match &self {
            ApiError::BadRequest(msg) => json!(msg),
            ApiError::Unprocessable(Value::Array(objs)) => json!(objs),
            ApiError::Unprocessable(obj) => json!([obj.clone()]),
            ApiError::NotFound => json!("not_found"),
            ApiError::NoVersionAtDate(msg) => json!(msg),
            ApiError::Unauthorized => json!("auth_required"),
            ApiError::SessionRejected => json!("session_rejected"),
            ApiError::Unavailable(msg) => json!(msg),
            ApiError::BadGateway(msg) => json!(msg),
            ApiError::DbBusy(_) => json!("database_busy"),
            ApiError::TooManyRequests => json!("rate_limited"),
            ApiError::Store(_) | ApiError::Internal(_) => {
                json!(if busy {
                    "database_busy"
                } else {
                    "internal_error"
                })
            }
        };
        // Famille PG reclassable (parse syntax / data exception) : portée en
        // extension de réponse, relue par [`crate::routes::pg_error_boundary`] qui émet
        // le 422 ciblé avec le `path`/`query` de la requête. Capturé AVANT le log 5xx.
        let pg_kind = self.pg_err_kind();
        if pg_kind.is_none() && status.is_server_error() {
            let mut chain = self.to_string();
            let mut src = std::error::Error::source(&self);
            while let Some(e) = src {
                chain.push_str(&format!(" -> {e}"));
                src = e.source();
            }
            // Base occupée = fenêtre de maintenance attendue, pas un bug : warn.
            if busy {
                tracing::warn!(error = %chain, "503 database_busy");
            } else {
                tracing::error!(error = %chain, "5xx");
            }
        }
        let mut resp = (status, Json(json!({ "detail": detail }))).into_response();
        if busy {
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static(DB_BUSY_RETRY_AFTER_SECS),
            );
        }
        if matches!(self, ApiError::TooManyRequests) {
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static(RATE_LIMIT_RETRY_AFTER_SECS),
            );
        }
        // Pas de `warn!` ici : le refus est journalisé par l'extracteur, une seule
        // fois par requête et sur toutes les routes, pas seulement celles qui
        // exigent une identité.
        if matches!(self, ApiError::SessionRejected) {
            for effacement in crate::session::clear_cookies() {
                if let Ok(v) = axum::http::HeaderValue::from_str(&effacement) {
                    resp.headers_mut().append(axum::http::header::SET_COOKIE, v);
                }
            }
        }
        if let Some(kind) = pg_kind {
            resp.extensions_mut().insert(kind);
        }
        resp
    }
}

/// Constructeurs d'objets d'erreur de validation (`{type, loc, msg, input,
/// ctx?}`) — le corps 422 de l'API.
///
/// Chaque fonction couvre un cas de validation détecté à la main (axum/serde ne
/// valide pas en désérialisant). `loc` porte le préfixe `"query"`/`"body"`.
pub mod validation {
    use serde_json::{json, Value};

    /// `int_parsing` — valeur de query/corps non convertible en entier.
    pub fn int_parsing(loc: &[&str], input: &str) -> Value {
        json!({
            "type": "int_parsing",
            "loc": loc,
            "msg": "Input should be a valid integer, unable to parse string as an integer",
            "input": input,
        })
    }

    /// `missing` — paramètre/champ requis absent (`input` nul, élidé en A/B).
    pub fn missing(loc: &[&str]) -> Value {
        json!({
            "type": "missing",
            "loc": loc,
            "msg": "Field required",
            "input": Value::Null,
        })
    }

    /// `string_too_short` — chaîne sous `min_length`.
    pub fn string_too_short(loc: &[&str], input: &str, min_length: u64) -> Value {
        json!({
            "type": "string_too_short",
            "loc": loc,
            "msg": format!("String should have at least {min_length} character{}",
                if min_length == 1 { "" } else { "s" }),
            "input": input,
            "ctx": { "min_length": min_length },
        })
    }

    /// `string_too_long` — chaîne au-dessus de `max_length`.
    pub fn string_too_long(loc: &[&str], input: &str, max_length: u64) -> Value {
        json!({
            "type": "string_too_long",
            "loc": loc,
            "msg": format!("String should have at most {max_length} character{}",
                if max_length == 1 { "" } else { "s" }),
            "input": input,
            "ctx": { "max_length": max_length },
        })
    }

    /// `greater_than_equal` — entier sous la borne `ge`.
    pub fn greater_than_equal(loc: &[&str], input: &str, ge: i64) -> Value {
        json!({
            "type": "greater_than_equal",
            "loc": loc,
            "msg": format!("Input should be greater than or equal to {ge}"),
            "input": input,
            "ctx": { "ge": ge },
        })
    }

    /// `less_than_equal` — entier au-dessus de la borne `le`.
    pub fn less_than_equal(loc: &[&str], input: &str, le: i64) -> Value {
        json!({
            "type": "less_than_equal",
            "loc": loc,
            "msg": format!("Input should be less than or equal to {le}"),
            "input": input,
            "ctx": { "le": le },
        })
    }

    /// `date_from_datetime_parsing` — chaîne date non parsable. `error` est le
    /// détail (« invalid character in year », « month value is outside expected
    /// range of 1-12 »…), repris verbatim dans `msg` et `ctx.error`.
    pub fn date_parsing(loc: &[&str], input: &str, error: &str) -> Value {
        json!({
            "type": "date_from_datetime_parsing",
            "loc": loc,
            "msg": format!("Input should be a valid date or datetime, {error}"),
            "input": input,
            "ctx": { "error": error },
        })
    }

    /// `greater_than_equal` pour un champ date : la borne `ge` est sérialisée en
    /// ISO (`"1678-01-01"`) dans `msg` et `ctx`, contrairement à la variante entière.
    pub fn date_greater_than_equal(loc: &[&str], input: &str, ge: &str) -> Value {
        json!({
            "type": "greater_than_equal",
            "loc": loc,
            "msg": format!("Input should be greater than or equal to {ge}"),
            "input": input,
            "ctx": { "ge": ge },
        })
    }

    /// `less_than_equal` pour un champ date (borne `le` ISO).
    pub fn date_less_than_equal(loc: &[&str], input: &str, le: &str) -> Value {
        json!({
            "type": "less_than_equal",
            "loc": loc,
            "msg": format!("Input should be less than or equal to {le}"),
            "input": input,
            "ctx": { "le": le },
        })
    }

    /// `enum` — valeur hors des variantes. `expected` est la liste `'a', 'b' or
    /// 'c'` (virgules + `or` final, sans Oxford).
    pub fn enum_error(loc: &[&str], input: &str, variants: &[&str]) -> Value {
        let expected = expected_list(variants);
        json!({
            "type": "enum",
            "loc": loc,
            "msg": format!("Input should be {expected}"),
            "input": input,
            "ctx": { "expected": expected },
        })
    }

    /// `extra_forbidden`: a parameter the contract does not name, refused with
    /// the names it does.
    pub fn extra_forbidden(loc: &[&str], input: &str, known: &[&str]) -> Value {
        json!({
            "type": "extra_forbidden",
            "loc": loc,
            "msg": format!("Extra inputs are not permitted, expected one of: {}", known.join(", ")),
            "input": input,
        })
    }

    /// `value_error` — valeur rejetée par une validation métier (message
    /// libre, ex. code juridiction hors référentiel avec suggestions).
    pub fn value_error(loc: &[&str], input: &str, error: &str) -> Value {
        json!({
            "type": "value_error",
            "loc": loc,
            "msg": format!("Value error, {error}"),
            "input": input,
            "ctx": { "error": error },
        })
    }

    /// `too_short` — liste sous `min_length` (après validation des items).
    pub fn too_short(loc: &[&str], input: Value, field_type: &str, min_length: u64) -> Value {
        let actual = input.as_array().map(|a| a.len() as u64).unwrap_or(0);
        json!({
            "type": "too_short",
            "loc": loc,
            "msg": format!(
                "{field_type} should have at least {min_length} item{} after validation, not {actual}",
                if min_length == 1 { "" } else { "s" }),
            "input": input,
            "ctx": { "field_type": field_type, "min_length": min_length, "actual_length": actual },
        })
    }

    /// `too_long` — liste au-dessus de `max_length`.
    pub fn too_long(loc: &[&str], input: Value, field_type: &str, max_length: u64) -> Value {
        let actual = input.as_array().map(|a| a.len() as u64).unwrap_or(0);
        json!({
            "type": "too_long",
            "loc": loc,
            "msg": format!(
                "{field_type} should have at most {max_length} item{} after validation, not {actual}",
                if max_length == 1 { "" } else { "s" }),
            "input": input,
            "ctx": { "field_type": field_type, "max_length": max_length, "actual_length": actual },
        })
    }

    /// Joint les variantes : `'a'`, `'a' or 'b'`, `'a', 'b' or 'c'` (virgule
    /// entre tous sauf le dernier, lié par ` or `).
    fn expected_list(variants: &[&str]) -> String {
        let quoted: Vec<String> = variants.iter().map(|v| format!("'{v}'")).collect();
        match quoted.split_last() {
            None => String::new(),
            Some((last, [])) => last.clone(),
            Some((last, head)) => format!("{} or {last}", head.join(", ")),
        }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::validation::*;
    use super::ApiError;
    use axum::response::IntoResponse;
    use serde_json::{json, Value};

    #[test]
    fn pool_wait_timeout_maps_to_503_with_retry_after() {
        let err: ApiError =
            deadpool_postgres::PoolError::Timeout(deadpool_postgres::TimeoutType::Wait).into();
        assert_eq!(err.status(), 503);
        let resp = err.into_response();
        assert_eq!(resp.status(), 503);
        assert_eq!(
            resp.headers().get(axum::http::header::RETRY_AFTER).unwrap(),
            "3600"
        );
    }

    #[test]
    fn pool_closed_stays_500() {
        let err: ApiError = deadpool_postgres::PoolError::Closed.into();
        assert_eq!(err.status(), 500);
    }

    #[test]
    fn int_parsing_error_shape() {
        assert_eq!(
            int_parsing(&["query", "limit"], "abc"),
            json!({
                "type": "int_parsing",
                "loc": ["query", "limit"],
                "msg": "Input should be a valid integer, unable to parse string as an integer",
                "input": "abc"
            })
        );
    }

    #[test]
    fn missing_error_shape() {
        assert_eq!(
            missing(&["query", "code_challenge"]),
            json!({
                "type": "missing",
                "loc": ["query", "code_challenge"],
                "msg": "Field required",
                "input": Value::Null
            })
        );
    }

    #[test]
    fn string_too_short_singular_character() {
        assert_eq!(
            string_too_short(&["query", "q"], "", 1),
            json!({
                "type": "string_too_short",
                "loc": ["query", "q"],
                "msg": "String should have at least 1 character",
                "input": "",
                "ctx": { "min_length": 1 }
            })
        );
    }

    #[test]
    fn string_too_long_plural_characters() {
        assert_eq!(
            string_too_long(&["query", "q"], "x", 512),
            json!({
                "type": "string_too_long",
                "loc": ["query", "q"],
                "msg": "String should have at most 512 characters",
                "input": "x",
                "ctx": { "max_length": 512 }
            })
        );
    }

    #[test]
    fn bound_errors_shape() {
        assert_eq!(
            greater_than_equal(&["query", "limit"], "0", 1),
            json!({
                "type": "greater_than_equal",
                "loc": ["query", "limit"],
                "msg": "Input should be greater than or equal to 1",
                "input": "0",
                "ctx": { "ge": 1 }
            })
        );
        assert_eq!(
            less_than_equal(&["query", "limit"], "51", 50),
            json!({
                "type": "less_than_equal",
                "loc": ["query", "limit"],
                "msg": "Input should be less than or equal to 50",
                "input": "51",
                "ctx": { "le": 50 }
            })
        );
    }

    #[test]
    fn enum_error_shape() {
        assert_eq!(
            enum_error(
                &["query", "mode"],
                "fulltext",
                &["auto", "lexical", "semantic"]
            ),
            json!({
                "type": "enum",
                "loc": ["query", "mode"],
                "msg": "Input should be 'auto', 'lexical' or 'semantic'",
                "input": "fulltext",
                "ctx": { "expected": "'auto', 'lexical' or 'semantic'" }
            })
        );
        assert_eq!(
            enum_error(&["query", "m"], "z", &["lexical", "hybrid"]),
            json!({
                "type": "enum",
                "loc": ["query", "m"],
                "msg": "Input should be 'lexical' or 'hybrid'",
                "input": "z",
                "ctx": { "expected": "'lexical' or 'hybrid'" }
            })
        );
    }

    #[test]
    fn date_errors_shape() {
        assert_eq!(
            date_parsing(
                &["query", "dateFrom"],
                "not-a-date",
                "invalid character in year"
            ),
            json!({
                "type": "date_from_datetime_parsing",
                "loc": ["query", "dateFrom"],
                "msg": "Input should be a valid date or datetime, invalid character in year",
                "input": "not-a-date",
                "ctx": { "error": "invalid character in year" }
            })
        );
        assert_eq!(
            date_greater_than_equal(&["query", "dateFrom"], "1600-01-01", "1678-01-01"),
            json!({
                "type": "greater_than_equal",
                "loc": ["query", "dateFrom"],
                "msg": "Input should be greater than or equal to 1678-01-01",
                "input": "1600-01-01",
                "ctx": { "ge": "1678-01-01" }
            })
        );
        assert_eq!(
            date_less_than_equal(&["query", "dateTo"], "2300-01-01", "2262-01-01"),
            json!({
                "type": "less_than_equal",
                "loc": ["query", "dateTo"],
                "msg": "Input should be less than or equal to 2262-01-01",
                "input": "2300-01-01",
                "ctx": { "le": "2262-01-01" }
            })
        );
    }

    #[test]
    fn list_length_errors_shape() {
        assert_eq!(
            too_short(&["body", "redirect_uris"], json!([]), "List", 1),
            json!({
                "type": "too_short",
                "loc": ["body", "redirect_uris"],
                "msg": "List should have at least 1 item after validation, not 0",
                "input": [],
                "ctx": { "field_type": "List", "min_length": 1, "actual_length": 0 }
            })
        );
        let eleven: Vec<String> = (0..11).map(|i| format!("https://a/{i}")).collect();
        assert_eq!(
            too_long(&["body", "redirect_uris"], json!(eleven), "List", 10),
            json!({
                "type": "too_long",
                "loc": ["body", "redirect_uris"],
                "msg": "List should have at most 10 items after validation, not 11",
                "input": eleven,
                "ctx": { "field_type": "List", "max_length": 10, "actual_length": 11 }
            })
        );
    }
}
