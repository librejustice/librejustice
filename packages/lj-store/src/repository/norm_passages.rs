//! `legal_article_passage`, the passages of the article versions the norms search reads.

use pgvector::Vector;

use super::DecisionRepository;
use crate::error::Result;

/// What the passages of one article version are cut from.
pub struct NormPassageInput {
    pub article_id: i64,
    pub content_checksum: i64,
    pub title: String,
    pub title_path: String,
    pub num: String,
    pub texte: String,
}

/// The vectors of one article version's passages in order, with the checksum
/// of the version they were cut from.
pub struct NormPassageWrite {
    pub article_id: i64,
    pub content_checksum: i64,
    pub embeddings: Vec<Vec<f32>>,
}

impl DecisionRepository<'_> {
    /// The norms searched with a text whose passages are missing or were cut
    /// from another checksum, newest first.
    #[tracing::instrument(name = "db.norm_embed_queue", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_embed_queue(&self) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                "SELECT a.id FROM legal_article a
                 WHERE a.kind = 'article' AND a.searchable AND a.texte <> ''
                   AND NOT EXISTS (SELECT 1 FROM legal_article_passage p
                                   WHERE p.article_id = a.id AND p.passage_index = 0
                                     AND p.content_checksum = a.content_checksum)
                 ORDER BY a.id DESC",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// The inputs of `ids` still in the table.
    #[tracing::instrument(name = "db.norm_embed_inputs", skip_all, fields(db.system = "postgresql", ids = ids.len()))]
    pub async fn norm_embed_inputs(&self, ids: &[i64]) -> Result<Vec<NormPassageInput>> {
        let rows = self
            .conn
            .query(
                "SELECT a.id, a.content_checksum, coalesce(a.code_title, ''),
                        coalesce(a.title_path, ''), a.num, a.texte
                 FROM legal_article a
                 WHERE a.id = ANY($1) AND a.texte <> ''",
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| NormPassageInput {
                article_id: r.get(0),
                content_checksum: r.get(1),
                title: r.get(2),
                title_path: r.get(3),
                num: r.get(4),
                texte: r.get(5),
            })
            .collect())
    }

    /// Replaces the passages of each version written, its vectors quantised to
    /// `rabitq4(512)`. Returns the passages written.
    #[tracing::instrument(name = "db.replace_norm_passages", skip_all, fields(db.system = "postgresql", articles = writes.len()))]
    pub async fn replace_norm_passages(&self, writes: &[NormPassageWrite]) -> Result<u64> {
        let ids: Vec<i64> = writes.iter().map(|w| w.article_id).collect();
        let mut article_ids = Vec::new();
        let mut indexes = Vec::new();
        let mut checksums = Vec::new();
        let mut vectors = Vec::new();
        for w in writes {
            for (i, v) in w.embeddings.iter().enumerate() {
                article_ids.push(w.article_id);
                indexes.push(i as i16);
                checksums.push(w.content_checksum);
                vectors.push(Vector::from(v.clone()));
            }
        }
        self.conn
            .execute(
                "DELETE FROM legal_article_passage WHERE article_id = ANY($1)",
                &[&ids],
            )
            .await?;
        Ok(self
            .conn
            .execute(
                "INSERT INTO legal_article_passage
                   (article_id, passage_index, content_checksum, embedding)
                 SELECT p.article_id, p.passage_index, p.content_checksum,
                        quantize_to_rabitq4(p.embedding)::rabitq4(512)
                 FROM unnest($1::bigint[], $2::smallint[], $3::bigint[], $4::vector[])
                      AS p(article_id, passage_index, content_checksum, embedding)
                 WHERE EXISTS (SELECT 1 FROM legal_article a WHERE a.id = p.article_id)",
                &[&article_ids, &indexes, &checksums, &vectors],
            )
            .await?)
    }
}
