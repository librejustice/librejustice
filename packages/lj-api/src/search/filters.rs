//! Filtres SQL (pre-filter facettes scalaires + filtres tableau tantivy/SQL).
//!
//! Depuis l'ADR 0146, les filtres référentiels (solution, voie, office,
//! domaine, instance, publication, juridiction fine) ciblent les colonnes
//! `_uid`/`jurisdiction_code` de `decisions` — plus les colonnes TEXT
//! ancien-monde. Côté jambe ANN (`decision_chunks`, qui ne porte pas ces
//! colonnes), ils passent par un `EXISTS` sur `decisions`.

use serde_json::Value;
use tokio_postgres::types::ToSql;

use lj_dtos::{Domaine, JuridictionType, SearchRequest};

/// Param dynamique : on accumule `Box<dyn ToSql>` et on génère les `$n`.
/// `Send` requis pour que les futures de handler axum restent `Send`.
pub(crate) type Params = Vec<Box<dyn ToSql + Sync + Send>>;

pub(crate) fn as_refs(params: &Params) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|b| b.as_ref() as &(dyn ToSql + Sync))
        .collect()
}

fn jt_codes(types: &[JuridictionType]) -> Vec<String> {
    types.iter().map(|t| jt_to_str(*t).to_string()).collect()
}

pub(crate) fn jt_to_str(t: JuridictionType) -> &'static str {
    match t {
        JuridictionType::Ta => "TA",
        JuridictionType::Caa => "CAA",
        JuridictionType::Ce => "CE",
        JuridictionType::Constit => "CONSTIT",
        JuridictionType::Tc => "TC",
        JuridictionType::Cc => "CC",
        JuridictionType::Ca => "CA",
        JuridictionType::Tj => "TJ",
        JuridictionType::Tcom => "TCOM",
        JuridictionType::Cedh => "CEDH",
        JuridictionType::Cjue => "CJUE",
        JuridictionType::Cnda => "CNDA",
    }
}

/// Sérialise un enum schéma vers son code SCREAMING (via serde_json).
fn enum_code<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Uids complets `facet_value` d'une sélection d'enums : `prefix:<CODE>`.
fn enum_uids<T: serde::Serialize>(prefix: &str, vs: &[T]) -> Vec<String> {
    vs.iter()
        .map(|v| format!("{prefix}:{}", enum_code(v)))
        .collect()
}

/// Expansion domaine (ADR 0146 §2) : une racine sélectionnée matche elle-même
/// et toutes ses feuilles (codes préfixés `<RACINE>_` dans `Domaine::ALL`) ;
/// une racine sans feuille (FISCAL, EUROPEEN…) ou une feuille matche elle seule.
fn domaine_uids(selected: &[Domaine]) -> Vec<String> {
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for d in selected {
        let code = enum_code(d);
        let leaf_prefix = format!("{code}_");
        out.insert(format!("domaine:{code}"));
        for other in Domaine::ALL {
            let oc = enum_code(&other);
            if oc.starts_with(&leaf_prefix) {
                out.insert(format!("domaine:{oc}"));
            }
        }
    }
    out.into_iter().collect()
}

/// Ajoute une clause `sql` (avec `$?` substitué par le prochain placeholder) +
/// son param. Mutation explicite pour éviter un emprunt long de `clauses`.
fn add_clause(
    clauses: &mut Vec<String>,
    params: &mut Params,
    idx: &mut usize,
    sql: &str,
    p: Box<dyn ToSql + Sync + Send>,
) {
    clauses.push(sql.replace("$?", &format!("${idx}")));
    params.push(p);
    *idx += 1;
}

/// Table ciblée par le filtre : `decisions` (alias `d`, toutes colonnes
/// locales) côté jambes BM25, ou `decision_chunks` (alias `c`) côté ANN — les
/// colonnes référentielles `_uid`/`jurisdiction_code` n'existent que sur
/// `decisions`, elles y sont atteintes par `EXISTS`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterTable {
    Decisions,
    Chunks,
}

/// Construit le filtre facettes scalaire d'une jambe. Renvoie le fragment SQL
/// (préfixé ` AND `), en remplissant `params` à partir de `next_idx`.
pub(crate) fn build_facet_filter(
    req: &SearchRequest,
    next_idx: &mut usize,
    params: &mut Params,
    arrays: ArrayFilters,
    table: FilterTable,
) -> String {
    let alias = match table {
        FilterTable::Decisions => "d",
        FilterTable::Chunks => "c",
    };
    let mut clauses: Vec<String> = Vec::new();
    // Colonnes présentes sur les deux tables (dénormalisées côté chunks).
    if let Some(types) = &req.juridiction_type {
        add_clause(
            &mut clauses,
            params,
            next_idx,
            &format!("{alias}.juridiction_type = ANY($?)"),
            Box::new(jt_codes(types)),
        );
    }
    if arrays == ArrayFilters::AsSql {
        for (col, values) in array_filter_values(req) {
            add_clause(
                &mut clauses,
                params,
                next_idx,
                &format!("{alias}.{col} && $?::text[]"),
                Box::new(values),
            );
        }
        // Portée (ADR 0167) : `publication_codes` est dénormalisée sur les deux
        // tables — clause directe sur l'alias local (le détour `EXISTS fd` payait
        // un lookup `decisions` par candidat du prefilter ANN). Côté jambes BM25
        // (`Tantivy`), le filtre est composé DANS le `@@@` par
        // [`build_array_filter_queries`] : le `&&` SQL y devenait un
        // `heap_filter` — et sa négation un `must_not` évalué sur tout l'index
        // (3,2 s vs 1,1 s mesurés sur la requête à 4 mots, 2026-07-06).
        if let Some(v) = &req.portee {
            if let Some(clause) = portee_clause(alias, v) {
                clauses.push(clause);
            }
        }
    }
    if let Some(d) = &req.date_from {
        add_clause(
            &mut clauses,
            params,
            next_idx,
            &format!("{alias}.date_lecture >= $?::text::date"),
            Box::new(d.clone()),
        );
    }
    if let Some(d) = &req.date_to {
        add_clause(
            &mut clauses,
            params,
            next_idx,
            &format!("{alias}.date_lecture <= $?::text::date"),
            Box::new(d.clone()),
        );
    }
    // Colonnes référentielles (ADR 0146), sur `decisions` uniquement.
    let mut ref_clauses: Vec<String> = Vec::new();
    let ref_alias = match table {
        FilterTable::Decisions => "d",
        FilterTable::Chunks => "fd",
    };
    if let Some(v) = &req.solution {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.solution_uid = ANY($?)"),
            Box::new(enum_uids("solution", v)),
        );
    }
    if let Some(v) = &req.voie {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.voie_uid = ANY($?)"),
            Box::new(enum_uids("voie", v)),
        );
    }
    if let Some(v) = &req.office {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.office_uid = ANY($?)"),
            Box::new(enum_uids("office", v)),
        );
    }
    if let Some(v) = &req.legal_domain {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.legal_domain_uid = ANY($?)"),
            Box::new(domaine_uids(v)),
        );
    }
    if let Some(v) = nonempty(&req.jurisdiction_code) {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.jurisdiction_code = ANY($?)"),
            Box::new(v),
        );
    }
    if let Some(v) = nonempty(&req.publication) {
        add_clause(
            &mut ref_clauses,
            params,
            next_idx,
            &format!("{ref_alias}.publication_uid = ANY($?)"),
            Box::new(
                v.into_iter()
                    .map(|s| format!("publication:{s}"))
                    .collect::<Vec<String>>(),
            ),
        );
    }
    if !ref_clauses.is_empty() {
        match table {
            FilterTable::Decisions => clauses.extend(ref_clauses),
            FilterTable::Chunks => clauses.push(format!(
                "EXISTS (SELECT 1 FROM decisions fd WHERE fd.id = c.decision_id AND {})",
                ref_clauses.join(" AND ")
            )),
        }
    }
    if clauses.is_empty() {
        String::new()
    } else {
        format!(" AND {}", clauses.join(" AND "))
    }
}

/// Littéral SQL `ARRAY['r','A']` d'un groupe de portée (codes constants du
/// référentiel lj-core, jamais d'entrée utilisateur — pas de paramètre).
fn portee_sql_array(groups: &[&str]) -> String {
    let codes: Vec<String> = groups
        .iter()
        .flat_map(|g| lj_core::publication::portee_codes(g))
        .map(|c| format!("'{c}'"))
        .collect();
    format!("ARRAY[{}]", codes.join(","))
}

/// `term_set` tantivy d'un groupe de portée sur `publication_codes` (champ
/// `pdb.literal` de `decisions_bm25` : termes exacts, pas de lowercasing —
/// parité `&&` vérifiée en prod, `r`≠`R`, `C+` intact).
fn portee_term_set(groups: &[&str]) -> String {
    let terms: Vec<String> = groups
        .iter()
        .flat_map(|g| lj_core::publication::portee_codes(g))
        .map(|c| format!("paradedb.term('publication_codes', '{c}')"))
        .collect();
    if terms.len() == 1 {
        terms.into_iter().next().expect("non vide")
    } else {
        format!("paradedb.term_set(terms => ARRAY[{}])", terms.join(", "))
    }
}

/// Filtre `portee` composé dans le `@@@` des jambes BM25 (mêmes sémantiques
/// rang-le-plus-fort que [`portee_clause`], mais en requêtes tantivy indexées :
/// la négation passe par un `must_not` sur l'index inversé au lieu d'un
/// `heap_filter` sur tout le corpus). `None` si la sélection est vide.
fn portee_tantivy(selected: &[lj_dtos::Portee]) -> Option<String> {
    use lj_dtos::Portee;
    let majeure = || portee_term_set(&["majeure"]);
    let importante = || portee_term_set(&["importante"]);
    let limitee = || portee_term_set(&["limitee"]);
    let mut branches: Vec<String> = Vec::new();
    for p in selected {
        let branch = match p {
            Portee::Majeure => majeure(),
            Portee::Importante => format!(
                "paradedb.boolean(must => ARRAY[{}], must_not => ARRAY[{}])",
                importante(),
                majeure()
            ),
            Portee::Limitee => format!(
                "paradedb.boolean(must => ARRAY[{}], must_not => ARRAY[{}])",
                limitee(),
                portee_term_set(&["majeure", "importante"])
            ),
            Portee::Indeterminee => format!(
                "paradedb.boolean(must => ARRAY[paradedb.all()], must_not => ARRAY[{}])",
                portee_term_set(&["majeure", "importante", "limitee"])
            ),
        };
        if !branches.contains(&branch) {
            branches.push(branch);
        }
    }
    match branches.len() {
        0 => None,
        1 => branches.into_iter().next(),
        _ => Some(format!(
            "paradedb.boolean(should => ARRAY[{}])",
            branches.join(", ")
        )),
    }
}

/// Clause `portee` (ADR 0167) : sémantique **rang le plus fort** — une décision
/// `{b,r}` est majeure, pas importante ; un simple `&&` par groupe la
/// compterait deux fois. Chaque groupe matche son overlap MOINS les groupes
/// plus forts ; `INDETERMINEE` = aucun code classant. Sélections OR-ées.
/// `None` si la sélection est vide (aucune contrainte).
fn portee_clause(alias: &str, selected: &[lj_dtos::Portee]) -> Option<String> {
    use lj_dtos::Portee;
    let col = format!("{alias}.publication_codes");
    let majeure = portee_sql_array(&["majeure"]);
    let importante = portee_sql_array(&["importante"]);
    let limitee = portee_sql_array(&["limitee"]);
    let classant = portee_sql_array(&["majeure", "importante", "limitee"]);
    let m_i = portee_sql_array(&["majeure", "importante"]);
    let mut ors: Vec<String> = Vec::new();
    for p in selected {
        let cond = match p {
            Portee::Majeure => format!("{col} && {majeure}"),
            Portee::Importante => {
                format!("({col} && {importante} AND NOT {col} && {majeure})")
            }
            Portee::Limitee => format!("({col} && {limitee} AND NOT {col} && {m_i})"),
            Portee::Indeterminee => format!("NOT {col} && {classant}"),
        };
        if !ors.contains(&cond) {
            ors.push(cond);
        }
    }
    if ors.is_empty() {
        None
    } else {
        Some(format!("({})", ors.join(" OR ")))
    }
}

/// Sort des filtres tableau (`legal_instrument`, `legal_article` composite)
/// dans `build_facet_filter`.
///
/// - Jambes BM25 (`@@@` sur `decisions_bm25`) : `Tantivy` — le `&&` SQL sur un
///   fast field multi-valeurs est évalué par scan de colonne (~40 s mesurés sur
///   `chunks_bm25` v2) là où le term/term_set passe par l'index inversé
///   (~0,15 s). Les filtres sont composés dans la requête via
///   [`compose_tantivy_query`].
/// - Jambe ANN (pas de `@@@`) : `AsSql` — `&&` évalué par le prefilter
///   VectorChord sur le heap, comme avant.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrayFilters {
    AsSql,
    Tantivy,
}

/// `(colonne, valeurs)` des filtres tableau actifs (sémantique `&&` = overlap).
fn array_filter_values(req: &SearchRequest) -> Vec<(&'static str, Vec<String>)> {
    [
        ("legal_instruments", nonempty(&req.legal_instrument)),
        ("legal_article_composite", nonempty(&req.legal_article)),
    ]
    .into_iter()
    .filter_map(|(col, v)| v.map(|values| (col, values)))
    .collect()
}

/// Requêtes tantivy des filtres tableau — `paradedb.term` (1 valeur) ou
/// `paradedb.term_set` (overlap = ANY) — plus le filtre portée composé
/// ([`portee_tantivy`]). À AND-er dans le `@@@` via [`compose_tantivy_query`].
pub(crate) fn build_array_filter_queries(
    req: &SearchRequest,
    next_idx: &mut usize,
    params: &mut Params,
) -> Vec<String> {
    let mut out = Vec::new();
    for (col, values) in array_filter_values(req) {
        let terms: Vec<String> = values
            .into_iter()
            .map(|v| {
                params.push(Box::new(v) as Box<dyn ToSql + Sync + Send>);
                let expr = format!("paradedb.term('{col}', ${next_idx}::text)");
                *next_idx += 1;
                expr
            })
            .collect();
        out.push(if terms.len() == 1 {
            terms.into_iter().next().expect("non vide")
        } else {
            format!("paradedb.term_set(terms => ARRAY[{}])", terms.join(", "))
        });
    }
    if let Some(v) = &req.portee {
        if let Some(expr) = portee_tantivy(v) {
            out.push(expr);
        }
    }
    out
}

/// Compose l'expression `@@@` d'une jambe BM25 : la requête primaire seule,
/// ou `paradedb.boolean(must => …)` avec les filtres tableau in-index.
pub(crate) fn compose_tantivy_query(primary: &str, filters: Vec<String>) -> String {
    if filters.is_empty() {
        primary.to_string()
    } else {
        format!(
            "paradedb.boolean(must => ARRAY[{primary}, {}])",
            filters.join(", ")
        )
    }
}

pub(crate) fn nonempty(v: &Option<Vec<String>>) -> Option<Vec<String>> {
    let filtered: Vec<String> = v
        .as_ref()
        .map(|x| x.iter().filter(|s| !s.is_empty()).cloned().collect())
        .unwrap_or_default();
    if filtered.is_empty() {
        None
    } else {
        Some(filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domaine_root_expands_to_leaves() {
        // CIVIL (racine à feuilles) : elle-même + ses 15 feuilles, rien d'autre.
        let uids = domaine_uids(&[Domaine::Civil]);
        assert!(uids.contains(&"domaine:CIVIL".to_string()));
        assert!(uids.contains(&"domaine:CIVIL_DROIT_LOCATIF".to_string()));
        assert_eq!(uids.len(), 16);
        assert!(uids.iter().all(|u| u.starts_with("domaine:CIVIL")));
        // FISCAL (racine sans feuille) : elle seule.
        assert_eq!(
            domaine_uids(&[Domaine::Fiscal]),
            vec!["domaine:FISCAL".to_string()]
        );
        // Une feuille : elle seule.
        assert_eq!(
            domaine_uids(&[Domaine::SocialDroitTravail]),
            vec!["domaine:SOCIAL_DROIT_TRAVAIL".to_string()]
        );
    }

    #[test]
    fn portee_filter_routes_by_leg_with_strongest_rank_semantics() {
        let req: SearchRequest = serde_json::from_value(serde_json::json!({
            "query": "x",
            "portee": ["IMPORTANTE", "INDETERMINEE"],
        }))
        .unwrap();
        // Jambes BM25 : composé DANS le `@@@` (index inversé), absent du
        // fragment SQL — le `&&`/`NOT &&` en SQL y devenait un heap_filter
        // plein-corpus (3,2 s vs 1,1 s mesurés, 2026-07-06).
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::Tantivy,
            FilterTable::Decisions,
        );
        assert!(!frag.contains("publication_codes"));
        let queries = build_array_filter_queries(&req, &mut idx, &mut params);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];
        // Importante = must importante, must_not majeure ; indéterminée =
        // all() must_not tout code classant ; sélections OR-ées par `should`.
        assert!(q.contains("paradedb.term('publication_codes', 'b')"));
        assert!(q.contains("must_not => ARRAY[paradedb.term_set(terms => ARRAY[paradedb.term('publication_codes', 'r'), paradedb.term('publication_codes', 'A')])]"));
        assert!(q.contains("paradedb.all()"));
        assert!(q.starts_with("paradedb.boolean(should => ARRAY["));
        assert!(params.is_empty());

        // Jambe ANN : clause SQL directe sur la colonne dénormalisée du chunk
        // (pas d'EXISTS vers `decisions`).
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::AsSql,
            FilterTable::Chunks,
        );
        assert!(frag.contains(
            "(c.publication_codes && ARRAY['b','l','c','B','C+','R'] \
             AND NOT c.publication_codes && ARRAY['r','A'])"
        ));
        assert!(frag.contains(
            "NOT c.publication_codes && ARRAY['r','A','b','l','c','B','C+','R','n','C','D','Z']"
        ));
        assert!(!frag.contains("EXISTS"));
        assert!(params.is_empty());
    }

    #[test]
    fn chunk_filter_reaches_uid_columns_via_exists() {
        let req: SearchRequest = serde_json::from_value(serde_json::json!({
            "query": "x",
            "solution": ["REJET"],
            "publication": ["PUBLIE_BULLETIN"],
            "jurisdictionCode": ["tj76351"],
        }))
        .unwrap();
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::AsSql,
            FilterTable::Chunks,
        );
        assert!(frag.contains("EXISTS (SELECT 1 FROM decisions fd WHERE fd.id = c.decision_id"));
        assert!(frag.contains("fd.solution_uid = ANY($1)"));
        assert!(frag.contains("fd.jurisdiction_code = ANY($2)"));
        assert!(frag.contains("fd.publication_uid = ANY($3)"));
        assert_eq!(params.len(), 3);

        // Même requête côté decisions : clauses directes sur `d`.
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let frag = build_facet_filter(
            &req,
            &mut idx,
            &mut params,
            ArrayFilters::Tantivy,
            FilterTable::Decisions,
        );
        assert!(!frag.contains("EXISTS"));
        assert!(frag.contains("d.solution_uid = ANY($1)"));
    }
}
