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
    /// contraintes) → 422, parité avec la `RequestValidationError` de FastAPI
    /// (Pydantic/Query). `BadRequest`/400 reste réservé au JSON malformé.
    ///
    /// Porte l'objet d'erreur Pydantic déjà construit (`{type, loc, msg, input,
    /// ctx?}`) : le corps `{"detail": [<obj>]}` est byte-identique à
    /// `jsonable_encoder(exc.errors())` après canonicalisation A/B (`jq -S`,
    /// strip des clés nulles). Cf. [`validation`].
    #[error("unprocessable")]
    Unprocessable(Value),
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    /// Service indisponible (ex. client résumé non instancié) → 503, parité
    /// `HTTPException(503, "summary_unavailable")`.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// Erreur d'un service amont (ex. génération résumé Mistral) → 502, parité
    /// `HTTPException(502, "summary_error")`.
    #[error("bad gateway: {0}")]
    BadGateway(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl From<tokio_postgres::Error> for ApiError {
    fn from(e: tokio_postgres::Error) -> Self {
        // Conserve l'erreur PG structurée (SQLSTATE) au lieu de la stringifier :
        // la frontière HTTP ([`crate::routes::pg_error_boundary`]) la classe
        // (class 22 → 422 invalid_request_bytes, parse ParadeDB → 422
        // invalid_query_syntax), parité des `@app.exception_handler` de `main.py`.
        ApiError::Store(lj_store::error::StoreError::Postgres(e))
    }
}

/// Famille d'erreur Postgres reconnue à la frontière HTTP et reclassée en 422
/// (porté dans une extension de réponse, lue par [`crate::routes::pg_error_boundary`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgErrKind {
    /// Syntaxe `paradedb.parse` invalide (`InternalError_` + `_PARADEDB_PARSE_ERR_RE`).
    ParseSyntax,
    /// Donnée invalide au niveau bytes/cast (NUL `\x00`, date hors plage…), SQLSTATE
    /// class 22 — `psycopg.DataError` côté oracle.
    DataException,
}

impl ApiError {
    /// Classe une erreur PG sous-jacente pour la reclasser en 422 à la frontière
    /// HTTP. `None` = erreur serveur opaque (reste 500). Parité avec l'ordre des
    /// handlers `main.py` : la syntaxe `paradedb.parse` (regex sur le message)
    /// avant la class 22 (NUL/cast).
    pub fn pg_err_kind(&self) -> Option<PgErrKind> {
        let ApiError::Store(lj_store::error::StoreError::Postgres(e)) = self else {
            return None;
        };
        // On lit le `DbError` structuré (message brut + SQLSTATE) plutôt que le
        // `Display` de l'erreur, dont le formatage masque le message ParadeDB.
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
}

impl ApiError {
    /// Code HTTP de la variante — source unique partagée par [`Self::status`] et
    /// l'impl [`IntoResponse`], pour qu'ils ne divergent pas.
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::BadGateway(_) => StatusCode::BAD_GATEWAY,
            ApiError::Store(_) | ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Code HTTP numérique de la réponse produite pour cette erreur.
    pub fn status(&self) -> u16 {
        self.status_code().as_u16()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Corps JSON : `{"detail": …}`, parité FastAPI. Pour les 4xx « détail
        // texte » (`HTTPException(detail="…")`), `detail` est la chaîne ; pour la
        // validation 422 (`RequestValidationError`), `detail` est la liste
        // d'objets Pydantic ; pour 5xx, on conserve la chaîne `detail` mais on
        // log la chaîne d'erreur complète côté serveur.
        let status = self.status_code();
        let detail: Value = match &self {
            ApiError::BadRequest(msg) => json!(msg),
            ApiError::Unprocessable(obj) => json!([obj.clone()]),
            ApiError::NotFound => json!("not_found"),
            ApiError::Unauthorized => json!("auth_required"),
            ApiError::Unavailable(msg) => json!(msg),
            ApiError::BadGateway(msg) => json!(msg),
            ApiError::Store(_) | ApiError::Internal(_) => json!("internal_error"),
        };
        // Famille PG reclassable (parse syntax / data exception) : portée en
        // extension de réponse. [`crate::routes::pg_error_boundary`] la relit pour
        // émettre le 422 ciblé (avec le `path`/`query` de la requête, qu'on n'a pas
        // ici). Capturé AVANT le log 5xx — une donnée invalide n'est pas une erreur
        // serveur.
        let pg_kind = self.pg_err_kind();
        if pg_kind.is_none() && status.is_server_error() {
            let mut chain = self.to_string();
            let mut src = std::error::Error::source(&self);
            while let Some(e) = src {
                chain.push_str(&format!(" -> {e}"));
                src = e.source();
            }
            tracing::error!(error = %chain, "5xx");
        }
        let mut resp = (status, Json(json!({ "detail": detail }))).into_response();
        if let Some(kind) = pg_kind {
            resp.extensions_mut().insert(kind);
        }
        resp
    }
}

/// Constructeurs d'objets d'erreur Pydantic (`{type, loc, msg, input, ctx?}`)
/// pour la parité byte-à-byte avec `RequestValidationError` de FastAPI.
///
/// Chaque fonction reproduit la sortie de `jsonable_encoder(exc.errors())` pour
/// un cas de validation que le port Rust détecte à la main (Pydantic valide en
/// désérialisant, axum/serde non). `loc` porte le préfixe Pydantic
/// (`"query"`/`"body"`), les messages et `ctx` sont copiés mot pour mot des
/// versions installées (FastAPI 0.136 / Pydantic 2.13).
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

    /// `date_from_datetime_parsing` — chaîne date non parsable (type émis par
    /// Pydantic en mode smart pour un champ `date` recevant une string : il tente
    /// d'abord le parse datetime). `error` est le détail speedate (« invalid
    /// character in year », « month value is outside expected range of 1-12 »…),
    /// repris verbatim dans `msg` et `ctx.error`.
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

    /// `enum` — valeur hors des variantes d'un `StrEnum`. `expected` est la
    /// liste Pydantic : `'a', 'b' or 'c'` (virgules + `or` final, sans Oxford).
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

    /// Joint les variantes au format Pydantic : `'a'`, `'a' or 'b'`,
    /// `'a', 'b' or 'c'` (virgule entre tous sauf le dernier, lié par ` or `).
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
    use serde_json::{json, Value};

    // Chaque attendu est copié de la sortie réelle de FastAPI 0.136 / Pydantic
    // 2.13 (`jsonable_encoder(exc.errors())`) pour figer la parité byte-à-byte.

    #[test]
    fn int_parsing_matches_pydantic() {
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
    fn missing_matches_pydantic() {
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
    fn bound_errors_match_pydantic() {
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
    fn enum_error_matches_pydantic() {
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
        // Variante à deux items : `'a' or 'b'` (pas de virgule).
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
    fn date_errors_match_pydantic() {
        // Sorties figées depuis Pydantic 2.13 sur le champ `date` du schéma
        // `SearchRequest` (type smart-mode `date_from_datetime_parsing`).
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
    fn list_length_errors_match_pydantic() {
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
