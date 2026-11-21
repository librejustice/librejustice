//! What the `bodacc` and `joafe` fonds share: the query, and the department axis.

use lj_dtos::{FacetChoice, JournalSearchRequest};
use lj_gazette::Source;
use lj_norm::referential_labels::departement_label;
use lj_store::repository::{DecisionRepository, GazetteQuery};

use crate::error::Result;

/// A SIREN, or a SIRET's, asks for the notices naming it, an RNA for the association's.
pub(crate) fn query(req: &JournalSearchRequest, source: Source) -> GazetteQuery {
    let Some(q) = req.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) else {
        return GazetteQuery::All;
    };
    let compact: String = q.chars().filter(|c| !c.is_whitespace()).collect();
    let digits = compact.bytes().all(|b| b.is_ascii_digit());
    match source {
        Source::Bodacc if digits && matches!(compact.len(), 9 | 14) => {
            GazetteQuery::Siren(compact[..9].parse().expect("nine digits"))
        }
        Source::Joafe if is_rna(&compact) => GazetteQuery::Rna(compact.to_ascii_uppercase()),
        _ => GazetteQuery::Text(q.to_string()),
    }
}

/// `W`, the prefecture's three characters, six digits.
fn is_rna(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[0].eq_ignore_ascii_case(&b'W')
        && b[1..4].iter().all(u8::is_ascii_alphanumeric)
        && b[4..].iter().all(u8::is_ascii_digit)
}

/// Each department dictionary code of a source, under its official code.
pub(crate) struct Depts(Vec<(i16, String)>);

impl Depts {
    pub(crate) async fn of(repo: &DecisionRepository<'_>, source: Source) -> Result<Depts> {
        Ok(Depts(
            repo.gazette_depts(source)
                .await?
                .into_iter()
                .map(|(code, value)| (code, lj_gazette::canonical_dept(&value).into_owned()))
                .collect(),
        ))
    }

    pub(crate) fn codes(&self, zones: &[String]) -> Option<Vec<i16>> {
        (!zones.is_empty()).then(|| {
            self.0
                .iter()
                .filter(|(_, value)| zones.contains(value))
                .map(|(code, _)| *code)
                .collect()
        })
    }

    /// The department axis, the codes of one department summed.
    pub(crate) fn choices(&self, counts: Vec<(i16, i64)>) -> Vec<FacetChoice> {
        let mut choices: Vec<FacetChoice> = Vec::new();
        for (code, count) in counts {
            let Some((_, value)) = self.0.iter().find(|(c, _)| *c == code) else {
                continue;
            };
            match choices.iter_mut().find(|c| &c.value == value) {
                Some(choice) => choice.count += count,
                None => choices.push(FacetChoice {
                    label: departement_label(value).unwrap_or(value).to_string(),
                    value: value.clone(),
                    count,
                    parent: None,
                    hint: None,
                }),
            }
        }
        by_count(choices)
    }
}

pub(crate) fn by_count(mut choices: Vec<FacetChoice>) -> Vec<FacetChoice> {
    choices.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str, source: Source) -> GazetteQuery {
        query(
            &JournalSearchRequest {
                q: Some(text.into()),
                ..Default::default()
            },
            source,
        )
    }

    #[test]
    fn an_identifier_in_the_query_asks_for_its_notices() {
        assert!(matches!(
            q("552 100 554", Source::Bodacc),
            GazetteQuery::Siren(552_100_554)
        ));
        assert!(matches!(
            q("55210055400013", Source::Bodacc),
            GazetteQuery::Siren(552_100_554)
        ));
        assert!(
            matches!(q("w751208507", Source::Joafe), GazetteQuery::Rna(r) if r == "W751208507")
        );
        assert!(matches!(
            q("W2A1000123", Source::Joafe),
            GazetteQuery::Rna(_)
        ));
        assert!(matches!(
            q("Wonderland", Source::Joafe),
            GazetteQuery::Text(_)
        ));
        assert!(matches!(
            q("552100554", Source::Joafe),
            GazetteQuery::Text(_)
        ));
        assert!(matches!(q("  ", Source::Bodacc), GazetteQuery::All));
    }
}
