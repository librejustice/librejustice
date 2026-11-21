//! Helpers libres partagés par les blocs `impl DecisionRepository` répartis dans
//! les sous-modules : routage des champs ré-extractibles, bindings dynamiques
//! tokio-postgres, parsing des refs JSON, mapping de rows. `pub(super)` car
//! couplés aux blocs impl, jamais exposés hors du module `repository`.

use super::types::ExtractedFields;
use chrono::{DateTime, Utc};
use tokio_postgres::types::ToSql;

/// Champs ré-extractibles (colonnes `decisions` mises à jour par re-extraction).
pub const REEXTRACTABLE_FIELDS: &[&str] = &[
    "date_lecture",
    "date_audience",
    "docket_numbers",
    "seat",
    "seat_paths",
    "formation_uid",
    "formation_paths",
    "case_name",
    "search_title",
    "publication_codes",
    "solution_uid",
    "procedure_uid",
    "office_uid",
    "legal_domain_uid",
    "jurisdiction_code",
    "themes",
    "legal_references",
    "decision_links",
    "case_citations",
    "parties",
    "kind",
];

/// Champ ré-extractible routé vers `replace_citations` (pas une colonne JSONB sur
/// `decisions`).
pub(super) const LEGAL_REFS_FIELD: &str = "legal_references";

/// Champ ré-extractible routé vers `replace_decision_links` (table
/// `decision_links`).
pub(super) const DECISION_LINKS_FIELD: &str = "decision_links";

/// Champ ré-extractible routé vers `replace_case_citations` (table
/// `case_citation`).
pub(super) const CASE_CITATIONS_FIELD: &str = "case_citations";

/// Re-extractable field routed to `replace_decision_actors` (table
/// `decision_actors`).
pub(super) const PARTIES_FIELD: &str = "parties";

/// Champ ré-extractible routé vers `set_source_case_names` : trois colonnes
/// (`case_name`, `case_name_keys`, `case_name_source`) et un rang d'écrivain à
/// respecter, que l'UPDATE générique ne sait pas porter.
pub(super) const CASE_NAME_FIELD: &str = "case_name";

/// Champ écrit par une passe dédiée, hors de l'UPDATE colonnes générique.
pub(super) fn is_routed_field(field: &str) -> bool {
    matches!(
        field,
        LEGAL_REFS_FIELD
            | DECISION_LINKS_FIELD
            | CASE_CITATIONS_FIELD
            | PARTIES_FIELD
            | CASE_NAME_FIELD
    )
}

/// Types PG des colonnes ré-extractibles. Sert à caster la première ligne de
/// `VALUES` dans `update_extracted_fields_bulk` (inférence de type robuste même
/// quand le batch n'a que des NULL pour une colonne).
pub(super) fn extracted_column_type(field: &str) -> &'static str {
    match field {
        "date_lecture" => "date",
        "date_audience" => "date",
        "docket_numbers" | "publication_codes" | "seat_paths" | "formation_paths" => "text[]",
        "search_title" | "kind" => "text",
        "seat" => "jsonb",
        "solution_uid" | "procedure_uid" | "office_uid" | "legal_domain_uid"
        | "jurisdiction_code" | "formation_uid" => "text",
        "themes" => "text[]",
        other => panic!("colonne ré-extractible inconnue: {other}"),
    }
}

/// Valeur d'une colonne ré-extractible, boxée pour le binding tokio-postgres.
/// Routage champ → valeur SQL.
pub(super) fn extracted_field_value(
    extracted: &ExtractedFields,
    field: &str,
) -> Box<dyn ToSql + Sync> {
    match field {
        "date_lecture" => Box::new(extracted.date_lecture),
        "date_audience" => Box::new(extracted.date_audience),
        "docket_numbers" => Box::new(extracted.docket_numbers.clone()),
        "seat" => Box::new(extracted.seat.clone().map(tokio_postgres::types::Json)),
        "seat_paths" => Box::new(extracted.seat_paths.clone()),
        "formation_uid" => Box::new(extracted.formation_uid.clone()),
        "formation_paths" => Box::new(formation_paths(extracted)),
        "search_title" => Box::new(extracted.search_title.clone()),
        "publication_codes" => Box::new(extracted.publication_codes.clone()),
        "solution_uid" => Box::new(extracted.solution_uid.clone()),
        "procedure_uid" => Box::new(extracted.procedure_uid.clone()),
        "office_uid" => Box::new(extracted.office_uid.clone()),
        "legal_domain_uid" => Box::new(extracted.legal_domain_uid.clone()),
        // Le code canonique exige la résolution `source_code` → `code` via
        // `ensure_jurisdictions` : les appelants spécialisent ce champ avant d'arriver
        // ici.
        "jurisdiction_code" => unreachable!("jurisdiction_code se résout via ensure_jurisdictions"),
        "themes" => Box::new(extracted.themes.clone()),
        "kind" => Box::new(extracted.kind.as_str()),
        other => panic!("colonne ré-extractible inconnue: {other}"),
    }
}

/// The « Formation » filter's paths, derived from the fields that name the bench.
pub(super) fn formation_paths(extracted: &ExtractedFields) -> Vec<String> {
    lj_core::formation::formation_paths(
        &extracted.seat_paths,
        extracted.formation_uid.as_deref(),
        extracted.office_uid.as_deref(),
        extracted.procedure_uid.as_deref(),
    )
}

/// `CASE` de rang d'autorité sur une colonne de codes de publication, du plus
/// fort au plus faible, pour trier une liste quand la pertinence ne tranche pas.
/// Les paliers sont des constantes du code, inlinées, pas des données.
pub fn authority_rank_sql(column: &str) -> String {
    let tiers = lj_core::publication::authority_tiers();
    let mut sql = String::from("CASE");
    for (rank, codes) in tiers.iter().enumerate() {
        let list: Vec<String> = codes.iter().map(|c| format!("'{c}'")).collect(); // sql-splice-ok: paliers constants de lj-core
        sql.push_str(&format!(
            " WHEN {column} && ARRAY[{}] THEN {rank}",
            list.join(",")
        ));
    }
    sql.push_str(&format!(" ELSE {} END", tiers.len()));
    sql
}

/// Dérive le nom de source d'un `source_uid` à partir de son préfixe. Les fonds
/// DILA encodent `<fond>/<ID DILA>` ; les autres provenances héritent du repli
/// `opendata`.
pub fn source_from_source_uid(source_uid: &str) -> &'static str {
    if source_uid.starts_with("judilibre/") {
        "judilibre"
    } else if source_uid.starts_with("dila-jade/") {
        "dila-jade"
    } else if source_uid.starts_with("dila-constit/") {
        "dila-constit"
    } else if source_uid.starts_with("dila-cnil/") {
        "dila-cnil"
    } else if source_uid.starts_with("dila-cass/") {
        "dila-cass"
    } else if source_uid.starts_with("dila-inca/") {
        "dila-inca"
    } else if source_uid.starts_with("dila-capp/") {
        "dila-capp"
    } else if source_uid.starts_with("cedh/") {
        "cedh"
    } else if source_uid.starts_with("cjue/") {
        "cjue"
    } else if source_uid.starts_with("cnda/") {
        "cnda"
    } else if source_uid.starts_with("ddd/") {
        "ddd"
    } else if source_uid.starts_with("ddd-jurisprudence/") {
        "ddd-jurisprudence"
    } else if source_uid.starts_with("adlc/") {
        "adlc"
    } else if source_uid.starts_with("comeu/") {
        "comeu"
    } else if source_uid.starts_with("amf/") {
        "amf"
    } else if source_uid.starts_with("ariane-dce/") {
        "ariane-dce"
    } else if source_uid.starts_with("ariane-web/") {
        "ariane-web"
    } else if source_uid.starts_with("juricaf/") {
        "juricaf"
    } else if source_uid.starts_with("entscheidsuche/") {
        "entscheidsuche"
    } else if source_uid.starts_with("legalis/") {
        "legalis"
    } else if source_uid.starts_with("legimonaco/") {
        "legimonaco"
    } else if source_uid.starts_with("justice-lu/") {
        "justice-lu"
    } else if source_uid.starts_with("scc-csc/") {
        "scc-csc"
    } else if source_uid.starts_with("cce/") {
        "cce"
    } else if source_uid.starts_with("opencaselaw/") {
        "opencaselaw"
    } else if source_uid.starts_with("a2aj/") {
        "a2aj"
    } else if source_uid.starts_with("juportal/") {
        "juportal"
    } else if source_uid.starts_with("rechtspraak/") {
        "rechtspraak"
    } else if source_uid.starts_with("juradm-site/") {
        "juradm-site"
    } else if source_uid.starts_with("cc-site/") {
        "cc-site"
    } else if source_uid.starts_with("consiliaweb/") {
        "consiliaweb"
    } else if source_uid.starts_with("hal/") {
        "hal"
    } else if source_uid.starts_with("doctrine-oai/") {
        "doctrine-oai"
    } else if source_uid.starts_with("african-court/") {
        "african-court"
    } else if source_uid.starts_with("cc-bj/") {
        "cc-bj"
    } else if source_uid.starts_with("cc-ci/") {
        "cc-ci"
    } else if source_uid.starts_with("cc-dj/") {
        "cc-dj"
    } else if source_uid.starts_with("cc-ma/") {
        "cc-ma"
    } else if source_uid.starts_with("ccja/") {
        "ccja"
    } else if source_uid.starts_with("cassation-ci/") {
        "cassation-ci"
    } else if source_uid.starts_with("cndj-ci/") {
        "cndj-ci"
    } else if source_uid.starts_with("hcc-mg/") {
        "hcc-mg"
    } else if source_uid.starts_with("jurisprudence-mg/") {
        "jurisprudence-mg"
    } else if source_uid.starts_with("journalofficiel-dj/") {
        "journalofficiel-dj"
    } else if source_uid.starts_with("contribution/") {
        "contribution"
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

/// Timestamp courant UTC (tz-aware).
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

/// The three columns an address binds to. The date crosses the pure boundary as
/// an ISO string and becomes a `date` here; a string that does not parse binds
/// NULL, which widens the designation instead of failing the write.
pub fn address_columns(
    a: &lj_core::address::CaseAddress,
) -> (Option<&str>, Option<&str>, Option<chrono::NaiveDate>) {
    (
        a.court.as_deref(),
        a.docket.as_deref(),
        a.date
            .as_deref()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracted_column_type_covers_all_column_fields() {
        for f in REEXTRACTABLE_FIELDS {
            if is_routed_field(f) {
                continue;
            }
            let _ = extracted_column_type(f);
        }
    }

    #[test]
    fn every_ranked_source_is_named_by_its_uid() {
        let rank = crate::migrator::embedded_migrations()
            .into_iter()
            .rev()
            .find_map(|m| {
                m.sql
                    .rsplit_once("ADD COLUMN source_rank SMALLINT GENERATED")
            })
            .expect("a migration defines source_rank")
            .1;
        let ranked = rank
            .lines()
            .take_while(|l| l.trim() != "END")
            .filter_map(|l| l.trim().strip_prefix("WHEN '"))
            .filter_map(|l| l.split('\'').next())
            .filter(|s| *s != "opendata")
            .collect::<Vec<_>>();
        assert_eq!(ranked.len(), 42);
        for source in ranked {
            assert_eq!(source_from_source_uid(&format!("{source}/x")), source);
        }
    }
}
