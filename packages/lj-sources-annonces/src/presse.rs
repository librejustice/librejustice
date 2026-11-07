use std::path::Path;

use serde::Deserialize;

use lj_annonces::{
    cles_jointure_depuis_urls, echeance_ts, embellir_titre, nettoyer_acheteur, parse_heure,
    parse_ts_iso, AnnonceBrute, ContenuAnnonce, NatureMarche,
};
use lj_core::dates::parse_date_fr;

use super::MoteurRun;
use lj_sources::error::{Result, SourceError};

/// One line of the dump. Lenient on what it reads: the collector only fills
/// what the notice card carries.
#[derive(Debug, Deserialize)]
struct Ligne {
    /// Identifiant de l'avis dans le portail — la clé de ré-énumération.
    uid: String,
    titre: String,
    #[serde(default)]
    acheteur: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    /// Le journal qui a publié l'avis : la provenance presse, qu'aucun autre
    /// canal ne porte.
    #[serde(default)]
    journal: Option<String>,
    #[serde(default)]
    nature: Option<String>,
    #[serde(default)]
    date_publication: Option<String>,
    /// Échéance ISO (`2026-08-28T12:00:00+02:00`) ou date française
    /// (`28/08/2026`), l'heure à part quand la carte la sépare.
    #[serde(default)]
    echeance: Option<String>,
    #[serde(default)]
    heure_limite: Option<String>,
    /// La page de l'avis sur le portail.
    #[serde(default)]
    url_avis: Option<String>,
    /// Les liens sortants de l'avis : profil d'acheteur, DCE, dépôt.
    #[serde(default)]
    urls: Vec<String>,
}

/// Reads one dump. `annonce` = lines read, so the collected/announced check
/// sees a truncated collection.
pub async fn enumerate(chemin: &Path) -> Result<MoteurRun> {
    let contenu = tokio::fs::read_to_string(chemin).await.map_err(|e| {
        SourceError::Invalid(format!(
            "presse: dump illisible ({}): {e}",
            chemin.display()
        ))
    })?;
    Ok(depuis_ndjson(&contenu))
}

fn depuis_ndjson(contenu: &str) -> MoteurRun {
    let mut rows = Vec::new();
    let mut lues = 0;
    let mut echecs = 0;
    for ligne in contenu.lines().filter(|l| !l.trim().is_empty()) {
        lues += 1;
        match serde_json::from_str::<Ligne>(ligne) {
            Ok(l) => match depuis_ligne(l) {
                Some(r) => rows.push(r),
                None => echecs += 1,
            },
            Err(_) => echecs += 1,
        }
    }
    MoteurRun {
        annonce: Some(lues),
        echecs,
        hotes: Vec::new(),
        rows: super::dedup_source_uid(rows),
    }
}

fn depuis_ligne(l: Ligne) -> Option<AnnonceBrute> {
    let objet = embellir_titre(l.titre.trim());
    if objet.is_empty() || l.uid.trim().is_empty() {
        return None;
    }
    // Une carte sans lien sortant ne mène à rien : ni dossier, ni tenant, et le
    // titre seul ne rattache pas.
    if l.url_avis.is_none() && l.urls.is_empty() {
        return None;
    }
    let echeance = l.echeance.as_deref().and_then(|e| {
        parse_ts_iso(e).or_else(|| {
            let d = parse_date_fr(e)?;
            Some(echeance_ts(
                d,
                l.heure_limite.as_deref().and_then(parse_heure),
            ))
        })
    });
    Some(AnnonceBrute {
        contenu: ContenuAnnonce::default(),
        descripteurs: Vec::new(),
        moteur: "presse",
        source_uid: format!("presse:{}", l.uid.trim()),
        cles_jointure: cles_jointure_depuis_urls(
            l.urls
                .iter()
                .map(String::as_str)
                .chain(l.url_avis.as_deref()),
        ),
        reference: l
            .reference
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty()),
        acheteur: l
            .acheteur
            .as_deref()
            .map(nettoyer_acheteur)
            .filter(|a| !a.is_empty()),
        siren: None,
        objet,
        nature: l.nature.as_deref().and_then(NatureMarche::parse),
        cpv: None,
        zones: Vec::new(),
        date_publication: l.date_publication.as_deref().and_then(parse_date_fr),
        echeance,
        url_avis: l.url_avis,
        url_rc: None,
        // Le premier lien sortant est le dossier quand il en porte un ;
        // `url_depot` garde le tenant, que le recensement classe.
        url_dce: l.urls.first().cloned(),
        url_depot: l.urls.first().cloned(),
        procedure: l.journal,
    })
}

#[cfg(test)]
mod tests {
    use super::depuis_ndjson;

    /// A JAL notice absent from the BOAMP whose outgoing link carries the key of an
    /// Atline consultation in stock: the press row merges into it, no duplicate.
    #[test]
    fn un_avis_de_presse_porte_la_cle_de_jointure_de_sa_plateforme() {
        let dump = r#"
{"uid":"9dnadna513003000-2026","titre":"Réalisation de la voirie définitive rue de la Gravière","acheteur":"Communauté de communes de la Basse Zorn (67)","journal":"Dernières Nouvelles d'Alsace","date_publication":"15/08/2026","echeance":"28/08/2026","heure_limite":"12h00","url_avis":"https://portail.example/avis/9dnadna513003000-2026","urls":["https://www.marches-securises.fr/entreprise/?module=consultation&cle_dce=1786547716vlgv4uwcmnjz"]}
{"uid":"vide"}
"#;
        let run = depuis_ndjson(dump);
        assert_eq!(run.annonce, Some(2));
        assert_eq!(run.echecs, 1);
        assert_eq!(run.rows.len(), 1);
        let r = &run.rows[0];
        assert_eq!(r.source_uid, "presse:9dnadna513003000-2026");
        assert_eq!(
            r.cles_jointure,
            vec!["atline:1786547716vlgv4uwcmnjz".to_string()]
        );
        assert_eq!(
            r.acheteur.as_deref(),
            Some("Communauté de communes de la Basse Zorn")
        );
        assert_eq!(
            r.echeance.map(|t| t.to_string()),
            Some("2026-08-28T10:00:00Z".to_string())
        );
        assert_eq!(r.procedure.as_deref(), Some("Dernières Nouvelles d'Alsace"));
    }

    /// Une carte sans lien sortant ne mène ni au dossier ni au tenant.
    #[test]
    fn une_carte_sans_lien_ne_fait_pas_de_ligne() {
        let run = depuis_ndjson(r#"{"uid":"x","titre":"Un marché quelconque de travaux"}"#);
        assert!(run.rows.is_empty());
        assert_eq!(run.echecs, 1);
    }
}
