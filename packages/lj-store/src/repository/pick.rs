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
