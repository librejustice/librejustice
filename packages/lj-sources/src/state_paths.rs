//! Layout du répertoire d'état (`state_dir`) — source de vérité unique des
//! chemins sous `LIBREJUSTICE_STATE_DIR` (défaut `~/.local/share/librejustice`).
//!
//! Deux familles :
//! - `ingest/` : tout ce que `lj-ingest` LIT (fonds auto-téléchargés sous
//!   `ingest/cache/`, datasets légaux curés sous `ingest/corpus/`) ;
//! - `bench/`, `analysis/` : scratch des bancs offline.
//!
//! `sources/` (matière première sourcée à la main), `tls/` et `pgdata/`
//! (conteneur Postgres) restent hors code.

use std::path::{Path, PathBuf};

/// Noms des sous-dossiers de fonds sous [`StatePaths::cache`]. Source de vérité
/// unique : le downloader (écriture) et le pipeline d'ingest (lecture) passent
/// tous par [`StatePaths`], jamais par une constante recopiée.
pub(crate) const ARIANE_DIR: &str = "ariane";
pub(crate) const DILA_DIR: &str = "dila";
pub(crate) const JUDILIBRE_DIR: &str = "judilibre";
pub(crate) const OPENDATA_DIR: &str = "opendata_conseil_etat";
pub(crate) const CEDH_DIR: &str = "cedh";
pub(crate) const CNDA_DIR: &str = "cnda";
pub(crate) const DDD_DIR: &str = "ddd";
pub(crate) const ADLC_DIR: &str = "adlc";
pub(crate) const AMF_DIR: &str = "amf";
pub(crate) const JURADM_SITE_DIR: &str = "juradm-site";
pub(crate) const CJUE_DIR: &str = "cjue";
pub(crate) const EU_LAW_DIR: &str = "eu_law";
pub(crate) const PRESSE_DIR: &str = "presse";
pub(crate) const DECP_DIR: &str = "decp";

/// Résout et compose les chemins sous `state_dir`.
#[derive(Debug, Clone)]
pub struct StatePaths {
    root: PathBuf,
}

impl StatePaths {
    /// Résolveur unique : `LIBREJUSTICE_STATE_DIR`, sinon
    /// `$HOME/.local/share/librejustice`.
    pub fn from_env() -> Self {
        let root = std::env::var_os("LIBREJUSTICE_STATE_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default_state_dir);
        Self { root }
    }

    /// Construit à partir d'une racine explicite (pur, sans I/O).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Racine `state_dir`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `ingest/cache` — fonds auto-téléchargés (working data du pipeline).
    pub fn cache(&self) -> PathBuf {
        self.root.join("ingest/cache")
    }

    /// `ingest/corpus` — datasets légaux curés (chargés par
    /// `load-legal-corpus`).
    pub fn corpus(&self) -> PathBuf {
        self.root.join("ingest/corpus")
    }

    /// `ingest/instruments` — datasets de frappe d'identités d'instruments
    /// (`mint-instruments`).
    pub fn instruments(&self) -> PathBuf {
        self.root.join("ingest/instruments")
    }

    /// `bench` — scratch régénérable du banc.
    pub fn bench(&self) -> PathBuf {
        self.root.join("bench")
    }

    /// `analysis` — sondes/inventaires matérialisés depuis la prod.
    pub fn analysis(&self) -> PathBuf {
        self.root.join("analysis")
    }

    /// `ingest/cache/ariane` — ArianeWeb (HTML AJCE shardés + manifeste).
    pub fn ariane(&self) -> PathBuf {
        self.cache().join(ARIANE_DIR)
    }

    /// `ingest/cache/dila` — fonds DILA bulk (tarballs par infixe).
    pub fn dila(&self) -> PathBuf {
        self.cache().join(DILA_DIR)
    }

    /// `ingest/cache/judilibre` — sync incrémental Judilibre.
    pub fn judilibre(&self) -> PathBuf {
        self.cache().join(JUDILIBRE_DIR)
    }

    /// `ingest/cache/opendata_conseil_etat` — ZIP opendata Conseil d'État.
    pub fn opendata(&self) -> PathBuf {
        self.cache().join(OPENDATA_DIR)
    }

    /// `ingest/cache/cedh` — arrêts CEDH (HUDOC).
    pub fn cedh(&self) -> PathBuf {
        self.cache().join(CEDH_DIR)
    }

    /// `ingest/cache/cnda` — décisions CNDA.
    pub fn cnda(&self) -> PathBuf {
        self.cache().join(CNDA_DIR)
    }

    /// `ingest/cache/ddd`, the catalogue of the Défenseur des droits.
    pub fn ddd(&self) -> PathBuf {
        self.cache().join(DDD_DIR)
    }

    /// `ingest/cache/adlc`, the dataset of the Autorité de la concurrence.
    pub fn adlc(&self) -> PathBuf {
        self.cache().join(ADLC_DIR)
    }

    /// `ingest/cache/amf`, the pages of the AMF's Commission des sanctions decisions.
    pub fn amf(&self) -> PathBuf {
        self.cache().join(AMF_DIR)
    }

    /// `ingest/cache/juradm-site`, the items read on the sites of the administrative courts.
    pub fn juradm_site(&self) -> PathBuf {
        self.cache().join(JURADM_SITE_DIR)
    }

    /// `ingest/cache/cjue` — arrêts CJUE.
    pub fn cjue(&self) -> PathBuf {
        self.cache().join(CJUE_DIR)
    }

    /// `ingest/cache/eu_law` — manifeste du fond eu-law (watermark delta +
    /// versions futures en attente).
    pub fn eu_law(&self) -> PathBuf {
        self.cache().join(EU_LAW_DIR)
    }

    /// `ingest/cache/presse` — dumps NDJSON des sessions navigateur sur le
    /// portail d'annonces légales, un fichier par session.
    pub fn presse(&self) -> PathBuf {
        self.cache().join(PRESSE_DIR)
    }

    /// `ingest/cache/decp` : exports CSV des données essentielles de la
    /// commande publique, un fichier par fenêtre de notification.
    pub fn decp(&self) -> PathBuf {
        self.cache().join(DECP_DIR)
    }
}

/// `$HOME/.local/share/librejustice`.
fn default_state_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".local/share/librejustice")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_stable() {
        let p = StatePaths::new(PathBuf::from("/s"));
        assert_eq!(p.cache(), PathBuf::from("/s/ingest/cache"));
        assert_eq!(p.corpus(), PathBuf::from("/s/ingest/corpus"));
        assert_eq!(p.instruments(), PathBuf::from("/s/ingest/instruments"));
        assert_eq!(p.bench(), PathBuf::from("/s/bench"));
        assert_eq!(p.analysis(), PathBuf::from("/s/analysis"));
        assert_eq!(p.ariane(), PathBuf::from("/s/ingest/cache/ariane"));
        assert_eq!(p.dila(), PathBuf::from("/s/ingest/cache/dila"));
        assert_eq!(p.judilibre(), PathBuf::from("/s/ingest/cache/judilibre"));
        assert_eq!(
            p.opendata(),
            PathBuf::from("/s/ingest/cache/opendata_conseil_etat")
        );
        assert_eq!(p.cedh(), PathBuf::from("/s/ingest/cache/cedh"));
        assert_eq!(p.cnda(), PathBuf::from("/s/ingest/cache/cnda"));
        assert_eq!(p.cjue(), PathBuf::from("/s/ingest/cache/cjue"));
        assert_eq!(p.eu_law(), PathBuf::from("/s/ingest/cache/eu_law"));
        assert_eq!(p.decp(), PathBuf::from("/s/ingest/cache/decp"));
    }
}
