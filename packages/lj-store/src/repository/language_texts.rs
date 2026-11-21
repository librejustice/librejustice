//! `legal_article_text`, an article version's text in each language other than its `texte`'s.

use std::collections::HashMap;

use super::DecisionRepository;
use crate::error::{Result, StoreError};

/// One version's text in one language, as its source states it.
pub struct ArticleLanguageWrite {
    pub article_id: i64,
    pub text_status: String,
    pub texte: String,
    pub title_path: Option<String>,
    pub content_checksum: i64,
}

/// One version's text in the language asked.
pub struct ArticleLanguageText {
    pub text_status: String,
    pub texte: String,
    pub title_path: Option<String>,
}

impl DecisionRepository<'_> {
    /// Sets the texts of `text_uid` in `language` to `rows` in one statement, a version
    /// absent from `rows` losing its text in that language. Returns the rows written.
    #[tracing::instrument(name = "db.replace_language_texts", skip(self, rows), fields(db.system = "postgresql", rows = rows.len()))]
    pub async fn replace_language_texts(
        &self,
        text_uid: &str,
        language: &str,
        rows: &[ArticleLanguageWrite],
    ) -> Result<u64> {
        let ids: Vec<i64> = rows.iter().map(|r| r.article_id).collect();
        let owned: i64 = self
            .conn
            .query_one(
                "SELECT count(*) FROM legal_article
                 WHERE id = ANY($1) AND text_uid = $2 AND language <> $3",
                &[&ids, &text_uid, &language],
            )
            .await?
            .get(0);
        if owned as usize != ids.len() {
            return Err(StoreError::Invalid(format!(
                "{} of {} texts in {language} are not versions of {text_uid} in another language",
                ids.len() - owned as usize,
                ids.len()
            )));
        }
        let statuses: Vec<&str> = rows.iter().map(|r| r.text_status.as_str()).collect();
        let textes: Vec<&str> = rows.iter().map(|r| r.texte.as_str()).collect();
        let paths: Vec<Option<&str>> = rows.iter().map(|r| r.title_path.as_deref()).collect();
        let checksums: Vec<i64> = rows.iter().map(|r| r.content_checksum).collect();
        Ok(self
            .conn
            .execute(
                "WITH r AS (
                     SELECT * FROM unnest($3::bigint[], $4::text[], $5::text[], $6::text[], $7::bigint[])
                         AS r(article_id, text_status, texte, title_path, content_checksum)
                 ), gone AS (
                     DELETE FROM legal_article_text t USING legal_article a
                     WHERE a.id = t.article_id AND a.text_uid = $1 AND t.language = $2
                       AND t.article_id NOT IN (SELECT article_id FROM r)
                 )
                 INSERT INTO legal_article_text
                     (article_id, language, text_status, texte, title_path, content_checksum)
                 SELECT article_id, $2, text_status, texte, title_path, content_checksum FROM r
                 ON CONFLICT (article_id, language) DO UPDATE SET
                     text_status = EXCLUDED.text_status, texte = EXCLUDED.texte,
                     title_path = EXCLUDED.title_path, content_checksum = EXCLUDED.content_checksum
                 WHERE (legal_article_text.text_status, legal_article_text.texte,
                        legal_article_text.title_path, legal_article_text.content_checksum)
                       IS DISTINCT FROM (EXCLUDED.text_status, EXCLUDED.texte,
                                         EXCLUDED.title_path, EXCLUDED.content_checksum)",
                &[&text_uid, &language, &ids, &statuses, &textes, &paths, &checksums],
            )
            .await?)
    }

    /// The texts in `language` of the versions `article_ids` that hold one.
    #[tracing::instrument(name = "db.language_texts", skip(self, article_ids), fields(db.system = "postgresql", ids = article_ids.len()))]
    pub async fn language_texts(
        &self,
        article_ids: &[i64],
        language: &str,
    ) -> Result<HashMap<i64, ArticleLanguageText>> {
        let rows = self
            .conn
            .query(
                "SELECT article_id, text_status, texte, title_path FROM legal_article_text
                 WHERE article_id = ANY($1) AND language = $2",
                &[&article_ids, &language],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get(0),
                    ArticleLanguageText {
                        text_status: r.get(1),
                        texte: r.get(2),
                        title_path: r.get(3),
                    },
                )
            })
            .collect())
    }

    /// The languages the versions of `text_uid` hold their `texte` in.
    #[tracing::instrument(name = "db.texte_languages", skip(self), fields(db.system = "postgresql"))]
    pub async fn texte_languages(&self, text_uid: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT language FROM legal_article WHERE text_uid = $1",
                &[&text_uid],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Names `language` the language of every `texte` of `text_uid`. Returns the rows changed.
    #[tracing::instrument(name = "db.set_texte_language", skip(self), fields(db.system = "postgresql"))]
    pub async fn set_texte_language(&self, text_uid: &str, language: &str) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE legal_article SET language = $2 WHERE text_uid = $1 AND language <> $2",
                &[&text_uid, &language],
            )
            .await?)
    }
}
