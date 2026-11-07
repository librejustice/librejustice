//! Données essentielles de la commande publique: the mandatory declaration of a
//! concluded contract, fourth award channel. Not an engine: it enumerates no
//! consultation and founds no row.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use lj_sources::error::{Result, SourceError};

const CATALOGUE: &str = "https://data.economie.gouv.fr/api/explore/v2.1/catalog/datasets";

/// One publication of the obligation and the columns it names. The 2022 arrêté
/// starts at its entry into force, the 2019 one stops at February 2024, and the
/// aggregated set holds a third more rows than either over 2019 to 2022. Their
/// overlap declares the same contracts, which the caller deduplicates.
pub struct Jeu {
    pub id: &'static str,
    objet: &'static str,
    acheteur: &'static str,
    titulaire: &'static str,
    offres: Option<&'static str>,
}

pub const JEUX: [Jeu; 3] = [
    Jeu {
        id: "decp-2022-marches-valides",
        objet: "objet",
        acheteur: "acheteur_id",
        titulaire: "titulaire_id_1",
        offres: Some("offresrecues"),
    },
    Jeu {
        id: "decp-v3-marches-valides",
        objet: "objet",
        acheteur: "acheteur_id",
        titulaire: "titulaire_id_1",
        offres: Some("offresrecues"),
    },
    Jeu {
        id: "decp_augmente",
        objet: "objetmarche",
        acheteur: "idacheteur",
        titulaire: "siretetablissement",
        offres: None,
    },
];

impl Jeu {
    /// Columns asked of the export. The service serves them in its own order,
    /// never this one, so every read resolves a name against the header.
    fn colonnes(&self) -> String {
        let mut noms = vec![
            "id",
            self.objet,
            self.acheteur,
            "datenotification",
            "montant",
            self.titulaire,
            "codecpv",
        ];
        noms.extend(self.offres);
        noms.join(",")
    }
}

/// The catalogue's own publication date for a dataset. Two of the three sets
/// are frozen, one moves, and a pass that finds the three dates it already
/// walked would re-download them whole to re-read its own last answer.
pub async fn modifie_le(jeu: &Jeu) -> Result<String> {
    let corps = super::send_text(
        super::client().get(format!("{CATALOGUE}/{}", jeu.id)),
        "decp catalogue",
    )
    .await?;
    let fiche: serde_json::Value = serde_json::from_str(&corps)
        .map_err(|e| SourceError::Invalid(format!("decp : catalogue {} illisible: {e}", jeu.id)))?;
    fiche["metas"]["default"]["modified"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| SourceError::Invalid(format!("decp : {} sans date de publication", jeu.id)))
}

/// What the last written pass read, kept under `state_dir` beside the exports.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifeste {
    /// Publication date of each dataset, as the catalogue gave it.
    #[serde(default)]
    pub modifies: BTreeMap<String, String>,
    /// Start of the notification window that pass walked.
    #[serde(default)]
    pub depuis: Option<String>,
    #[serde(default)]
    pub lu_le: Option<String>,
}

impl Manifeste {
    pub fn load(dir: &Path) -> Result<Self> {
        let fichier = dir.join("manifeste.json");
        if !fichier.exists() {
            return Ok(Self::default());
        }
        let brut = std::fs::read_to_string(&fichier).map_err(|e| {
            SourceError::Invalid(format!("decp : lecture de {}: {e}", fichier.display()))
        })?;
        serde_json::from_str(&brut)
            .map_err(|e| SourceError::Invalid(format!("decp : manifeste illisible: {e}")))
    }

    pub fn save(&mut self, dir: &Path) -> Result<()> {
        self.lu_le = Some(
            chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S+00:00")
                .to_string(),
        );
        std::fs::create_dir_all(dir).map_err(|e| {
            SourceError::Invalid(format!("decp : création de {}: {e}", dir.display()))
        })?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| SourceError::Invalid(format!("decp : manifeste non sérialisable: {e}")))?;
        let fichier = dir.join("manifeste.json");
        let partiel = fichier.with_extension("json.partiel");
        std::fs::write(&partiel, json).map_err(|e| {
            SourceError::Invalid(format!("decp : écriture de {}: {e}", partiel.display()))
        })?;
        std::fs::rename(&partiel, &fichier).map_err(|e| {
            SourceError::Invalid(format!("decp : renommage de {}: {e}", partiel.display()))
        })
    }

    /// True when the three datasets say what they said and the window asked for
    /// reaches no further back. A manual pass widening the window has work to
    /// do on an export that has not budged.
    pub fn deja_vu(&self, modifies: &BTreeMap<String, String>, depuis: Date) -> bool {
        self.depuis
            .as_deref()
            .and_then(|d| d.parse::<Date>().ok())
            .is_some_and(|vu| vu <= depuis)
            && &self.modifies == modifies
    }
}

/// A concluded contract as its buyer declares it. `acheteur_siren` is the first
/// nine digits of the SIRET, the only form our registry indexes.
#[derive(Debug, Clone)]
pub struct AttributionDecp {
    pub id: String,
    pub objet: String,
    pub acheteur_siren: String,
    pub date_notification: Date,
    pub montant: Option<f64>,
    pub titulaire_siren: Option<String>,
    pub nb_offres: Option<i64>,
    pub cpv: Option<String>,
}

/// Downloads a notification window into `dir`, one file per dataset and start
/// date, and keeps what is already there, the export of the whole history being
/// heavy. A cached file whose header misses a column was asked for
/// another question, and answers this one with holes.
pub async fn telecharger(dir: &Path, jeu: &Jeu, depuis: Date) -> Result<PathBuf> {
    let fichier = dir.join(format!("{}-{depuis}.csv", jeu.id));
    if fichier.exists() && entete_complet(&fichier, jeu) {
        return Ok(fichier);
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| SourceError::Invalid(format!("decp : création de {}: {e}", dir.display())))?;
    let corps = super::send_bytes(
        super::client()
            .get(format!("{CATALOGUE}/{}/exports/csv", jeu.id))
            .query(&[
                ("select", jeu.colonnes().as_str()),
                ("where", &format!("datenotification >= date'{depuis}'")),
                ("delimiter", ";"),
            ]),
        "decp export",
    )
    .await?;
    let partiel = fichier.with_extension("csv.partiel");
    std::fs::write(&partiel, &corps).map_err(|e| {
        SourceError::Invalid(format!("decp : écriture de {}: {e}", partiel.display()))
    })?;
    std::fs::rename(&partiel, &fichier).map_err(|e| {
        SourceError::Invalid(format!("decp : renommage de {}: {e}", partiel.display()))
    })?;
    Ok(fichier)
}

/// Reads the export. `CDL` is what the dataset writes for an undeclared value,
/// on every column: a row without a usable buyer, object or notification date
/// can attach nothing and drops in silence.
pub fn lire(fichier: &Path, jeu: &Jeu) -> Result<Vec<AttributionDecp>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(b';')
        .from_path(fichier)
        .map_err(|e| {
            SourceError::Invalid(format!("decp : lecture de {}: {e}", fichier.display()))
        })?;
    let entete = rdr
        .headers()
        .map_err(|e| SourceError::Invalid(format!("decp : en-tête de {}: {e}", jeu.id)))?
        .clone();
    let rang = |nom: &str| {
        entete
            .iter()
            .position(|c| c.trim_start_matches('\u{feff}').trim() == nom)
    };
    let (Some(c_id), Some(c_objet), Some(c_acheteur), Some(c_date)) = (
        rang("id"),
        rang(jeu.objet),
        rang(jeu.acheteur),
        rang("datenotification"),
    ) else {
        return Err(SourceError::Invalid(format!(
            "decp : {} ne sert pas les colonnes demandées",
            jeu.id
        )));
    };
    let (c_montant, c_titulaire, c_cpv, c_offres) = (
        rang("montant"),
        rang(jeu.titulaire),
        rang("codecpv"),
        jeu.offres.and_then(rang),
    );
    let mut out = Vec::new();
    for ligne in rdr.records() {
        let l = ligne.map_err(|e| SourceError::Invalid(format!("decp : ligne illisible: {e}")))?;
        let champ = |i: usize| {
            l.get(i)
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "CDL")
        };
        let (Some(id), Some(objet), Some(acheteur), Some(date)) = (
            champ(c_id),
            champ(c_objet),
            champ(c_acheteur),
            champ(c_date),
        ) else {
            continue;
        };
        let Some(siren) = siren_de(acheteur) else {
            continue;
        };
        let Ok(date_notification) = date.parse::<Date>() else {
            continue;
        };
        out.push(AttributionDecp {
            id: id.to_string(),
            objet: objet.to_string(),
            acheteur_siren: siren,
            date_notification,
            montant: c_montant.and_then(champ).and_then(|m| m.parse().ok()),
            titulaire_siren: c_titulaire.and_then(champ).and_then(siren_de),
            nb_offres: c_offres.and_then(champ).and_then(|n| n.parse().ok()),
            cpv: c_cpv.and_then(champ).map(str::to_string),
        });
    }
    Ok(out)
}

fn entete_complet(fichier: &Path, jeu: &Jeu) -> bool {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(fichier) else {
        return false;
    };
    let mut entete = String::new();
    if std::io::BufReader::new(f).read_line(&mut entete).is_err() {
        return false;
    }
    let servies: Vec<&str> = entete
        .trim_start_matches('\u{feff}')
        .trim()
        .split(';')
        .collect();
    jeu.colonnes().split(',').all(|n| servies.contains(&n))
}

/// The SIREN of a declared identifier. The column carries a SIRET when the
/// declared type is `SIRET`, but just as well a VAT number or a foreign id:
/// only nine or fourteen digits name a French one.
fn siren_de(id: &str) -> Option<String> {
    let chiffres: String = id.chars().filter(|c| c.is_ascii_digit()).collect();
    (chiffres.len() == 9 || chiffres.len() == 14).then(|| chiffres[..9].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_siren_sort_du_siret_et_rien_d_autre() {
        assert_eq!(siren_de("24450046800040").as_deref(), Some("244500468"));
        assert_eq!(siren_de("244500468").as_deref(), Some("244500468"));
        assert_eq!(siren_de("FR12345678901"), None);
        assert_eq!(siren_de("CDL"), None);
        assert_eq!(siren_de(""), None);
    }

    #[test]
    fn un_cache_a_qui_manque_une_colonne_n_est_pas_reutilise() {
        let f = std::env::temp_dir().join("decp_test_entete.csv");
        std::fs::write(&f, "id;objet;acheteur_id;datenotification\n").unwrap();
        assert!(!entete_complet(&f, &JEUX[0]));
        std::fs::write(
            &f,
            format!("\u{feff}{}\r\n", JEUX[0].colonnes().replace(',', ";")),
        )
        .unwrap();
        assert!(entete_complet(&f, &JEUX[0]));
        std::fs::remove_file(&f).ok();
    }

    #[test]
    fn l_ordre_servi_n_est_pas_l_ordre_demande() {
        let csv = "codecpv;datenotification;objet;id;acheteur_id;montant;titulaire_id_1;offresrecues\n\
                   45233140-2;2025-06-05;Travaux de voirie;2025M1;24450046800040;39900.0;81295216600021;2\n\
                   CDL;2025-06-05;Objet sans acheteur;2025M2;CDL;100.0;CDL;CDL\n\
                   CDL;CDL;Objet sans date;2025M3;24450046800040;100.0;CDL;CDL\n";
        let f = std::env::temp_dir().join("decp_test_lire.csv");
        std::fs::write(&f, csv).unwrap();
        let lignes = lire(&f, &JEUX[0]).unwrap();
        std::fs::remove_file(&f).ok();
        assert_eq!(lignes.len(), 1);
        assert_eq!(lignes[0].id, "2025M1");
        assert_eq!(lignes[0].objet, "Travaux de voirie");
        assert_eq!(lignes[0].acheteur_siren, "244500468");
        assert_eq!(lignes[0].montant, Some(39900.0));
        assert_eq!(lignes[0].titulaire_siren.as_deref(), Some("812952166"));
        assert_eq!(lignes[0].nb_offres, Some(2));
        assert_eq!(lignes[0].cpv.as_deref(), Some("45233140-2"));
    }

    /// The two halves of the quick exit: upstream moving, and a wider window
    /// asked for on an upstream that has not.
    #[test]
    fn la_passe_se_saute_sur_le_catalogue_et_sur_la_fenetre() {
        let dir = std::env::temp_dir().join("decp_test_manifeste");
        std::fs::remove_dir_all(&dir).ok();
        let vu: BTreeMap<String, String> = [("decp-2022-marches-valides", "2026-01-01T00:00:00Z")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let vierge = Manifeste::load(&dir).unwrap();
        assert!(!vierge.deja_vu(&vu, Date::constant(2026, 3, 16)));

        let mut m = Manifeste {
            modifies: vu.clone(),
            depuis: Some("2026-03-16".to_string()),
            lu_le: None,
        };
        m.save(&dir).unwrap();
        let relu = Manifeste::load(&dir).unwrap();
        assert!(relu.lu_le.is_some());
        assert!(relu.deja_vu(&vu, Date::constant(2026, 3, 16)));
        assert!(relu.deja_vu(&vu, Date::constant(2026, 6, 18)));
        assert!(!relu.deja_vu(&vu, Date::constant(2022, 1, 1)));

        let bouge: BTreeMap<String, String> =
            [("decp-2022-marches-valides", "2026-01-02T00:00:00Z")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert!(!relu.deja_vu(&bouge, Date::constant(2026, 3, 16)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn le_jeu_agrege_nomme_ses_colonnes_autrement() {
        let csv = "id;objetmarche;idacheteur;datenotification;montant;siretetablissement;codecpv\n\
                   202396203200;Construction de 102 logements;53531174000034;2023-07-28;298610.0;50149657400028;44316500-3\n";
        let f = std::env::temp_dir().join("decp_test_augmente.csv");
        std::fs::write(&f, csv).unwrap();
        let lignes = lire(&f, &JEUX[2]).unwrap();
        std::fs::remove_file(&f).ok();
        assert_eq!(lignes.len(), 1);
        assert_eq!(lignes[0].acheteur_siren, "535311740");
        assert_eq!(lignes[0].titulaire_siren.as_deref(), Some("501496574"));
        assert_eq!(lignes[0].nb_offres, None);
    }
}
