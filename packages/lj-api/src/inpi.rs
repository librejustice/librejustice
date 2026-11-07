//! A company's deeds and annual accounts, listed by the INPI's RNE API on demand,
//! each file read from the INPI once and served from the bucket after that.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use lj_bucket::{key, Bucket};
use lj_dtos::{EntityDocumentDto, EntityDocumentsResponse};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::config::Settings;
use crate::error::{ApiError, Result};
use crate::state::AppState;
use crate::support::PlafondJournalier;
use crate::typed_cache::TypedCache;

const RNE: &str = "https://registre-national-entreprises.inpi.fr/api";
/// Daily cap on INPI calls.
const CALLS_PER_DAY: u32 = 5_000;
const TIMEOUT: Duration = Duration::from_secs(30);
const LISTINGS: u64 = 5_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Acte,
    Bilan,
}

impl Kind {
    pub fn parse(slug: &str) -> Option<Self> {
        match slug {
            "acte" => Some(Kind::Acte),
            "bilan" => Some(Kind::Bilan),
            _ => None,
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Kind::Acte => "acte",
            Kind::Bilan => "bilan",
        }
    }

    fn collection(self) -> &'static str {
        match self {
            Kind::Acte => "actes",
            Kind::Bilan => "bilans",
        }
    }
}

pub struct Inpi {
    http: reqwest::Client,
    username: String,
    password: String,
    token: tokio::sync::Mutex<Option<Arc<str>>>,
    bucket: Bucket,
    listings: TypedCache<Arc<Filed>>,
    calls: PlafondJournalier,
}

impl Inpi {
    /// `None` without the INPI account or the bucket.
    pub fn from_settings(settings: &Settings) -> Option<Self> {
        let bucket = settings.bucket.as_ref()?;
        if settings.inpi_username.is_empty() || settings.inpi_password.is_empty() {
            return None;
        }
        Some(Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent("librejustice.fr")
                .build()
                .expect("client HTTP INPI"),
            username: settings.inpi_username.clone(),
            password: settings.inpi_password.clone(),
            token: tokio::sync::Mutex::new(None),
            bucket: Bucket::new(bucket).expect("bucket client from a parsed config"),
            listings: TypedCache::builder(LISTINGS)
                .time_to_live(Duration::from_secs(24 * 3600))
                .build(),
            calls: PlafondJournalier::new(CALLS_PER_DAY),
        })
    }

    async fn filed(&self, siren: &str) -> Result<Arc<Filed>> {
        if let Some(hit) = self.listings.get(siren).await {
            return Ok(hit);
        }
        let response = self.get(&format!("/companies/{siren}/attachments")).await?;
        let filed: Filed = match response.status() {
            StatusCode::NOT_FOUND => Filed::default(),
            status if status.is_success() => response
                .json()
                .await
                .map_err(|e| upstream("attachments payload", &e))?,
            status => return Err(refused("attachments", status)),
        };
        let filed = Arc::new(filed.public());
        self.listings
            .insert(siren.to_string(), Arc::clone(&filed))
            .await;
        Ok(filed)
    }

    async fn file(&self, siren: &str, kind: Kind, id: &str) -> Result<Bytes> {
        let object = key::inpi(siren, kind.slug(), id);
        if let Some(bytes) = self.bucket.get(&object).await.map_err(bucket)? {
            return Ok(bytes);
        }
        if !self.filed(siren).await?.lists(kind, id) {
            return Err(ApiError::NotFound);
        }
        let response = self
            .get(&format!("/{}/{id}/download", kind.collection()))
            .await?;
        if !response.status().is_success() {
            return Err(refused("download", response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| upstream("download body", &e))?;
        self.bucket
            .put(&object, bytes.clone())
            .await
            .map_err(bucket)?;
        Ok(bytes)
    }

    /// Logs in again once on a 401: the token lives an hour.
    async fn get(&self, path: &str) -> Result<reqwest::Response> {
        let token = self.token(None).await?;
        let response = self.send(path, &token).await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let token = self.token(Some(&token)).await?;
        self.send(path, &token).await
    }

    async fn send(&self, path: &str, token: &str) -> Result<reqwest::Response> {
        self.spend()?;
        self.http
            .get(format!("{RNE}{path}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| upstream(path, &e))
    }

    /// The held token, unless it is `stale`: then a new login.
    async fn token(&self, stale: Option<&str>) -> Result<Arc<str>> {
        let mut held = self.token.lock().await;
        if let Some(token) = held.as_ref().filter(|t| Some(&***t) != stale) {
            return Ok(Arc::clone(token));
        }
        self.spend()?;
        let response = self
            .http
            .post(format!("{RNE}/sso/login"))
            .json(&serde_json::json!({ "username": self.username, "password": self.password }))
            .send()
            .await
            .map_err(|e| upstream("login", &e))?;
        if !response.status().is_success() {
            return Err(refused("login", response.status()));
        }
        let login: Login = response
            .json()
            .await
            .map_err(|e| upstream("login payload", &e))?;
        let token: Arc<str> = login.token.into();
        *held = Some(Arc::clone(&token));
        Ok(token)
    }

    fn spend(&self) -> Result<()> {
        if self.calls.preleve(chrono::Utc::now().timestamp()) {
            return Ok(());
        }
        tracing::warn!(cap = CALLS_PER_DAY, "inpi: the daily calls are spent");
        Err(ApiError::Unavailable("documents INPI".to_string()))
    }
}

/// `GET /entity/siren/{siren}/documents`.
pub async fn documents(state: &AppState, ns: &str, id: &str) -> Result<EntityDocumentsResponse> {
    let (inpi, siren) = target(state, ns, id)?;
    Ok(inpi.filed(siren).await?.present(siren))
}

/// `GET /entity/siren/{siren}/documents/{kind}/{doc}`.
pub async fn file(state: &AppState, ns: &str, id: &str, kind: &str, doc: &str) -> Result<Bytes> {
    let (inpi, siren) = target(state, ns, id)?;
    let kind = Kind::parse(kind).ok_or(ApiError::NotFound)?;
    if doc.is_empty() || doc.len() > 64 || !doc.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(ApiError::NotFound);
    }
    inpi.file(siren, kind, doc).await
}

fn target<'a>(state: &'a AppState, ns: &str, id: &'a str) -> Result<(&'a Inpi, &'a str)> {
    let inpi = state
        .inpi
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("documents INPI".to_string()))?;
    if ns != "siren" || id.len() != 9 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::NotFound);
    }
    Ok((inpi, id))
}

fn upstream(what: &str, e: &reqwest::Error) -> ApiError {
    tracing::warn!(what, error = %e, "inpi upstream failed");
    ApiError::BadGateway("INPI".to_string())
}

fn refused(what: &str, status: StatusCode) -> ApiError {
    tracing::warn!(what, status = status.as_u16(), "inpi refused");
    ApiError::BadGateway("INPI".to_string())
}

fn bucket(e: lj_bucket::Error) -> ApiError {
    ApiError::Internal(format!("bucket: {e}"))
}

#[derive(Deserialize)]
struct Login {
    token: String,
}

/// `GET /api/companies/{siren}/attachments`.
#[derive(Default, Deserialize)]
struct Filed {
    #[serde(default)]
    actes: Vec<Acte>,
    #[serde(default)]
    bilans: Vec<Bilan>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Acte {
    id: String,
    date_depot: Option<String>,
    updated_at: String,
    confidentiality: String,
    deleted: bool,
    libelle: Option<String>,
    /// What an older deed records, in place of a `libelle`.
    #[serde(default)]
    type_rdd: Vec<TypeRdd>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TypeRdd {
    type_acte: Option<String>,
    decision: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bilan {
    id: String,
    date_depot: Option<String>,
    updated_at: String,
    confidentiality: String,
    deleted: bool,
    date_cloture: Option<String>,
    type_bilan: Option<String>,
    /// Set on a deed filed with the accounts, which has no closing date.
    libelle: Option<String>,
}

impl Filed {
    fn public(self) -> Self {
        let public = |confidentiality: &str, deleted: bool| confidentiality == "Public" && !deleted;
        Self {
            actes: self
                .actes
                .into_iter()
                .filter(|a| public(&a.confidentiality, a.deleted))
                .collect(),
            bilans: self
                .bilans
                .into_iter()
                .filter(|b| public(&b.confidentiality, b.deleted))
                .collect(),
        }
    }

    fn lists(&self, kind: Kind, id: &str) -> bool {
        match kind {
            Kind::Acte => self.actes.iter().any(|a| a.id == id),
            Kind::Bilan => self.bilans.iter().any(|b| b.id == id),
        }
    }

    fn present(&self, siren: &str) -> EntityDocumentsResponse {
        let url = |kind: Kind, id: &str| {
            format!(
                "/internal/entity/siren/{siren}/documents/{}/{id}",
                kind.slug()
            )
        };
        let comptes = newest_first(
            self.bilans
                .iter()
                .map(|b| {
                    let dto = EntityDocumentDto {
                        label: bilan_label(b),
                        filed: b.date_depot.clone(),
                        url: url(Kind::Bilan, &b.id),
                    };
                    (b.date_depot.as_deref().unwrap_or(&b.updated_at), dto)
                })
                .collect(),
        );
        let actes = newest_first(
            self.actes
                .iter()
                .map(|a| {
                    let dto = EntityDocumentDto {
                        label: acte_label(a),
                        filed: a.date_depot.clone(),
                        url: url(Kind::Acte, &a.id),
                    };
                    (a.date_depot.as_deref().unwrap_or(&a.updated_at), dto)
                })
                .collect(),
        );
        EntityDocumentsResponse { comptes, actes }
    }
}

/// By filing date, by `updatedAt` for a filing the INPI gives no date.
fn newest_first(mut rows: Vec<(&str, EntityDocumentDto)>) -> Vec<EntityDocumentDto> {
    rows.sort_by(|a, b| b.0.cmp(a.0));
    rows.into_iter().map(|(_, dto)| dto).collect()
}

fn acte_label(acte: &Acte) -> String {
    if let Some(libelle) = acte
        .libelle
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        return libelle.to_string();
    }
    let mut recorded: Vec<&str> = Vec::new();
    for rdd in &acte.type_rdd {
        let Some(what) = rdd.decision.as_deref().or(rdd.type_acte.as_deref()) else {
            continue;
        };
        let what = what.trim();
        if !what.is_empty() && !recorded.contains(&what) {
            recorded.push(what);
        }
    }
    if recorded.is_empty() {
        "Acte déposé".to_string()
    } else {
        recorded.join(", ")
    }
}

fn bilan_label(bilan: &Bilan) -> String {
    let Some(year) = bilan.date_cloture.as_deref().and_then(|d| d.get(..4)) else {
        return bilan
            .libelle
            .clone()
            .unwrap_or_else(|| "Comptes annuels".to_string());
    };
    match bilan.type_bilan.as_deref() {
        Some("K") => format!("Comptes consolidés {year}"),
        _ => format!("Comptes annuels {year}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_public_filings_newest_first_with_a_label_each() {
        let filed: Filed = serde_json::from_value(serde_json::json!({
            "actes": [
                {"id": "a1", "dateDepot": "2022-12-23", "updatedAt": "2023-10-27T12:00:00+02:00", "confidentiality": "Public", "deleted": false,
                 "nomDocument": "x", "typeRdd": [
                    {"typeActe": "Acte", "decision": "Augmentation du capital social "},
                    {"typeActe": "Statuts mis à jour"},
                    {"typeActe": "Acte", "decision": "Augmentation du capital social"}]},
                {"id": "a2", "dateDepot": "2026-07-31", "updatedAt": "2026-08-01T10:00:00+02:00", "confidentiality": "Public", "deleted": false,
                 "libelle": "Copie des statuts mis à jour", "typeDocument": "PJ_02"},
                {"id": "a3", "dateDepot": "2016-06-08", "updatedAt": "2023-10-27T12:00:00+02:00", "confidentiality": "Public", "deleted": false},
                {"id": "a4", "dateDepot": "2020-01-01", "updatedAt": "2023-10-27T12:00:00+02:00", "confidentiality": "Public", "deleted": true},
                {"id": "a5", "updatedAt": "2026-10-29T07:00:00+01:00", "confidentiality": "Public", "deleted": false, "libelle": "PV"}
            ],
            "bilans": [
                {"id": "b1", "dateDepot": "2026-07-29", "updatedAt": "2026-09-17T10:00:00+02:00", "confidentiality": "Public", "deleted": false,
                 "dateCloture": "2025-12-31", "typeBilan": "K"},
                {"id": "b2", "dateDepot": "2026-07-24", "updatedAt": "2026-09-17T10:00:00+02:00", "confidentiality": "Public", "deleted": false,
                 "dateCloture": "2025-12-31", "typeBilan": "C"},
                {"id": "b3", "dateDepot": "2025-06-01", "updatedAt": "2025-06-02T10:00:00+02:00", "confidentiality": "Confidentiel", "deleted": false,
                 "dateCloture": "2024-12-31", "typeBilan": "S"}
            ],
            "bilansSaisis": [{"id": "s1"}]
        }))
        .unwrap();
        let filed = filed.public();
        assert!(filed.lists(Kind::Acte, "a1"));
        assert!(!filed.lists(Kind::Acte, "a4"));
        assert!(!filed.lists(Kind::Bilan, "b3"));
        assert!(!filed.lists(Kind::Bilan, "a1"));

        let response = filed.present("552081317");
        let labels: Vec<&str> = response.actes.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "PV",
                "Copie des statuts mis à jour",
                "Augmentation du capital social, Statuts mis à jour",
                "Acte déposé"
            ]
        );
        assert_eq!(
            response.actes[1].url,
            "/internal/entity/siren/552081317/documents/acte/a2"
        );
        let comptes: Vec<&str> = response.comptes.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(comptes, ["Comptes consolidés 2025", "Comptes annuels 2025"]);
    }

    #[test]
    fn a_company_the_inpi_has_nothing_for_reads_as_empty() {
        let filed: Filed = serde_json::from_str("{}").unwrap();
        assert!(filed.actes.is_empty() && filed.bilans.is_empty());
    }
}
