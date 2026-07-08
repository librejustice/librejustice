//! Helpers libres partagés par les blocs `impl DecisionRepository` répartis dans
//! les sous-modules : routage des champs ré-extractibles, bindings dynamiques
//! tokio-postgres, parsing des refs JSON, mapping de rows. `pub(super)` car
//! couplés aux blocs impl, jamais exposés hors du module `repository`.

use super::types::ExtractedFields;
use chrono::{DateTime, Utc};
use tokio_postgres::types::ToSql;

/// Champs ré-extractibles (colonnes `decisions` mises à jour par re-extraction).
pub const REEXTRACTABLE_FIELDS: &[&str] = &[
    "jurisdiction_name",
    "date_lecture",
    "date_audience",
    "docket_numbers",
    "formation_or_chamber",
    "chamber_position",
    "chambre_uid",
    "formation_uid",
    "publication_codes",
    "solution_uid",
    "voie_uid",
    "office_uid",
    "legal_domain_uid",
    "publication_uid",
    "jurisdiction_code",
    "applicant_counsel_names",
    "applicant_law_firms",
    "applicant_companies",
    "defendant_counsel_names",
    "defendant_law_firms",
    "defendant_companies",
    "themes",
    "legal_references",
    "decision_links",
    "case_citations",
];

/// Champ ré-extractible routé vers `replace_citations` (pas une colonne JSONB sur
/// `decisions`, cf. ADR 0033 / 0112 P5).
pub(super) const LEGAL_REFS_FIELD: &str = "legal_references";

/// Champ ré-extractible routé vers `replace_decision_links` (table
/// `decision_links`, ADR 0161).
pub(super) const DECISION_LINKS_FIELD: &str = "decision_links";

/// Champ ré-extractible routé vers `replace_case_citations` (table
/// `case_citation`, ADR 0165).
pub(super) const CASE_CITATIONS_FIELD: &str = "case_citations";

/// Types PG des colonnes ré-extractibles. Sert à caster la première ligne de
/// `VALUES` dans `update_extracted_fields_bulk` (inférence de type robuste même
/// quand le batch n'a que des NULL pour une colonne).
pub(super) fn extracted_column_type(field: &str) -> &'static str {
    match field {
        "jurisdiction_name" => "text",
        "date_lecture" => "date",
        "date_audience" => "date",
        "docket_numbers" | "publication_codes" => "text[]",
        "formation_or_chamber" | "chamber_position" => "text",
        "solution_uid" | "voie_uid" | "office_uid" | "legal_domain_uid" | "publication_uid"
        | "jurisdiction_code" | "chambre_uid" | "formation_uid" => "text",
        "applicant_counsel_names"
        | "applicant_law_firms"
        | "applicant_companies"
        | "defendant_counsel_names"
        | "defendant_law_firms"
        | "defendant_companies"
        | "themes" => "text[]",
        other => panic!("colonne ré-extractible inconnue: {other}"),
    }
}

/// Valeur d'une colonne ré-extractible, boxée pour le binding tokio-postgres.
/// Réplique `_extracted_field_values` : routage champ → valeur SQL.
pub(super) fn extracted_field_value(
    extracted: &ExtractedFields,
    field: &str,
) -> Box<dyn ToSql + Sync> {
    match field {
        "jurisdiction_name" => Box::new(extracted.jurisdiction_name.clone()),
        "date_lecture" => Box::new(extracted.date_lecture),
        "date_audience" => Box::new(extracted.date_audience),
        "docket_numbers" => Box::new(extracted.docket_numbers.clone()),
        "formation_or_chamber" => Box::new(extracted.formation_or_chamber.clone()),
        "chamber_position" => Box::new(extracted.chamber_position.clone()),
        "chambre_uid" => Box::new(extracted.chambre_uid.clone()),
        "formation_uid" => Box::new(extracted.formation_uid.clone()),
        "publication_codes" => Box::new(extracted.publication_codes.clone()),
        "solution_uid" => Box::new(extracted.solution_uid.clone()),
        "voie_uid" => Box::new(extracted.voie_uid.clone()),
        "office_uid" => Box::new(extracted.office_uid.clone()),
        "legal_domain_uid" => Box::new(extracted.legal_domain_uid.clone()),
        "publication_uid" => Box::new(extracted.publication_uid.clone()),
        "jurisdiction_code" => Box::new(extracted.jurisdiction.as_ref().map(|j| j.code.clone())),
        "applicant_counsel_names" => Box::new(extracted.applicant_counsel_names.clone()),
        "applicant_law_firms" => Box::new(extracted.applicant_law_firms.clone()),
        "applicant_companies" => Box::new(extracted.applicant_companies.clone()),
        "defendant_counsel_names" => Box::new(extracted.defendant_counsel_names.clone()),
        "defendant_law_firms" => Box::new(extracted.defendant_law_firms.clone()),
        "defendant_companies" => Box::new(extracted.defendant_companies.clone()),
        "themes" => Box::new(extracted.themes.clone()),
        other => panic!("colonne ré-extractible inconnue: {other}"),
    }
}

/// Dérive le nom de source d'un `source_uid` à partir de son préfixe. Les fonds
/// DILA (ADR 0093) encodent `<fond>/<ID DILA>` ; les autres provenances héritent
/// du repli `opendata`.
pub fn source_from_source_uid(source_uid: &str) -> &'static str {
    if source_uid.starts_with("judilibre/") {
        "judilibre"
    } else if source_uid.starts_with("dila-jade/") {
        "dila-jade"
    } else if source_uid.starts_with("dila-constit/") {
        "dila-constit"
    } else if source_uid.starts_with("cedh/") {
        "cedh"
    } else if source_uid.starts_with("cjue/") {
        "cjue"
    } else if source_uid.starts_with("cnda/") {
        "cnda"
    } else {
        "opendata"
    }
}

/// Mappe une row `legal_article` (14 colonnes dans l'ordre du SELECT canonique :
/// text_uid, num, num_key, position, title_path, status, date_debut, date_fin,
/// texte, nota, content_checksum, source, source_uid, source_url) en
/// [`super::types::LegalArticleRow`]. Le `content_checksum` BIGINT est relu en
/// `u64` via le cast bit-à-bit inverse de l'écriture. `date_debut` est NOT NULL en
/// base (sentinelle '0001-01-01' pour la borne ouverte) → toujours `Some`.
pub(super) fn legal_article_row_from_row(r: tokio_postgres::Row) -> super::types::LegalArticleRow {
    let checksum: i64 = r.get(10);
    super::types::LegalArticleRow {
        text_uid: r.get(0),
        num: r.get(1),
        num_key: r.get(2),
        position: r.get(3),
        title_path: r.get(4),
        status: r.get(5),
        date_debut: r.get(6),
        date_fin: r.get(7),
        texte: r.get(8),
        nota: r.get(9),
        content_checksum: u64::from_ne_bytes(checksum.to_ne_bytes()),
        source: r.get(11),
        source_uid: r.get(12),
        source_url: r.get(13),
        texte_original: r.get(14),
        lang_original: r.get(15),
        translation: r.get(16),
        source_asof: r.get(17),
        source_upstream_url: r.get(18),
    }
}

/// Timestamp courant UTC (réplique `_now()` Python : tz-aware UTC).
pub(super) fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Adapte `Vec<Box<dyn ToSql + Sync>>` en slice de `&(dyn ToSql + Sync)` pour
/// les `execute`/`query` à params dynamiques.
pub(super) fn as_param_refs(params: &[Box<dyn ToSql + Sync>]) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|b| b.as_ref() as &(dyn ToSql + Sync))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracted_column_type_covers_all_column_fields() {
        for f in REEXTRACTABLE_FIELDS {
            if *f == LEGAL_REFS_FIELD || *f == DECISION_LINKS_FIELD || *f == CASE_CITATIONS_FIELD {
                continue;
            }
            // Ne panique pas → couverture complète.
            let _ = extracted_column_type(f);
        }
    }
}
