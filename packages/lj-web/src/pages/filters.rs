//! Description des filtres d'une recherche passee (chips) — port de la logique
//! `describeFilters` / `FILTER_LABELS` / `FILTER_VALUE_LABELS` d'`activity-page`.
//!
//! Les valeurs des filtres referentiels (solution, voie, office, domaine,
//! codes de juridiction, publication — ADR 0146) sont affichees brutes
//! (suffixes d'uid / codes) : l'historique serialise le `SearchRequest` et le
//! front n'embarque aucune table de libelles referentiels. Seul le type de
//! juridiction (enum porte par `lj-dtos`) garde une map locale.

use serde_json::Value;

/// Champs de requete serialises dans `filters` qui ne sont PAS des filtres
/// affichables (port de `NON_FILTER_KEYS`).
const NON_FILTER_KEYS: [&str; 3] = ["mode", "sort", "ai_mode"];

/// Libelle court par cle de filtre (snake_case via la serialisation serde du
/// `SearchRequest`).
fn filter_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "juridiction_type" => "Juridiction",
        "solution" => "Solution",
        "voie" => "Voie",
        "office" => "Office",
        "legal_domain" => "Domaine",
        "jurisdiction_code" => "Juridiction",
        "legal_instrument" => "Texte",
        "legal_article" => "Article",
        "publication" => "Publication",
        "date_from" => "Depuis le",
        "date_to" => "Jusqu'au",
        _ => return None,
    })
}

/// Map code -> libelle pour une cle de filtre donnee (port de
/// `FILTER_VALUE_LABELS`). `None` si la cle n'a pas de map de valeurs.
fn value_label(key: &str, code: &str) -> Option<&'static str> {
    match key {
        "juridiction_type" => juridiction_type_label(code),
        _ => None,
    }
}

// Les CLES sont les codes serialises par serde cote API (sigles maj. pour la
// juridiction, UPPER_SNAKE pour les enums).

fn juridiction_type_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "TA" => "Tribunal administratif",
        "CAA" => "Cour administrative d'appel",
        "CE" => "Conseil d'État",
        "CC" => "Cour de cassation",
        "CA" => "Cour d'appel",
        "TJ" => "Tribunal judiciaire",
        "TCOM" => "Tribunal de commerce",
        _ => return None,
    })
}

/// Filtre decrit pour une chip (`{ key, label, value }`).
pub(super) struct DescribedFilter {
    pub label: String,
    pub value: String,
}

/// Formate la valeur d'un filtre : array -> labels joints par `, ` ; sinon label
/// unique (fallback `String(v)`). Port de `formatFilterValue`.
fn format_filter_value(key: &str, value: &Value) -> String {
    let one = |v: &Value| -> String {
        let code = json_scalar(v);
        value_label(key, &code).map(str::to_string).unwrap_or(code)
    };
    match value {
        Value::Array(items) => items.iter().map(one).collect::<Vec<_>>().join(", "),
        other => one(other),
    }
}

/// Representation scalaire d'une valeur JSON, parite `String(v)` JS.
fn json_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// Decrit les filtres affichables d'une entree d'historique. Port de
/// `describeFilters` : exclut les cles non-filtre, mappe libelles + valeurs.
pub(super) fn describe_filters(filters: &Value) -> Vec<DescribedFilter> {
    let Some(map) = filters.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(key, _)| !NON_FILTER_KEYS.contains(&key.as_str()))
        .map(|(key, value)| DescribedFilter {
            label: filter_label(key).unwrap_or(key).to_string(),
            value: format_filter_value(key, value),
        })
        .collect()
}

/// Encode un terme de requete pour le query string (parite `encodeURIComponent`).
pub(super) fn encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for b in query.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
