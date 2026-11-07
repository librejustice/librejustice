//! Candidats de surnom (`alias_candidate`) : écriture par gisement, relecture
//! par exclusivité. L'activation n'est pas ici : elle est un commit dans le
//! dataset versionné que la recherche lit.

use chrono::NaiveDate;

use super::DecisionRepository;
use crate::error::Result;

/// Un candidat prêt à écrire. `target_date` seule suffit quand la source ne donne
/// pas de numéro.
#[derive(Debug, Clone)]
pub struct AliasCandidat {
    pub alias_key: String,
    pub alias_vu: String,
    pub target_ref: String,
    pub target_date: Option<NaiveDate>,
    pub support: i32,
}

/// Un candidat majoritaire, avec le support du second pour le même surnom.
#[derive(Debug, Clone)]
pub struct AliasRevue {
    pub alias_key: String,
    pub alias_vu: String,
    pub target_ref: String,
    pub target_date: Option<NaiveDate>,
    pub support: i64,
    pub support_second: i64,
    pub decision_id: Option<i64>,
}

impl DecisionRepository<'_> {
    /// Renvois officiels d'un lot de bundles ArianeWeb, par décision commentée,
    /// paginé sur `decision_id` croissant. L'ordre du bundle est conservé : c'est
    /// lui qui donne au renvoi son rang, et donc son identité.
    pub async fn ariane_renvois_batch(
        &self,
        apres: i64,
        taille: i64,
    ) -> Result<Vec<(i64, Vec<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT ds.decision_id, array_agg(ri.r #>> '{}' ORDER BY ci.n, ri.n)
                 FROM decision_sources ds,
                      LATERAL jsonb_array_elements(ds.source_fields->'commentaires')
                              WITH ORDINALITY AS ci(c, n),
                      LATERAL jsonb_array_elements(ci.c->'meta'->'renvois')
                              WITH ORDINALITY AS ri(r, n)
                 WHERE ds.source = 'ariane-web'
                   AND ds.deleted_at IS NULL
                   AND jsonb_typeof(ci.c->'meta'->'renvois') = 'array'
                   AND ds.decision_id > $1
                 GROUP BY ds.decision_id
                 ORDER BY ds.decision_id
                 LIMIT $2",
                &[&apres, &taille],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Corps des décisions qui emploient la locution du surnom de loi, paginé sur
    /// `id` croissant.
    pub async fn dite_loi_batch(&self, apres: i64, taille: i64) -> Result<Vec<(i64, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT d.decision_id, d.full_text
                 FROM decision_text d
                 WHERE d.id @@@ paradedb.parse_with_field('full_text', $1)
                   AND d.deleted_at IS NULL
                   AND d.lang = 'fr'
                   AND d.decision_id > $2
                 ORDER BY d.decision_id
                 LIMIT $3",
                &[&"\"dite loi\" OR \"loi dite\"", &apres, &taille],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Titles of the catalogue's laws. The number and the date are read there:
    /// `date_texte` is missing on many laws, and the title carries them all.
    pub async fn titres_de_lois(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT t.title
                 FROM legal_text t
                 WHERE upper(t.nature)
                       IN ('LOI', 'LOI_ORGANIQUE', 'LOI_CONSTIT', 'LOI_PROGRAMME')",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Vide un gisement avant de le rejouer.
    pub async fn alias_candidates_reset(&self, genre: &str, gisement: &str) -> Result<u64> {
        let n = self
            .conn
            .execute(
                "DELETE FROM alias_candidate WHERE genre = $1 AND gisement = $2",
                &[&genre, &gisement],
            )
            .await?;
        Ok(n)
    }

    /// Accumule un lot : un même couple vu par deux sources du même gisement pèse
    /// deux voix.
    pub async fn alias_candidates_bulk(
        &self,
        genre: &str,
        gisement: &str,
        lot: &[AliasCandidat],
    ) -> Result<u64> {
        if lot.is_empty() {
            return Ok(0);
        }
        let keys: Vec<&str> = lot.iter().map(|c| c.alias_key.as_str()).collect();
        let vus: Vec<&str> = lot.iter().map(|c| c.alias_vu.as_str()).collect();
        let refs: Vec<&str> = lot.iter().map(|c| c.target_ref.as_str()).collect();
        let dates: Vec<Option<NaiveDate>> = lot.iter().map(|c| c.target_date).collect();
        let supports: Vec<i32> = lot.iter().map(|c| c.support).collect();
        let n = self
            .conn
            .execute(
                "INSERT INTO alias_candidate
                     (genre, gisement, alias_key, alias_vu, target_ref, target_date, support)
                 SELECT $1, $2, k, v, r, d, s
                 FROM unnest($3::text[], $4::text[], $5::text[], $6::date[], $7::int[])
                      AS t(k, v, r, d, s)
                 ON CONFLICT (genre, gisement, alias_key, target_ref, target_date)
                 DO UPDATE SET support = alias_candidate.support + excluded.support,
                               calcule_le = now()",
                &[&genre, &gisement, &keys, &vus, &refs, &dates, &supports],
            )
            .await?;
        Ok(n)
    }

    /// Candidats majoritaires d'un genre, gisements sommés. `support_second` est
    /// le concurrent immédiat du même surnom : c'est lui qui tranche, pas le
    /// volume. `decision_id` à `None` dit que la cible n'est pas au fonds.
    ///
    /// The date enters the index condition with the jurisdiction: made optional
    /// by an `OR`, it falls back to a filter and the resolver sweeps every
    /// Conseil d'État decision per candidate.
    pub async fn alias_candidates_revue(
        &self,
        genre: &str,
        limite: i64,
    ) -> Result<Vec<AliasRevue>> {
        let rows = self
            .conn
            .query(
                "WITH somme AS (
                     SELECT alias_key,
                            mode() WITHIN GROUP (ORDER BY alias_vu) AS alias_vu,
                            target_ref,
                            target_date,
                            sum(support)::bigint AS support
                     FROM alias_candidate
                     WHERE genre = $1
                     GROUP BY alias_key, target_ref, target_date
                 ),
                 classe AS (
                     SELECT *,
                            row_number() OVER (PARTITION BY alias_key ORDER BY support DESC) AS rang,
                            lead(support) OVER (PARTITION BY alias_key ORDER BY support DESC) AS suivant
                     FROM somme
                 ),
                 tete AS (
                     SELECT * FROM classe
                     WHERE rang = 1
                     ORDER BY support DESC, alias_key
                     LIMIT $2
                 )
                 SELECT c.alias_key, c.alias_vu, c.target_ref, c.target_date, c.support,
                        coalesce(c.suivant, 0) AS suivant, d.id
                 FROM tete c
                 LEFT JOIN LATERAL (
                     SELECT d.id
                     FROM decisions d
                     WHERE d.jurisdiction_code = split_part(c.target_ref, '|', 1)
                       AND d.deleted_at IS NULL
                       AND d.date_lecture = c.target_date
                       AND lj_docket_keys(d.docket_numbers)
                           && lj_docket_keys(string_to_array(split_part(c.target_ref, '|', 2), ','))
                     LIMIT 1
                 ) d ON split_part(c.target_ref, '|', 2) <> ''
                        AND c.target_date IS NOT NULL
                 ORDER BY c.support DESC, c.alias_key",
                &[&genre, &limite],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AliasRevue {
                alias_key: r.get(0),
                alias_vu: r.get(1),
                target_ref: r.get(2),
                target_date: r.get(3),
                support: r.get(4),
                support_second: r.get(5),
                decision_id: r.get(6),
            })
            .collect())
    }
}
