//! Exact total and facets, counted in `decision_text_bm25` by a single
//! `paradedb.aggregate` rather than on the page fetched, which only gave a lower
//! bound: 26 jurisdictions out of 376 on `responsabilite contractuelle`, the
//! 350 missing being valid filters nobody could reach.
//!
//! Piège de l'appel : il ne prend qu'un prédicat, sans clause SQL. Un filtre qui
//! ne s'y traduit pas fait porter les décomptes sur un ensemble plus large que
//! les résultats rendus.

use deadpool_postgres::Client;
use serde_json::Value;
use tokio_postgres::types::ToSql;

use lj_dtos::{SearchMode, SearchRequest, SortOrder};

use super::filters::{
    as_refs, build_array_filter_queries, enum_uids, jt_codes, legal_domain_uids, nonempty, present,
    term_any, Params,
};
use super::hydrate::FacetCounters;

/// Aggregation tree. The seat and formation axes are flat: a path carries its own
/// hierarchy, so one `terms` renders every seat node of the corpus where the
/// type/court/chamber nest asked for three.
///
/// Sizes set with margin over the vocabulary: `sum_other_doc_count` is 0
/// everywhere, citations excepted, truncated by nature. The seat cap is global,
/// not nested under a court: below the seat vocabulary it truncates the tree with
/// no error and no log.
fn agg() -> &'static str {
    r#"{
  "seat": {"terms": {"field": "seat_paths", "size": 4000}},
  "dom": {"terms": {"field": "legal_domain_uid", "size": 100}},
  "sol": {"terms": {"field": "solution_uid", "size": 50}},
  "form": {"terms": {"field": "formation_paths", "size": 200}},
  "pub": {"terms": {"field": "publication_codes", "size": 50}},
  "jour": {"date_histogram": {"field": "date_lecture", "fixed_interval": "1d", "min_doc_count": 1}},
  "inst": {"terms": {"field": "legal_instruments", "size": 30}},
  "art": {"terms": {"field": "legal_article_composite", "size": 300}},
  "total": {"value_count": {"field": "id"}}
}"#
}

/// Plage de dates en `daterange`, `date_to` inclusive comme le filtre SQL.
fn date_range(req: &SearchRequest, next_idx: &mut usize, params: &mut Params) -> Option<String> {
    let literal = match (req.date_from.as_deref(), req.date_to.as_deref()) {
        (None, None) => return None,
        (Some(f), Some(t)) => format!("[{f},{t}]"),
        (Some(f), None) => format!("[{f},)"),
        (None, Some(t)) => format!("(,{t}]"),
    };
    params.push(Box::new(literal) as Box<dyn ToSql + Sync + Send>);
    let expr = format!("paradedb.range('date_lecture', ${next_idx}::text::daterange)");
    *next_idx += 1;
    Some(expr)
}

/// The question in the parts asked. Run in every part, it takes the title too:
/// without it, a decision only its usual name designates would be served and
/// counted zero. The lexical mode and a date order ask every word, as `ET` does.
fn question(req: &SearchRequest, next_idx: &mut usize, params: &mut Params) -> Option<String> {
    if super::query::query_lacks_searchable_terms(&req.query) {
        return None;
    }
    let boolean = req.mode == SearchMode::Lexical
        || req.sort != SortOrder::Relevance
        || super::query::is_boolean_query(&req.query);
    let body = if boolean {
        super::query::translate_boolean(&req.query)
    } else {
        super::query::phrase_combo_parse(&req.query)
    };
    let bi = *next_idx;
    params.push(Box::new(body) as Box<dyn ToSql + Sync + Send>);
    let parts = super::filters::parts_query(req, &format!("${bi}::text"));
    if !super::filters::every_part(req) {
        *next_idx += 1;
        return Some(parts);
    }
    let title_expr = if boolean {
        params
            .push(Box::new(super::query::translate_boolean(&req.query))
                as Box<dyn ToSql + Sync + Send>);
        format!(
            "paradedb.parse_with_field('search_title', ${}::text)",
            bi + 1
        )
    } else {
        params.push(Box::new(req.query.clone()) as Box<dyn ToSql + Sync + Send>);
        format!("paradedb.match('search_title', ${}::text)", bi + 1)
    };
    *next_idx += 2;
    Some(format!(
        "paradedb.boolean(should => ARRAY[{parts}, {title_expr}], minimum_should_match => 1)"
    ))
}

struct PredicateParts {
    question: Option<String>,
    filters: Vec<String>,
}

/// Prédicat tantivy de la requête entière. Sans le `must_not`, l'agrégat
/// compterait le fonds mort avec le vif. Le parcours s'en sert aussi pour lister
/// sa page, l'index sachant trier et tronquer ce qu'il vient de compter.
pub(super) fn build_predicate(
    req: &SearchRequest,
    next_idx: &mut usize,
    params: &mut Params,
) -> String {
    compose(&predicate_parts(req, next_idx, params))
}

/// Without a question, a decision counts through its lead text alone.
const LEAD: &str = "paradedb.term('lead', true)";

fn compose(parts: &PredicateParts) -> String {
    let mut must: Vec<String> = parts.question.iter().cloned().collect();
    if must.is_empty() {
        must.push(LEAD.to_string());
    }
    must.extend(parts.filters.iter().cloned());
    format!(
        "paradedb.boolean(must => ARRAY[{}], must_not => ARRAY[paradedb.exists('deleted_at')])",
        must.join(", ")
    )
}

fn compose_complement(parts: &PredicateParts, ids: &[i64]) -> Option<String> {
    let question = parts.question.as_deref().filter(|_| !ids.is_empty())?;
    Some(compose_ids(&parts.filters, ids, Some(question)))
}

fn compose_ids(filters: &[String], ids: &[i64], except: Option<&str>) -> String {
    let terms: Vec<String> = ids
        .iter()
        .map(|id| format!("paradedb.term('decision_id', {id}::bigint)"))
        .collect();
    let mut must = vec![
        format!("paradedb.term_set(terms => ARRAY[{}])", terms.join(", ")),
        LEAD.to_string(),
    ];
    must.extend(filters.iter().cloned());
    let mut must_not: Vec<&str> = except.into_iter().collect();
    must_not.push("paradedb.exists('deleted_at')");
    format!(
        "paradedb.boolean(must => ARRAY[{}], must_not => ARRAY[{}])",
        must.join(", "),
        must_not.join(", ")
    )
}

fn predicate_parts(
    req: &SearchRequest,
    next_idx: &mut usize,
    params: &mut Params,
) -> PredicateParts {
    let question = question(req, next_idx, params);
    PredicateParts {
        question,
        filters: filter_parts(req, next_idx, params),
    }
}

fn filter_parts(req: &SearchRequest, next_idx: &mut usize, params: &mut Params) -> Vec<String> {
    let mut must: Vec<String> = Vec::new();
    if let Some(types) = present(&req.jurisdiction_type) {
        must.push(term_any(
            "jurisdiction_type",
            &jt_codes(types),
            next_idx,
            params,
        ));
    }
    for (field, uids) in [
        (
            "solution_uid",
            present(&req.solution).map(|v| enum_uids("solution", v)),
        ),
        (
            "legal_domain_uid",
            present(&req.legal_domain).map(legal_domain_uids),
        ),
    ] {
        if let Some(uids) = uids {
            must.push(term_any(field, &uids, next_idx, params));
        }
    }
    if let Some(codes) = nonempty(&req.jurisdiction_code) {
        must.push(term_any("jurisdiction_code", &codes, next_idx, params));
    }
    if let Some(expr) = date_range(req, next_idx, params) {
        must.push(expr);
    }
    must.extend(build_array_filter_queries(req, next_idx, params));
    must
}

/// What the aggregate describes. `Served` exists because `reference` matches no
/// content: its question bounds nothing, so counting it counts the whole corpus.
pub(crate) enum Scope<'a> {
    /// The question, plus the served ids another leg brought in.
    Question(&'a [i64]),
    /// The served ids alone.
    Served(&'a [i64]),
}

/// Which values the aggregate counts. `Keys` names them: `include` bounds the
/// term dictionary and the predicate gains their union, without which the scan
/// covers the whole corpus.
pub(crate) enum Aggs<'a> {
    /// The eight axes of the panel, each capped at the head of its vocabulary.
    All,
    /// The given keys and nothing else, counted exactly whatever their rank.
    Keys {
        instruments: &'a [String],
        articles: &'a [String],
    },
}

impl Aggs<'_> {
    /// The tree, sized to what it counts: a named key is never truncated, and
    /// one absent from the buckets is a zero rather than a value out of the top.
    fn tree(&self) -> String {
        match self {
            Aggs::All => agg().to_string(),
            Aggs::Keys {
                instruments,
                articles,
            } => {
                let terms = |field: &str, keys: &[String]| {
                    let include: Vec<String> = keys.iter().map(|k| regex_literal(k)).collect();
                    serde_json::json!({"terms": {
                        "field": field,
                        "size": keys.len().max(1),
                        "include": include.join("|"),
                    }})
                };
                serde_json::json!({
                    "inst": terms("legal_instruments", instruments),
                    "art": terms("legal_article_composite", articles),
                })
                .to_string()
            }
        }
    }

    /// The union of the named keys, as a `must` of the predicate.
    fn restriction(&self, next_idx: &mut usize, params: &mut Params) -> Option<String> {
        let Aggs::Keys {
            instruments,
            articles,
        } = self
        else {
            return None;
        };
        let sets: Vec<String> = [
            ("legal_instruments", *instruments),
            ("legal_article_composite", *articles),
        ]
        .into_iter()
        .filter(|(_, keys)| !keys.is_empty())
        .map(|(field, keys)| term_any(field, keys, next_idx, params))
        .collect();
        Some(format!(
            "paradedb.boolean(should => ARRAY[{}], minimum_should_match => 1)",
            sets.join(", ")
        ))
    }
}

/// A key as a regular expression literal, `include` taking one: a composite key
/// carries the `|` that separates alternatives there.
fn regex_literal(key: &str) -> String {
    key.chars()
        .flat_map(|c| match c.is_ascii_alphanumeric() {
            true => vec![c],
            false => vec!['\\', c],
        })
        .collect()
}

/// Total and counters of the matched set, citations included since they come
/// out of the same scan. They alone are not exhaustive.
pub(crate) async fn aggregate_facets(
    conn: &Client,
    req: &SearchRequest,
    scope: Scope<'_>,
    aggs: Aggs<'_>,
) -> std::result::Result<(i64, FacetCounters), tokio_postgres::Error> {
    let mut params: Params = Vec::new();
    let mut idx = 1usize;
    let restriction = aggs.restriction(&mut idx, &mut params);
    let (predicate, complement, every_text) = match scope {
        Scope::Served([]) => return Ok((0, FacetCounters::default())),
        Scope::Served(ids) => {
            let mut filters = filter_parts(req, &mut idx, &mut params);
            filters.extend(restriction);
            (compose_ids(&filters, ids, None), None, false)
        }
        Scope::Question(hors_question) => {
            let mut parts = predicate_parts(req, &mut idx, &mut params);
            parts.filters.extend(restriction);
            let every_text = parts.question.is_some();
            (
                compose(&parts),
                compose_complement(&parts, hors_question),
                every_text,
            )
        }
    };
    let predicate_params = params.len();
    // The index carries tokens, not labels, and the catalogue is large: the
    // title joins in the same statement rather than in memory.
    params.push(Box::new(aggs.tree()) as Box<dyn ToSql + Sync + Send>);
    let agg = format!("${idx}::text::json");
    let sql = format!(
        "WITH agg AS (\
           SELECT paradedb.aggregate('decision_text_bm25', {predicate}, \
             {agg}, bucket_limit => 1000000) AS j, \
             {sup} AS k\
         ) \
         SELECT j::text, \
                (SELECT jsonb_object_agg(lt.text_uid, jsonb_build_array(lt.title, lt.slug)) \
                 FROM jsonb_array_elements(\
                        coalesce(j->'inst'->'buckets', '[]'::jsonb) \
                        || coalesce(k->'inst'->'buckets', '[]'::jsonb)) b \
                 JOIN legal_text lt ON lt.text_uid = b->>'key')::text, \
                k::text \
         FROM agg",
        sup = match &complement {
            Some(p) => format!(
                "paradedb.aggregate('decision_text_bm25', {p}, {agg}, bucket_limit => 1000000)"
            ),
            None => "'{}'::jsonb".to_string(),
        },
    );
    let refs = as_refs(&params);
    let (row, duplicates) = futures::try_join!(conn.query_one(sql.as_str(), &refs), async {
        if every_text {
            duplicate_texts(conn, &predicate, &refs[..predicate_params]).await
        } else {
            Ok(Vec::new())
        }
    })?;
    let raw: String = row.get(0);
    let json: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let mut counters = FacetCounters::default();
    let mut total = accumulate_counters(&json, &mut counters, 1);
    if let Some(raw_sup) = row.get::<_, Option<String>>(2) {
        let sup: Value = serde_json::from_str(&raw_sup).unwrap_or(Value::Null);
        total += accumulate_counters(&sup, &mut counters, 1);
    }
    if !duplicates.is_empty() {
        let terms: Vec<String> = duplicates
            .iter()
            .map(|id| format!("paradedb.term('id', {id}::bigint)"))
            .collect();
        let sql = format!(
            "SELECT paradedb.aggregate('decision_text_bm25', \
               paradedb.term_set(terms => ARRAY[{}]), $1::text::json, bucket_limit => 1000000)::text",
            terms.join(", ")
        );
        let row = conn.query_one(sql.as_str(), &[&aggs.tree()]).await?;
        let dup: Value = serde_json::from_str(&row.get::<_, String>(0)).unwrap_or(Value::Null);
        total += accumulate_counters(&dup, &mut counters, -1);
    }
    let libelles: Value = row
        .get::<_, Option<String>>(1)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    for (uid, paire) in libelles.as_object().into_iter().flatten() {
        if let Some(titre) = paire.get(0).and_then(Value::as_str) {
            counters
                .instrument
                .labels
                .insert(uid.clone(), titre.to_string());
        }
        counters.instrument.slugs.insert(
            uid.clone(),
            paire.get(1).and_then(Value::as_str).map(str::to_string),
        );
    }
    Ok((total, counters))
}

/// The matched texts beyond the first of each decision, its lead first: a
/// decision matched in several languages counts once.
async fn duplicate_texts(
    conn: &Client,
    predicate: &str,
    params: &[&(dyn ToSql + Sync)],
) -> std::result::Result<Vec<i64>, tokio_postgres::Error> {
    let sql = format!(
        "SELECT DISTINCT t.decision_id FROM decision_text t \
         WHERE t.id @@@ paradedb.boolean(must => ARRAY[{predicate}, paradedb.term('lead', false)])"
    );
    let decisions: Vec<i64> = conn
        .query(sql.as_str(), params)
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    if decisions.is_empty() {
        return Ok(Vec::new());
    }
    let terms: Vec<String> = decisions
        .iter()
        .map(|id| format!("paradedb.term('decision_id', {id}::bigint)"))
        .collect();
    let sql = format!(
        "SELECT t.id, t.decision_id FROM decision_text t \
         WHERE t.id @@@ paradedb.boolean(must => ARRAY[{predicate}, \
           paradedb.term_set(terms => ARRAY[{}])]) \
         ORDER BY t.decision_id, t.lead DESC, t.id",
        terms.join(", ")
    );
    let mut previous = None;
    let mut out = Vec::new();
    for r in conn.query(sql.as_str(), params).await? {
        let decision: i64 = r.get(1);
        if previous == Some(decision) {
            out.push(r.get(0));
        }
        previous = Some(decision);
    }
    Ok(out)
}

fn buckets(node: &Value, key: &str) -> Vec<Value> {
    node.get(key)
        .and_then(|a| a.get("buckets"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn bucket_key(b: &Value) -> Option<String> {
    b.get("key").and_then(Value::as_str).map(str::to_string)
}

fn doc_count(b: &Value) -> i64 {
    b.get("doc_count").and_then(Value::as_i64).unwrap_or(0)
}

fn accumulate_counters(json: &Value, c: &mut FacetCounters, sign: i64) -> i64 {
    for (axe, dest) in [
        ("seat", &mut c.seats),
        ("form", &mut c.formation),
        ("dom", &mut c.domain),
        ("sol", &mut c.solution),
    ] {
        for b in buckets(json, axe) {
            if let Some(uid) = bucket_key(&b) {
                *dest.entry(uid).or_insert(0) += sign * doc_count(&b);
            }
        }
    }
    // The aggregate returns one bucket per source code, the facet one value per
    // group. The sum is exact because no decision carries two codes of one group.
    for b in buckets(json, "pub") {
        let Some(code) = bucket_key(&b) else { continue };
        for key in lj_core::publication::publication_keys(&[code]) {
            *c.publication
                .entry(format!("publication:{key}"))
                .or_insert(0) += sign * doc_count(&b);
        }
    }
    // Un jour ne chevauche jamais deux années civiles, donc replier les seaux
    // d'un jour par millésime est exact. C'est ce qui dispense d'une colonne
    // `annee`, qu'aucune expression indexée ne sait rendre juste.
    for b in buckets(json, "jour") {
        let Some(annee) = b
            .get("key_as_string")
            .and_then(Value::as_str)
            .and_then(|s| s.get(..4))
        else {
            continue;
        };
        *c.year.entry(annee.to_string()).or_insert(0) += sign * doc_count(&b);
    }
    for b in buckets(json, "inst") {
        if let Some(uid) = bucket_key(&b) {
            *c.instrument.counts.entry(uid).or_insert(0) += sign * doc_count(&b);
        }
    }
    // The composite carries its instrument as a prefix, which files the articles
    // under their text without nesting the two axes, which runs several times slower.
    for b in buckets(json, "art") {
        let Some(composite) = bucket_key(&b) else {
            continue;
        };
        let Some((uid, num_key)) = composite.split_once('|') else {
            continue;
        };
        if !c.instrument.counts.contains_key(uid) {
            continue;
        }
        *c.instrument
            .articles
            .entry(uid.to_string())
            .or_default()
            .entry(num_key.to_string())
            .or_insert(0) += sign * doc_count(&b);
    }
    sign * json
        .get("total")
        .and_then(|t| t.get("value"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(v: serde_json::Value) -> SearchRequest {
        serde_json::from_value(v).expect("requête valide")
    }

    /// The seat is one axis: the ticked paths enter as one `term_set`, and a
    /// path matches everything under it.
    #[test]
    fn le_siege_entre_en_un_seul_term_set() {
        let r = req(serde_json::json!({
            "query": "",
            "seat": ["CC.SOCIALE", "CA.PARIS.C3"],
        }));
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let p = build_predicate(&r, &mut idx, &mut params);
        assert!(p.contains("paradedb.term_set"), "{p}");
        assert!(p.contains("paradedb.term('seat_paths'"), "{p}");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn sans_question_ni_filtre_le_predicat_est_tout_le_fonds_vif() {
        let p = build_predicate(
            &req(serde_json::json!({"query": ""})),
            &mut 1,
            &mut Vec::new(),
        );
        assert_eq!(
            p,
            "paradedb.boolean(must => ARRAY[paradedb.term('lead', true)], \
             must_not => ARRAY[paradedb.exists('deleted_at')])"
        );
    }

    /// Le second agrégat compte les servies que la question n'apparie pas :
    /// question en `must_not`, filtres des deux côtés, et les mêmes `$n` que le
    /// prédicat de base puisque les deux entrent dans une instruction unique.
    #[test]
    fn le_complement_soustrait_la_question_et_garde_les_filtres() {
        let r = req(serde_json::json!({
            "query": "conges payes", "jurisdictionType": ["TJ"],
        }));
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let parts = predicate_parts(&r, &mut idx, &mut params);
        let base = compose(&parts);
        let comp = compose_complement(&parts, &[7, 9]).expect("question posée");
        assert_eq!(params.len(), 3, "corps, titre, type de juridiction");
        for p in [&base, &comp] {
            assert!(
                p.contains("paradedb.term('jurisdiction_type', $3::text)"),
                "{p}"
            );
        }
        let (must, must_not) = comp.split_once("must_not =>").expect("must_not posé");
        assert!(
            must.contains("paradedb.term('decision_id', 7::bigint)"),
            "{must}"
        );
        assert!(
            must.contains("paradedb.term('decision_id', 9::bigint)"),
            "{must}"
        );
        assert!(must.contains("paradedb.term('lead', true)"), "{must}");
        assert!(!must.contains("search_title"), "{must}");
        assert!(must_not.contains("search_title"), "{must_not}");
        assert!(
            must_not.contains("paradedb.exists('deleted_at')"),
            "{must_not}"
        );
    }

    /// The served ids count without the question, which matches on either token
    /// of the title and would count far more decisions than were served.
    #[test]
    fn les_servies_se_comptent_sans_la_question() {
        let r = req(serde_json::json!({
            "query": "dupont conseil d'etat", "jurisdictionType": ["CE"],
        }));
        let mut params: Params = Vec::new();
        let mut idx = 1usize;
        let filters = filter_parts(&r, &mut idx, &mut params);
        let p = compose_ids(&filters, &[7, 9], None);
        assert_eq!(params.len(), 1, "le type de juridiction seul");
        assert!(
            p.contains("paradedb.term('jurisdiction_type', $1::text)"),
            "{p}"
        );
        assert!(p.contains("paradedb.term('decision_id', 7::bigint)"), "{p}");
        assert!(p.contains("paradedb.term('lead', true)"), "{p}");
        assert!(
            !p.contains("search_title") && !p.contains("full_text"),
            "{p}"
        );
        assert!(
            p.contains("must_not => ARRAY[paradedb.exists('deleted_at')]"),
            "{p}"
        );
    }

    #[test]
    fn sans_question_le_complement_n_existe_pas() {
        let mut params: Params = Vec::new();
        let parts = predicate_parts(
            &req(serde_json::json!({"query": "", "jurisdictionType": ["TJ"]})),
            &mut 1,
            &mut params,
        );
        assert!(compose_complement(&parts, &[7]).is_none());
    }

    #[test]
    fn les_supprimees_sortent_toujours() {
        for q in ["", "responsabilite"] {
            let p = build_predicate(
                &req(serde_json::json!({"query": q})),
                &mut 1,
                &mut Vec::new(),
            );
            assert!(
                p.contains("must_not => ARRAY[paradedb.exists('deleted_at')]"),
                "{p}"
            );
        }
    }

    /// Les bornes de dates passent en paramètre, jamais inlinées.
    #[test]
    fn les_bornes_de_dates_sont_un_daterange_parametre() {
        let mut params: Params = Vec::new();
        let r = req(serde_json::json!({
            "query": "", "dateFrom": "2020-01-01", "dateTo": "2021-06-30",
        }));
        let p = build_predicate(&r, &mut 1, &mut params);
        assert!(
            p.contains("paradedb.range('date_lecture', $1::text::daterange)"),
            "{p}"
        );
        assert_eq!(params.len(), 1);

        let mut params: Params = Vec::new();
        let r = req(serde_json::json!({"query": "", "dateTo": "2021-06-30"}));
        let _ = build_predicate(&r, &mut 1, &mut params);
        assert_eq!(params.len(), 1);
    }

    /// Les axes référentiels portent l'uid complet, pas le suffixe.
    #[test]
    fn les_axes_referentiels_portent_l_uid_complet() {
        let mut params: Params = Vec::new();
        let r = req(serde_json::json!({
            "query": "", "solution": ["REJET"], "formation": ["FR.TJ.JEX", "FR.CC.RESTREINTE"],
        }));
        let p = build_predicate(&r, &mut 1, &mut params);
        for field in ["solution_uid", "formation_paths"] {
            assert!(p.contains(&format!("paradedb.term('{field}'")), "{p}");
        }
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn la_projection_lit_l_arbre_et_replie_les_jours() {
        let json = serde_json::json!({
            "seat": {"buckets": [
                {"key": "CC", "doc_count": 30},
                {"key": "CC.SOCIALE", "doc_count": 18}]},
            "sol": {"buckets": [{"key": "solution:REJET", "doc_count": 25}]},
            "form": {"buckets": [{"key": "FR.TJ.JEX", "doc_count": 4}]},
            "pub": {"buckets": [{"key": "b", "doc_count": 7}, {"key": "n", "doc_count": 3}]},
            "jour": {"buckets": [
                {"key_as_string": "2024-03-01T00:00:00Z", "doc_count": 5},
                {"key_as_string": "2024-11-20T00:00:00Z", "doc_count": 6},
                {"key_as_string": "2025-01-09T00:00:00Z", "doc_count": 2}]},
            "total": {"value": 30.0},
        });
        let mut c = FacetCounters::default();
        let total = accumulate_counters(&json, &mut c, 1);
        assert_eq!(total, 30);
        assert_eq!(c.seats["CC"], 30);
        assert_eq!(c.seats["CC.SOCIALE"], 18);
        assert_eq!(c.solution["solution:REJET"], 25);
        assert_eq!(c.formation["FR.TJ.JEX"], 4);
        assert_eq!(c.publication["publication:PUBLIE_BULLETIN"], 7);
        assert_eq!(c.publication["publication:INEDIT_BULLETIN"], 3);
        assert_eq!(c.year["2024"], 11);
        assert_eq!(c.year["2025"], 2);
    }

    #[test]
    fn une_reponse_vide_ne_panique_pas() {
        let mut c = FacetCounters::default();
        let total = accumulate_counters(&Value::Null, &mut c, 1);
        assert_eq!(total, 0);
        assert!(c.seats.is_empty() && c.year.is_empty());
    }

    /// Le composite range l'article sous son texte par son préfixe. Celui d'un
    /// texte hors du top des instruments est jeté : la facette ne l'afficherait
    /// sous rien, les deux axes étant plats et tronqués à des rangs différents.
    #[test]
    fn l_article_se_range_sous_son_instrument_ou_disparait() {
        let mut c = FacetCounters::default();
        accumulate_counters(
            &serde_json::json!({
                "inst": {"buckets": [{"key": "LEGITEXT000006070716", "doc_count": 12}]},
                "art": {"buckets": [
                    {"key": "LEGITEXT000006070716|700", "doc_count": 9},
                    {"key": "LEGITEXT000006070716|145", "doc_count": 3},
                    {"key": "LEGITEXT000006072050|l1234-1", "doc_count": 7},
                    {"key": "sans-separateur", "doc_count": 5}
                ]}
            }),
            &mut c,
            1,
        );
        assert_eq!(c.instrument.counts["LEGITEXT000006070716"], 12);
        let articles = &c.instrument.articles["LEGITEXT000006070716"];
        assert_eq!(articles["700"], 9);
        assert_eq!(articles["145"], 3);
        assert_eq!(c.instrument.articles.len(), 1);
    }
}
