//! Renvois officiels (`decision_renvoi`) : l'analyse d'une décision cite une
//! autre décision. La résolution exige la date en plus du numéro, les numéros se
//! recyclant entre époques (66145 rend Cadot, 1889, et une décision de 1988).

use std::collections::HashSet;

use chrono::NaiveDate;
use lj_core::address::CaseAddress;

use super::cases::{resolve_case_address_sqls, ResolveScope};
use super::support::address_columns;
use super::DecisionRepository;
use crate::error::Result;

/// `renvoi_no` est le rang du renvoi dans le bundle : il regroupe les numéros
/// co-cités d'une même citation. `nom` est vide quand la source a caviardé la
/// désignation des parties.
#[derive(Debug, Clone)]
pub struct Renvoi {
    pub decision_id: i64,
    pub renvoi_no: i16,
    pub address: CaseAddress,
    pub nom: Option<String>,
}

/// Une voix pour un surnom : la décision citante, le nom qu'elle emploie, et la
/// cible telle que le fonds la porte.
#[derive(Debug, Clone)]
pub struct RenvoiNomme {
    pub citante: i64,
    pub nom: String,
    pub target_ref: String,
    pub target_date: NaiveDate,
}

/// A decision a case name designates, as the exact key leg queries it.
#[derive(Debug, Clone)]
pub struct NamedDecision {
    pub id: i64,
    pub docket_numbers: Vec<String>,
}

/// The longest case-name key a query carries that the corpus knows, and its bearers.
#[derive(Debug, Clone)]
pub struct CaseNameMatch {
    pub key: String,
    pub decisions: Vec<NamedDecision>,
}

/// Un nom d'usage à poser : la décision, sa forme affichable, et toutes les
/// graphies repliées sous lesquelles la requête peut le demander.
#[derive(Debug, Clone)]
pub struct ElectedCaseName {
    pub decision_id: i64,
    /// `None` quand le nom désigne plusieurs décisions : il n'est celui
    /// d'aucune, mais chacune doit répondre quand on le tape.
    pub name: Option<String>,
    pub keys: Vec<String>,
    /// La valeur de `case_name_source` que porte ce nom, dont
    /// `lj_case_name_rank` décide s'il l'emporte sur l'écrivain en place.
    pub source: &'static str,
}

/// De quoi recomposer le titre hors du chemin d'extraction : les axes
/// structurés que `lj_core::titles` attend, le nom d'usage, et le titre stocké
/// pour n'écrire que ce qui bouge.
#[derive(Debug, Clone)]
pub struct TitleInputs {
    pub decision_id: i64,
    pub jurisdiction_label: Option<String>,
    pub jurisdiction_type: String,
    pub date_lecture: Option<String>,
    pub docket_numbers: Vec<String>,
    pub case_name: Option<String>,
    pub search_title: Option<String>,
}

impl DecisionRepository<'_> {
    /// Remplace les renvois d'un lot de décisions commentées : le bundle est la
    /// source, la table sa projection.
    pub async fn replace_renvois(&self, decisions: &[i64], lot: &[Renvoi]) -> Result<u64> {
        self.conn
            .execute(
                "DELETE FROM decision_renvoi WHERE decision_id = ANY($1)",
                &[&decisions],
            )
            .await?;
        if lot.is_empty() {
            return Ok(0);
        }
        // One renvoi quoted twice in a bundle is a single renvoi: the table
        // no longer carries the constraint, the batch does.
        let mut vu = HashSet::new();
        let mut ids: Vec<i64> = Vec::new();
        let mut nos: Vec<i16> = Vec::new();
        let mut courts: Vec<Option<&str>> = Vec::new();
        let mut dockets: Vec<Option<&str>> = Vec::new();
        let mut dates: Vec<Option<NaiveDate>> = Vec::new();
        let mut noms: Vec<Option<&str>> = Vec::new();
        for r in lot {
            let (court, docket, date) = address_columns(&r.address);
            if !vu.insert((r.decision_id, r.renvoi_no, court, docket, date)) {
                continue;
            }
            ids.push(r.decision_id);
            nos.push(r.renvoi_no);
            courts.push(court);
            dockets.push(docket);
            dates.push(date);
            noms.push(r.nom.as_deref());
        }
        let n = self
            .conn
            .execute(
                "INSERT INTO decision_renvoi
                     (decision_id, renvoi_no, target_court, target_docket, target_date, nom)
                 SELECT i, no, c, n, d, nom
                 FROM unnest($1::int8[], $2::int2[], $3::text[], $4::text[], $5::date[],
                             $6::text[])
                      AS t(i, no, c, n, d, nom)",
                &[&ids, &nos, &courts, &dockets, &dates, &noms],
            )
            .await?;
        Ok(n)
    }

    /// Sets `target_decision_id` on the pending renvois. Same resolver as the
    /// citations: the date is no longer required on its own, the uniqueness of
    /// the holder saying by itself that a docket recycled between two eras
    /// designates nobody.
    pub async fn resolve_pending_renvois(&self) -> Result<u64> {
        let mut n = 0u64;
        for sql in resolve_case_address_sqls("decision_renvoi", ResolveScope::All) {
            n += self.conn.execute(sql.as_str(), &[]).await?;
        }
        Ok(n)
    }

    /// Renvois nommés dont la cible est au fonds, dédupliqués : les numéros
    /// co-cités d'une affaire jointe tombent sur la même décision, donc sur une
    /// seule voix.
    pub async fn renvois_nommes(&self) -> Result<Vec<RenvoiNomme>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT r.decision_id, r.nom,
                        d.jurisdiction_code || '|' || array_to_string(d.docket_numbers, ','),
                        d.date_lecture
                 FROM decision_renvoi r
                 JOIN decisions d ON d.id = r.target_decision_id AND d.deleted_at IS NULL
                 WHERE r.nom IS NOT NULL
                   AND d.date_lecture IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| RenvoiNomme {
                citante: r.get(0),
                nom: r.get(1),
                target_ref: r.get(2),
                target_date: r.get(3),
            })
            .collect())
    }

    /// Le nom que les renvois donnent à chaque décision citée : celui que le
    /// plus de citantes emploient, la forme la plus longue départageant
    /// (« Société Tropic Travaux Signalisation » nomme mieux que « Tropic »).
    pub async fn elected_case_names(&self) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT ON (r.target_decision_id)
                        r.target_decision_id, r.nom
                 FROM decision_renvoi r
                 JOIN decisions d ON d.id = r.target_decision_id AND d.deleted_at IS NULL
                 WHERE r.target_decision_id IS NOT NULL AND r.nom IS NOT NULL
                 GROUP BY r.target_decision_id, r.nom
                 ORDER BY r.target_decision_id,
                          count(DISTINCT r.decision_id) DESC,
                          length(r.nom) DESC,
                          r.nom",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Les décisions dont la passe possède le titre : celles qui portent un nom
    /// d'usage, et toute cible de renvoi, qui en portait peut-être un hier. La
    /// passe les relit toutes et n'écrit que ce qui bouge, ce qui rattrape des
    /// deux côtés : une ré-extraction réécrit `search_title` sans connaître le
    /// nom, un nom écarté le laisse dans le titre.
    pub async fn case_name_title_inputs(&self) -> Result<Vec<TitleInputs>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, j.label, d.jurisdiction_type, d.date_lecture::text,
                        d.docket_numbers, d.case_name, d.search_title
                 FROM decisions d
                 LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
                 WHERE (d.case_name IS NOT NULL
                        OR EXISTS (SELECT 1 FROM decision_renvoi r
                                   WHERE r.target_decision_id = d.id))
                   AND d.deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TitleInputs {
                decision_id: r.get(0),
                jurisdiction_label: r.get(1),
                jurisdiction_type: r.get(2),
                date_lecture: r.get(3),
                docket_numbers: r.get(4),
                case_name: r.get(5),
                search_title: r.get(6),
            })
            .collect())
    }

    /// Rewrites `search_title` where the case name moved it, in batches: the BM25
    /// index of `decisions` charges every rewritten row, and a first pass would
    /// run past the `statement_timeout`.
    pub async fn set_search_titles(&self, titles: &[(i64, String)]) -> Result<u64> {
        let mut n = 0;
        for lot in titles.chunks(2_000) {
            let ids: Vec<i64> = lot.iter().map(|(id, _)| *id).collect();
            let composed: Vec<&str> = lot.iter().map(|(_, t)| t.as_str()).collect();
            n += self
                .conn
                .execute(
                    "UPDATE decisions d SET search_title = t.titre
                     FROM unnest($1::int8[], $2::text[]) AS t(id, titre)
                     WHERE d.id = t.id",
                    &[&ids, &composed],
                )
                .await?;
        }
        Ok(n)
    }

    /// The longest of `keys` that names a decision, with every bearer; `None`
    /// when no key designates anything.
    pub async fn case_name_match(&self, keys: &[String]) -> Result<Option<CaseNameMatch>> {
        let rows = self
            .conn
            .query(
                "SELECT k.cle, d.id, d.docket_numbers
                 FROM decisions d, unnest(d.case_name_keys) AS k(cle)
                 WHERE d.case_name_keys && $1 AND k.cle = ANY($1)
                   AND d.deleted_at IS NULL",
                &[&keys],
            )
            .await?;
        let Some(key) = rows
            .iter()
            .map(|r| r.get::<_, String>(0))
            .max_by_key(String::len)
        else {
            return Ok(None);
        };
        let decisions = rows
            .iter()
            .filter(|r| r.get::<_, String>(0) == key)
            .map(|r| NamedDecision {
                id: r.get(1),
                docket_numbers: r.get(2),
            })
            .collect();
        Ok(Some(CaseNameMatch { key, decisions }))
    }
}
