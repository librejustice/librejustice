//! Downloader bulk DILA no-auth + réparation d'encodage au bord.
//!
//! La DILA diffuse les fonds JADE / CONSTIT en bulk XML sans
//! authentification sur `https://echanges.dila.gouv.fr/OPENDATA/<FOND>/` : un
//! **stock global** `Freemium_<fond>_global_*.tar.gz` (bootstrap auto au cold
//! start, plusieurs Go) et des **incréments** datés `<FOND>_YYYYMMDD-HHMMSS.tar.gz`
//! (créations / maj / suppressions), appliqués par ordre lexicographique =
//! chronologique. Le watermark (dernier incrément appliqué) est persisté dans un
//! manifeste JSON par fond, idempotent.
//!
//! La lecture des `.tar.gz` (XML + `.dat` de suppression) vit dans
//! [`crate::tar_reader`]. Le parsing métier vit dans `lj-core` ; ce module est le
//! bord I/O, et c'est là que vit [`repair_dila`] : la réparation d'encodage
//! (entités double-échappées + mojibake double-encodage des sous-arbres JADE) est
//! une frontière de validation I/O propre à la diffusion DILA.

use crate::downloader::{get_to_file_retrying, get_with_body_retrying};
use crate::error::{Result, SourceError};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use tracing::instrument;

pub(crate) const DILA_BASE_URL: &str = "https://echanges.dila.gouv.fr/OPENDATA";
const DILA_USER_AGENT: &str = "librejustice-dila-downloader/0.1 (+https://github.com/)";
use crate::state_paths::DILA_DIR;

/// Fonds DILA bulk no-auth. Le segment de listing est en majuscules
/// (`OPENDATA/JADE/`), le préfixe d'incrément aussi (`JADE_YYYYMMDD-HHMMSS`),
/// l'infixe du stock est en minuscules (`Freemium_jade_global_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DilaFond {
    Jade,
    Constit,
    /// Délibérations/décisions de la CNIL (fond `OPENDATA/CNIL/`).
    Cnil,
    /// Judgments of the Cour de cassation printed in the Bulletin.
    Cass,
    /// Unpublished judgments of the Cour de cassation.
    Inca,
    /// A selection of courts of appeal and first-degree courts.
    Capp,
    Legi,
    /// Journal officiel : décrets de publication des traités/accords (référentiel
    /// `source='treaty'`).
    Jorf,
    /// Conventions collectives nationales (KALICONT/KALIARTI).
    Kali,
}

impl DilaFond {
    /// Segment de listing / préfixe d'incrément, en majuscules.
    pub fn segment(self) -> &'static str {
        match self {
            DilaFond::Jade => "JADE",
            DilaFond::Constit => "CONSTIT",
            DilaFond::Cnil => "CNIL",
            DilaFond::Cass => "CASS",
            DilaFond::Inca => "INCA",
            DilaFond::Capp => "CAPP",
            DilaFond::Legi => "LEGI",
            DilaFond::Jorf => "JORF",
            DilaFond::Kali => "KALI",
        }
    }

    /// Infixe du stock global `Freemium_<infix>_global_*`, en minuscules.
    pub fn stock_infix(self) -> &'static str {
        match self {
            DilaFond::Jade => "jade",
            DilaFond::Constit => "constit",
            DilaFond::Cnil => "cnil",
            DilaFond::Cass => "cass",
            DilaFond::Inca => "inca",
            DilaFond::Capp => "capp",
            DilaFond::Legi => "legi",
            DilaFond::Jorf => "jorf",
            DilaFond::Kali => "kali",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "jade" => Ok(DilaFond::Jade),
            "constit" => Ok(DilaFond::Constit),
            "cnil" => Ok(DilaFond::Cnil),
            "cass" => Ok(DilaFond::Cass),
            "inca" => Ok(DilaFond::Inca),
            "capp" => Ok(DilaFond::Capp),
            "legi" => Ok(DilaFond::Legi),
            "jorf" => Ok(DilaFond::Jorf),
            "kali" => Ok(DilaFond::Kali),
            other => Err(SourceError::Invalid(format!("fond DILA inconnu: {other}"))),
        }
    }
}

/// Dossier local des `tar.gz` d'un fond DILA sous `data_dir`
/// (`<data_dir>/dila/<infix>/tarballs/`). Source unique du layout pour
/// [`sync_dila`] (qui y écrit les incréments téléchargés) et l'ingest (qui les
/// relit). Le stock global y est aussi déposé.
pub fn tarballs_dir(data_dir: &Path, fond: DilaFond) -> std::path::PathBuf {
    data_dir
        .join(DILA_DIR)
        .join(fond.stock_infix())
        .join("tarballs")
}

/// A fund's cached tarballs in replay order: by the timestamp their name
/// carries, a stock before the increment of the same instant.
pub fn cached_dila_tarballs(data_dir: &Path, fond: DilaFond) -> Result<Vec<std::path::PathBuf>> {
    let dir = tarballs_dir(data_dir, fond);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<std::path::PathBuf> = fs::read_dir(&dir)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".tar.gz"))
        .collect();
    paths.sort_by_cached_key(|p| replay_key(fond, p));
    Ok(paths)
}

fn replay_key(fond: DilaFond, path: &Path) -> (String, bool) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.starts_with("Freemium_") {
        true => (stock_watermark(fond, &name), false),
        false => (name, true),
    }
}

// ----------------------------------------------------------------------------
// Manifeste DILA (watermark par fond)
// ----------------------------------------------------------------------------

/// État de sync d'un fond DILA : watermark = nom du dernier incrément appliqué
/// (`<FOND>_YYYYMMDD-HHMMSS.tar.gz`). Tri lexical des noms = ordre chronologique.
/// Variante DILA du `Manifest` de `downloader.rs` (un fichier JSON par fond).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DilaManifest {
    #[serde(default)]
    pub stock_fetched: bool,
    #[serde(default)]
    pub watermark: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
}

impl DilaManifest {
    fn now_iso_seconds() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| SourceError::Invalid(format!("manifest DILA illisible: {e}")))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| SourceError::Invalid(format!("manifest DILA non sérialisable: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Listing du répertoire OPENDATA/<FOND>/
// ----------------------------------------------------------------------------

/// Tarball listé sur `OPENDATA/<FOND>/`, classé en stock ou incrément.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DilaTarball {
    /// Stock global `Freemium_<fond>_global_*.tar.gz` (bootstrap auto au cold start).
    Stock(String),
    /// Incrément daté `<FOND>_YYYYMMDD-HHMMSS.tar.gz`.
    Increment(String),
}

/// Extrait les noms de `.tar.gz` du listing HTML d'un répertoire Apache/nginx
/// (`href="…tar.gz"`) et les classe. Les noms hors-fond sont ignorés.
fn classify_listing(html: &str, fond: DilaFond) -> Vec<DilaTarball> {
    static HREF_RE: OnceLock<Regex> = OnceLock::new();
    let re = HREF_RE.get_or_init(|| {
        Regex::new(r#"href="(?P<name>[^"]+\.tar\.gz)""#).expect("regex href tar.gz valide")
    });
    let stock_prefix = format!("Freemium_{}_global_", fond.stock_infix());
    let incr_prefix = format!("{}_", fond.segment());
    let mut out = Vec::new();
    for cap in re.captures_iter(html) {
        // Le href peut être un chemin relatif ; on ne garde que le basename.
        let name = cap
            .name("name")
            .expect("groupe name présent")
            .as_str()
            .rsplit('/')
            .next()
            .expect("rsplit non vide")
            .to_string();
        if name.starts_with(&stock_prefix) {
            out.push(DilaTarball::Stock(name));
        } else if name.starts_with(&incr_prefix) {
            out.push(DilaTarball::Increment(name));
        }
    }
    out
}

pub(crate) fn http_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(DILA_USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        // Timeout TOTAL généreux : un stock global multi-Go streamé peut prendre
        // longtemps (pas de read_timeout par chunk sur le client blocking reqwest).
        .timeout(Duration::from_secs(3600))
        // PAS de réutilisation de connexion : une connexion poolée réutilisée finit
        // par se figer (spin CPU 100 %, fds multiples sur le même socket) sur le chemin
        // réseau vers echanges.dila.gouv.fr. Chaque requête repart donc sur une
        // connexion neuve, ce qui borne aussi le nombre de sockets ouverts.
        .pool_max_idle_per_host(0)
        .build()?)
}

/// Liste et classe les tarballs publiés pour un fond.
fn list_tarballs(client: &reqwest::blocking::Client, fond: DilaFond) -> Result<Vec<DilaTarball>> {
    let url = format!("{DILA_BASE_URL}/{}/", fond.segment());
    let (status, _, body) = with_stall_watchdog("GET listing", Duration::from_secs(60), || {
        get_with_body_retrying(&url, || client.get(&url).send())
    })?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "listing DILA {} statut {status}",
            fond.segment()
        )));
    }
    let bytes = body.expect("statut 200 → corps lu par get_with_body_retrying");
    let html = String::from_utf8_lossy(&bytes);
    Ok(classify_listing(&html, fond))
}

pub(crate) fn with_stall_watchdog<T>(what: &str, threshold: Duration, op: impl FnOnce() -> T) -> T {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let label = what.to_string();
    let watchdog = std::thread::spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(threshold) {
            tracing::error!(
                what = %label,
                threshold_s = threshold.as_secs(),
                "dila_network_stall : opération réseau bloquée au-delà du seuil \
                 (spin probable du client blocking reqwest sur le chemin hôte↔DILA) — \
                 ingest potentiellement gelé"
            );
            // Attendre la vraie fin sans re-spinner (n'alerte qu'une fois).
            let _ = rx.recv();
        }
    });
    let out = op();
    drop(tx);
    let _ = watchdog.join();
    out
}

/// Télécharge un tarball nommé sous `OPENDATA/<FOND>/` vers `dst` (tmp + rename).
///
/// Idempotent par PRÉSENCE : on skip si `dst` existe. Les tarballs DILA (stock
/// comme incréments) sont immuables, nommés par timestamp de publication. Pas de
/// HEAD `Content-Length` préalable : sur le chemin hôte↔DILA le HEAD du client
/// blocking reqwest se fige en spin CPU 100 % et bloque la chaîne à vie. Le
/// `.part` n'est renommé vers `dst` que sur GET complet (statut 200) ; une
/// troncature « propre » résiduelle échoue de toute façon à la décompression
/// `.tar.gz`.
fn download_tarball(
    client: &reqwest::blocking::Client,
    fond: DilaFond,
    name: &str,
    dst: &Path,
) -> Result<()> {
    let url = format!("{DILA_BASE_URL}/{}/{name}", fond.segment());
    if dst.exists() {
        tracing::info!(
            fond = fond.segment(),
            name,
            "dila_tarball already there, skip"
        );
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    // Streamé sur disque (RAM ~constante : un stock global pèse plusieurs Go).
    let tmp = dst.with_extension("part");
    // Seuil large (600 s) : un stock global multi-Go peut légitimement streamer
    // plusieurs minutes ; au-delà = spin.
    let (status, _, octets) =
        with_stall_watchdog(&format!("GET {name}"), Duration::from_secs(600), || {
            get_to_file_retrying(&url, &tmp, || client.get(&url).send())
        })?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "téléchargement DILA {name} statut {status}"
        )));
    }
    fs::rename(&tmp, dst)?;
    tracing::info!(fond = fond.segment(), name, octets, "dila_tarball");
    Ok(())
}

/// Sync incrémental d'un fond DILA : télécharge les incréments postérieurs au
/// watermark (ordre lexicographique = chronologique).
///
/// **Auto-switch cold ↔ warm** (un seul point d'entrée pour le cron) :
/// - **Cold start** (`stock_fetched == false`) : télécharge le **stock global**
///   `Freemium_<fond>_global_*.tar.gz` le plus récent EN PREMIER, puis cale le
///   watermark sur la date du stock ([`stock_watermark`]) pour n'appliquer que
///   les incréments postérieurs.
/// - **Warm** : applique les incréments `<FOND>_*` postérieurs au watermark.
///
/// Renvoie le lot à ingérer dans l'ordre d'application (stock d'abord au cold
/// start, puis incréments chronologiques). Idempotent : le stock n'est repris
/// qu'une fois (`stock_fetched`), un incrément ≤ watermark est ignoré.
///
/// **Le manifeste ne bouge pas ici** : le filigrane appartient à l'ingestion, et
/// s'avance archive par archive via [`DilaBatch::commit`]. Un téléchargement
/// perdu se refait pour rien du tout — le fichier reste sur disque et
/// `download_tarball` le saute.
#[instrument(skip(data_dir))]
pub fn sync_dila(data_dir: &Path, fond: DilaFond) -> Result<DilaBatch> {
    let tarballs_path = tarballs_dir(data_dir, fond);
    fs::create_dir_all(&tarballs_path)?;
    let source_dir = data_dir.join(DILA_DIR).join(fond.stock_infix());
    let manifest_path = source_dir.join("manifest.json");
    let manifest = DilaManifest::load(&manifest_path)?;

    let client = http_client()?;
    let tarballs = list_tarballs(&client, fond)?;

    let mut stocks: Vec<String> = Vec::new();
    let mut increments: Vec<String> = Vec::new();
    for t in &tarballs {
        match t {
            DilaTarball::Stock(name) => stocks.push(name.clone()),
            DilaTarball::Increment(name) => increments.push(name.clone()),
        }
    }
    stocks.sort();
    increments.sort();

    let mut archives = Vec::new();
    // Plancher du lot, figé pour tout le parcours : le filigrane en base de
    // départ, relevé à la date du stock quand le cold start en télécharge un.
    let mut floor = manifest.watermark.clone();

    // Cold start : bootstrap du stock global le plus récent AVANT les incréments.
    // Erreur franche si aucun stock publié — un fond ne peut pas démarrer par diff.
    if !manifest.stock_fetched {
        let stock = stocks.last().ok_or_else(|| {
            SourceError::Invalid(format!(
                "fond DILA {} : aucun stock global Freemium_{}_global_* publié, cold start impossible",
                fond.segment(),
                fond.stock_infix()
            ))
        })?;
        let dst = tarballs_path.join(stock);
        download_tarball(&client, fond, stock, &dst)?;
        let watermark = stock_watermark(fond, stock);
        floor = Some(watermark.clone());
        archives.push(DilaArchive {
            path: dst,
            watermark,
            is_stock: true,
        });
    }

    for name in increments {
        if let Some(wm) = &floor {
            if &name <= wm {
                continue;
            }
        }
        let dst = tarballs_path.join(&name);
        download_tarball(&client, fond, &name, &dst)?;
        archives.push(DilaArchive {
            path: dst,
            watermark: name,
            is_stock: false,
        });
    }

    Ok(DilaBatch {
        manifest_path,
        manifest,
        archives,
    })
}

/// Une archive DILA téléchargée, en attente d'ingestion.
#[derive(Clone, Debug)]
pub struct DilaArchive {
    pub path: std::path::PathBuf,
    /// Filigrane à poser une fois cette archive ingérée : le nom de l'incrément,
    /// ou la borne synthétique calée sur la date d'un stock global.
    pub watermark: String,
    /// Stock global de cold start plutôt qu'incrément.
    pub is_stock: bool,
}

/// Lot téléchargé et le manifeste qui suit son **ingestion**. Le filigrane
/// n'avance qu'au [`DilaBatch::commit`], appelé après chaque archive ingérée :
/// tuée ou en erreur au milieu du lot, la passe laisse le filigrane sur la
/// dernière archive réellement appliquée, et la suivante reprend là.
pub struct DilaBatch {
    manifest_path: std::path::PathBuf,
    manifest: DilaManifest,
    /// Archives à ingérer, dans l'ordre.
    pub archives: Vec<DilaArchive>,
}

impl DilaBatch {
    /// Pose le filigrane d'une archive ingérée. À appeler dans l'ordre du lot :
    /// le filigrane est un ordre lexicographique, il ne recule pas.
    pub fn commit(&mut self, archive: &DilaArchive) -> Result<()> {
        if archive.is_stock {
            self.manifest.stock_fetched = true;
        }
        self.manifest.watermark = Some(archive.watermark.clone());
        self.manifest.fetched_at = Some(DilaManifest::now_iso_seconds());
        self.manifest.save(&self.manifest_path)
    }
}

/// Watermark synthétique calé sur la date d'un stock global, pour ne reprendre
/// au warm que les incréments postérieurs. `Freemium_<infix>_global_<TS>.tar.gz`
/// → `<SEGMENT>_<TS>.tar.gz`, lexicographiquement comparable aux noms
/// d'incréments (`<FOND>_YYYYMMDD-HHMMSS.tar.gz`). Si le nom du stock est
/// inattendu (préfixe/suffixe absents), on renvoie un watermark vide
/// (`<SEGMENT>_.tar.gz`) — borne basse : tous les incréments s'appliqueront,
/// l'idempotence par checksum absorbe le recouvrement.
fn stock_watermark(fond: DilaFond, stock_name: &str) -> String {
    let ts = stock_name
        .strip_prefix(&format!("Freemium_{}_global_", fond.stock_infix()))
        .and_then(|s| s.strip_suffix(".tar.gz"))
        .unwrap_or("");
    format!("{}_{}.tar.gz", fond.segment(), ts)
}

// ----------------------------------------------------------------------------
// Réparation d'encodage (bord I/O)
// ----------------------------------------------------------------------------

/// Répare l'XML brut DILA avant parsing pur (`lj-core`).
///
/// Deux corrections, distinctes :
/// 1. **Mojibake double-encodage UTF-8** (`Ã©`→`é`, `â€™`→`'`), confiné aux
///    sous-arbres `SCT` / `ANA` / `CITATION_JP` de **JADE** uniquement (le
///    `CONTENU` est propre). On localise ces spans par regex et on applique
///    [`undo_mojibake`] **dans le span seulement**, jamais globalement.
/// 2. **Entités double-échappées** (`&amp;nbsp;`, tous fonds) : de-escape global
///    en `\u{a0}`.
///
/// Ordre : mojibake d'abord, de-escape ensuite. Le roundtrip latin1/cp1252
/// reconstitue des octets par caractère ≤ 0xFF ; un `\u{a0}` (0xA0) injecté avant
/// par la de-escape ferait échouer le décodage UTF-8 du span (0xA0 = octet de
/// continuation isolé). La séquence `&amp;nbsp;` (ASCII pur) traverse le
/// roundtrip inchangée.
pub fn repair_dila(raw: &[u8], fond: DilaFond) -> Vec<u8> {
    // Le document se déclare UTF-8 ; `from_utf8_lossy` est la frontière franche.
    let text = String::from_utf8_lossy(raw);

    let demojibake = if fond == DilaFond::Jade {
        repair_jade_spans(&text)
    } else {
        text.into_owned()
    };
    demojibake.replace("&amp;nbsp;", "\u{a0}").into_bytes()
}

/// Applique [`undo_mojibake`] dans chaque span `<(SCT|ANA|CITATION_JP)…>…</…>`.
fn repair_jade_spans(text: &str) -> String {
    static SPAN_RE: OnceLock<Regex> = OnceLock::new();
    let re = SPAN_RE.get_or_init(|| {
        Regex::new(r"(?s)<(SCT|ANA|CITATION_JP)\b[^>]*>.*?</(SCT|ANA|CITATION_JP)>")
            .expect("regex span SCT/ANA/CITATION_JP valide")
    });
    re.replace_all(text, |caps: &regex::Captures| undo_mojibake(&caps[0]))
        .into_owned()
}

/// Réparation latin1/cp1252-roundtrip : reconstitue les octets UTF-8 d'origine
/// (chaque caractère mojibaké provient d'un octet ≤ 0xFF mal ré-encodé), puis
/// re-décode. Si la séquence reconstituée est de l'UTF-8 valide ET différente de
/// l'entrée, on la garde ; sinon on laisse le span intact (pas de réparation
/// destructrice). cp1252 est nécessaire car les ponctuations Windows (`’`, `€`,
/// `™`…) occupent la plage 0x80–0x9F qui n'existe pas en latin-1 pur.
fn undo_mojibake(span: &str) -> String {
    let mut bytes = Vec::with_capacity(span.len());
    for ch in span.chars() {
        if let Some(b) = cp1252_byte(ch) {
            bytes.push(b);
        } else {
            // Caractère hors-mojibake (déjà propre / multi-octets non issu d'un
            // octet 0x00–0xFF) : on le ré-émet tel quel en UTF-8.
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    match std::str::from_utf8(&bytes) {
        Ok(decoded) if decoded != span => decoded.to_string(),
        _ => span.to_string(),
    }
}

/// Octet cp1252 dont le décodage donne `ch`, ou `None` si `ch` n'a pas
/// d'antécédent sur un octet 0x00–0xFF. Couvre latin-1 (0x00–0x7F, 0xA0–0xFF =
/// identité) + les ponctuations Windows de la plage 0x80–0x9F.
fn cp1252_byte(ch: char) -> Option<u8> {
    let cp = ch as u32;
    match cp {
        0x00..=0x7F | 0xA0..=0xFF => Some(cp as u8),
        0x20AC => Some(0x80),
        0x201A => Some(0x82),
        0x0192 => Some(0x83),
        0x201E => Some(0x84),
        0x2026 => Some(0x85),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x02C6 => Some(0x88),
        0x2030 => Some(0x89),
        0x0160 => Some(0x8A),
        0x2039 => Some(0x8B),
        0x0152 => Some(0x8C),
        0x017D => Some(0x8E),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x2022 => Some(0x95),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x02DC => Some(0x98),
        0x2122 => Some(0x99),
        0x0161 => Some(0x9A),
        0x203A => Some(0x9B),
        0x0153 => Some(0x9C),
        0x017E => Some(0x9E),
        0x0178 => Some(0x9F),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_listing_separates_stock_and_increments() {
        let html = r#"
            <a href="Freemium_jade_global_20240101-120000.tar.gz">stock</a>
            <a href="JADE_20260612-215918.tar.gz">incr1</a>
            <a href="/OPENDATA/JADE/JADE_20260613-010203.tar.gz">incr2</a>
            <a href="presentation.pdf">noise</a>
        "#;
        let got = classify_listing(html, DilaFond::Jade);
        assert_eq!(
            got,
            vec![
                DilaTarball::Stock("Freemium_jade_global_20240101-120000.tar.gz".to_string()),
                DilaTarball::Increment("JADE_20260612-215918.tar.gz".to_string()),
                DilaTarball::Increment("JADE_20260613-010203.tar.gz".to_string()),
            ]
        );
    }

    #[test]
    fn stock_watermark_derives_increment_comparable_name() {
        let wm = stock_watermark(
            DilaFond::Legi,
            "Freemium_legi_global_20250713-140000.tar.gz",
        );
        assert_eq!(wm, "LEGI_20250713-140000.tar.gz");
        assert!("LEGI_20250714-000000.tar.gz" > wm.as_str());
        assert!("LEGI_20250712-211706.tar.gz" < wm.as_str());
        assert_eq!(
            stock_watermark(DilaFond::Jade, "Freemium_autre.tar.gz"),
            "JADE_.tar.gz"
        );
    }

    #[test]
    fn a_stock_replays_before_its_increments_whatever_the_fund_letter() {
        let names = [
            "CASS_20260930-215413.tar.gz",
            "Freemium_cass_global_20250713-140000.tar.gz",
            "CASS_20250721-212334.tar.gz",
        ];
        let mut paths: Vec<std::path::PathBuf> =
            names.iter().map(|n| Path::new("/c").join(n)).collect();
        paths.sort_by_cached_key(|p| replay_key(DilaFond::Cass, p));
        let got: Vec<_> = paths.iter().map(|p| p.file_name().unwrap()).collect();
        assert_eq!(got, [names[1], names[2], names[0]]);
    }

    // ----- repair_dila -----

    #[test]
    fn repair_jade_fixes_mojibake_only_in_sct_ana_spans() {
        let raw = concat!(
            "<ROOT>",
            "<CONTENU>alinéa propre d'une décision</CONTENU>",
            "<SCT ID=\"x\" TYPE=\"PRINCIPAL\">dâ\u{20ac}\u{2122}une rÃ©f&amp;nbsp;ici</SCT>",
            "<ANA ID=\"y\">des Ã©lÃ©ments</ANA>",
            "</ROOT>"
        );
        let out = String::from_utf8(repair_dila(raw.as_bytes(), DilaFond::Jade)).unwrap();

        assert!(out.contains("<SCT ID=\"x\" TYPE=\"PRINCIPAL\">d’une réf\u{a0}ici</SCT>"));
        assert!(out.contains("<ANA ID=\"y\">des éléments</ANA>"));
        assert!(out.contains("<CONTENU>alinéa propre d'une décision</CONTENU>"));
    }

    #[test]
    fn repair_jade_leaves_clean_span_untouched() {
        let raw = "<ROOT><ANA ID=\"z\">analyse déjà propre, n° 493597</ANA></ROOT>";
        let out = String::from_utf8(repair_dila(raw.as_bytes(), DilaFond::Jade)).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn repair_non_jade_only_deescapes_entities() {
        let raw = "<ROOT><CONTENU>Ã© reste tel quel&amp;nbsp;fin</CONTENU></ROOT>";
        let out = String::from_utf8(repair_dila(raw.as_bytes(), DilaFond::Constit)).unwrap();
        assert_eq!(
            out,
            "<ROOT><CONTENU>Ã© reste tel quel\u{a0}fin</CONTENU></ROOT>"
        );
    }
}
