//! Client API Légifrance (PISTE / DILA, moteur `lf-engine-app`).
//!
//! Client HTTP fin pour le seul besoin du banc d'unicité d'articles : un
//! `POST /search` sur le champ `NUM_ARTICLE` (fond `CODE_DATE`) qui renvoie les
//! codes **en vigueur** contenant un numéro d'article donné. L'authentification
//! est l'OAuth2 `client_credentials` de PISTE (mêmes identifiants que Judilibre,
//! `LIBREJUSTICE_PISTE_CLIENT_ID`/`_SECRET`) — le jeton Bearer est mis en cache
//! et rafraîchi à expiration. Aucune logique métier : la résolution
//! article → code (unicité nationale) vit dans l'appelant (`lj-ingest`).

use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde_json::{json, Value};

use crate::error::{Result, SourceError};
use crate::piste::PisteOAuth;

/// Base de l'API Légifrance (moteur DILA, production).
pub const BASE_URL: &str = "https://api.piste.gouv.fr/dila/legifrance/lf-engine-app";
/// User-Agent envoyé sur chaque requête.
pub const USER_AGENT: &str = "librejustice-legifrance/0.1 (+https://github.com/)";

/// Date de version (epoch ms) à laquelle on évalue « en vigueur ». Fixe et
/// arbitrairement « maintenant + marge » : on veut l'état courant du droit, et
/// `singleDate` ne sert qu'à écarter les versions futures/abrogées d'un article.
const VERSION_DATE_MS: i64 = 1_750_000_000_000;

/// Client Légifrance : un fournisseur de jeton PISTE partagé + le client HTTP
/// des appels `/search`.
pub struct LegifranceClient {
    base_url: String,
    client: ClientWithMiddleware,
    oauth: PisteOAuth,
}

impl LegifranceClient {
    /// Construit un client de production (base [`BASE_URL`], OAuth PISTE partagé).
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("reqwest client build"),
        )
        .with(TracingMiddleware::default())
        .build();
        Self {
            base_url: BASE_URL.to_string(),
            client,
            oauth: PisteOAuth::new(client_id, client_secret),
        }
    }

    /// Codes **en vigueur** contenant le numéro d'article `num` (forme Légifrance,
    /// ex. `"L5423-24"`, `"R425-12"`), dédupliqués par titre de code.
    ///
    /// Recherche `NUM_ARTICLE EXACTE` sur le fond `CODE_DATE` sans filtre de code :
    /// l'ensemble des titres renvoyés ayant au moins un extrait `VIGUEUR` est le
    /// support national de ce numéro. Un singleton ⇒ article nationalement
    /// discriminant.
    pub async fn article_in_force_codes(&self, num: &str) -> Result<Vec<String>> {
        let body = json!({
            "recherche": {
                "champs": [{
                    "typeChamp": "NUM_ARTICLE",
                    "criteres": [{ "typeRecherche": "EXACTE", "valeur": num, "operateur": "ET" }],
                    "operateur": "ET"
                }],
                "filtres": [{ "facette": "DATE_VERSION", "singleDate": VERSION_DATE_MS }],
                "pageNumber": 1,
                "pageSize": 50,
                "operateur": "ET",
                "sort": "PERTINENCE",
                "typePagination": "DEFAUT"
            },
            "fond": "CODE_DATE"
        });
        let v = self.search(&body).await?;

        let mut codes: Vec<String> = Vec::new();
        if let Some(results) = v["results"].as_array() {
            for res in results {
                let in_force = res["sections"].as_array().is_some_and(|secs| {
                    secs.iter().any(|sec| {
                        sec["extracts"].as_array().is_some_and(|exs| {
                            exs.iter()
                                .any(|ex| ex["legalStatus"].as_str() == Some("VIGUEUR"))
                        })
                    })
                });
                if !in_force {
                    continue;
                }
                if let Some(title) = res["titles"][0]["title"].as_str() {
                    if !codes.iter().any(|c| c == title) {
                        codes.push(title.to_string());
                    }
                }
            }
        }
        Ok(codes)
    }

    /// `POST /search` signé Bearer ; JSON ou `LegifranceApi` sur statut ≠ 200.
    async fn search(&self, body: &Value) -> Result<Value> {
        let url = format!("{}/search", self.base_url);
        let token = self.oauth.bearer().await?;
        let resp = self
            .client
            .post(&url)
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .body(serde_json::to_string(body).expect("serialize search body"))
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("requête legifrance {url}: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        if status != 200 {
            return Err(SourceError::LegifranceApi {
                status,
                url,
                body: text,
            });
        }
        Ok(sonic_rs::from_str(&text)?)
    }
}
