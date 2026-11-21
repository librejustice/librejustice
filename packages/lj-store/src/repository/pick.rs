//! Which of a decision's texts is read: French first, the interface's only language.

/// SQL sort keys over the texts `t` of the decision `d`, the text read first.
pub fn order(t: &str, d: &str) -> String {
    format!("{t}.lang <> 'fr', {t}.lang <> 'en', {t}.lang <> {d}.lang, {t}.lang")
}

/// Subquery: `columns` of `p`, the text read of the decision `d`, one row at most.
pub fn text(columns: &str, d: &str) -> String {
    format!(
        "(SELECT {columns} FROM decision_text p WHERE p.decision_id = {d}.id ORDER BY {} LIMIT 1)",
        order("p", d)
    )
}

/// Subquery: `columns` of `p`, the text read of the decision `d`, and of `r`, the summary
/// of `p` written for the reader of `lang`, a SQL text expression, when `p` is in
/// another language and one is held.
pub fn read_summary(columns: &str, d: &str, lang: &str) -> String {
    format!(
        "(SELECT {columns} FROM decision_text p \
         LEFT JOIN decision_text_reader_summary r \
           ON r.decision_text_id = p.id AND r.lang = {lang} AND p.lang <> {lang} \
         WHERE p.decision_id = {d}.id ORDER BY {} LIMIT 1)",
        order("p", d)
    )
}

/// The columns of [`read_summary`] a reader is served: the reader summary when held,
/// else the text's own, then where the court's words and the body sit in it. A reader
/// summary holds no words of the court, which are in the text's language.
pub const SUMMARY_COLUMNS: &str = "coalesce(r.summary, p.summary) AS summary, \
     CASE WHEN r.summary IS NULL THEN p.summary_quote_start END AS summary_quote_start, \
     CASE WHEN r.summary IS NULL THEN p.summary_quote_end END AS summary_quote_end, \
     CASE WHEN r.summary IS NULL THEN p.summary_body_start ELSE r.summary_body_start END \
       AS summary_body_start, \
     CASE WHEN r.summary IS NULL THEN p.summary_body_end ELSE r.summary_body_end END \
       AS summary_body_end, \
     coalesce(r.summary_engine, p.summary_engine) AS summary_engine, \
     CASE WHEN r.summary IS NULL THEN p.lang ELSE r.lang END AS summary_lang";

/// Subquery: the summary of the decision `d` its reader of `lang` is served.
pub fn summary(d: &str, lang: &str) -> String {
    read_summary("coalesce(r.summary, p.summary)", d, lang)
}

/// Subquery: `[quote_start, quote_end, body_start, body_end]` of [`summary`], each NULL
/// when not stored.
pub fn summary_bounds(d: &str, lang: &str) -> String {
    read_summary(
        "CASE WHEN r.summary IS NULL \
           THEN ARRAY[p.summary_quote_start, p.summary_quote_end, p.summary_body_start, p.summary_body_end] \
           ELSE ARRAY[NULL, NULL, r.summary_body_start, r.summary_body_end] END",
        d,
        lang,
    )
}

/// Where the court's words sit and where the body sits, as [`summary_bounds`] selected them.
pub fn bounds(
    raw: Option<Vec<Option<i32>>>,
) -> (
    Option<std::ops::Range<usize>>,
    Option<std::ops::Range<usize>>,
) {
    let Some(raw) = raw else {
        return (None, None);
    };
    let range = |start: Option<i32>, end: Option<i32>| Some(start? as usize..end? as usize);
    (range(raw[0], raw[1]), range(raw[2], raw[3]))
}

/// SQL predicate: `t` is the text read of its decision.
pub fn read(t: &str) -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM decision_text o JOIN decisions od ON od.id = o.decision_id \
         WHERE o.decision_id = {t}.decision_id AND ({}) < ({}))",
        order("o", "od"),
        order(t, "od")
    )
}

/// SQL predicate: `t` is one of the texts its decision's passages read besides the
/// read one, those that make the decision authentic, the French and the English.
pub fn passages(t: &str) -> String {
    format!("({t}.text_status = 'authentic' OR {t}.lang IN ('fr', 'en'))")
}

/// SQL predicate: `t` is cut into its decision's passages, the read text or one of
/// [`passages`] whose provenance is active, each behind its own summary.
pub fn embedded(t: &str) -> String {
    format!(
        "({} OR ({} AND EXISTS (SELECT 1 FROM decision_sources es \
         WHERE es.source_uid = {t}.source_uid AND es.deleted_at IS NULL)))",
        read(t),
        passages(t)
    )
}
