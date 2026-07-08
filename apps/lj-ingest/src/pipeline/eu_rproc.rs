//! Ingestion catalogue des **règlements de procédure** des juridictions UE
//! (Cour de justice, Tribunal, ex-Tribunal de la fonction publique) — corpus gap
//! ADR 0137/0138.
//!
//! Ces instruments procéduraux sont cités ≈25 K fois par les décisions CJUE mais
//! sans numéro année/séquence, la passe EUR-Lex droit dérivé (`ingest-eu-catalog`)
//! ne les capte pas. On les ajoute en **entrées catalogue curées** (comme EUR-Lex,
//! ADR 0138) : la forme dominante citée devient le `title_key` (liée par la règle
//! de fold de titres du linker) ; les variantes (ancien nom de la juridiction,
//! version datée) vivent en alias TSV du linker
//! (`packages/lj-extract/data/link_aliases.tsv`, ADR 0145).
//!
//! Idempotent : dataset déjà présent non ré-écrit. `dry_run` liste sans écrire.

use anyhow::{anyhow, Result};

use crate::config::Settings;

/// Un règlement de procédure curé. `title_key_form` = forme citée dominante
/// (liée par fold de titre) ; les variantes vivent en alias TSV du linker.
struct Rproc {
    text_uid: &'static str,
    /// Titre canonique d'affichage.
    title: &'static str,
    /// Forme citée dominante → `title_key` (liée par fold de titre du linker).
    title_key_form: &'static str,
    /// Date d'adoption ISO `YYYY-MM-DD` (texte vivant / version consolidée courante).
    date_texte: &'static str,
    /// CELEX de la version consolidée (pour `source_url` EUR-Lex).
    celex: &'static str,
}

/// Catalogue curé (texte vivant). Le Tribunal de première instance (TPICE) est devenu
/// le Tribunal (art. 19 TUE post-Lisbonne) : même lignée, résolu au règlement courant.
const RPROCS: &[Rproc] = &[
    Rproc {
        text_uid: "EU/RPROC/CJUE",
        title: "Règlement de procédure de la Cour de justice",
        title_key_form: "Règlement de procédure de la cour",
        date_texte: "2012-09-25",
        celex: "32012Q0929(01)",
    },
    Rproc {
        text_uid: "EU/RPROC/TRIBUNAL",
        title: "Règlement de procédure du Tribunal",
        title_key_form: "Règlement de procédure du tribunal",
        date_texte: "2015-03-04",
        celex: "32015Q0423(01)",
    },
    Rproc {
        text_uid: "EU/RPROC/TFP",
        title: "Règlement de procédure du Tribunal de la fonction publique de l'Union européenne",
        title_key_form: "Règlement de procédure du tribunal de la fonction publique",
        date_texte: "2014-07-21",
        celex: "32014Q0919(01)",
    },
];

/// Génère les datasets catalogue des règlements de procédure (ADR 0137/0138).
/// `dry_run` : n'écrit rien, liste les instruments.
pub async fn ingest_eu_rproc(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();

    if dry_run {
        for r in RPROCS {
            println!("  {} — {}", r.text_uid, r.title);
        }
        println!(
            "DRY-RUN ingest-eu-rproc : {} instrument(s) ; aucune écriture.",
            RPROCS.len()
        );
        return Ok(());
    }

    // 1. Datasets catalogue (idempotent : fichier présent = déjà généré).
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("mkdir {}: {e}", dir.display()))?;
    let mut written = 0usize;
    for r in RPROCS {
        let path = dir.join(format!(
            "eu-rproc-{}.json",
            r.text_uid.replace('/', "-").to_lowercase()
        ));
        if path.exists() {
            continue;
        }
        let doc = serde_json::json!({
            "text_uid": r.text_uid,
            "source": "eur-lex",
            "jurisdiction": "UE",
            "title": r.title,
            // Forme citée dominante : liée par le fold de titre du linker.
            "title_key": r.title_key_form,
            "nature": "REGLEMENT",
            "translation": "officiel",
            "source_url": format!("http://publications.europa.eu/resource/celex/{}", r.celex),
            "date_texte": r.date_texte,
            "articles": [],
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&doc)?)
            .map_err(|e| anyhow!("write {}: {e}", path.display()))?;
        written += 1;
    }

    println!(
        "ingest-eu-rproc : {written} dataset(s) écrit(s) ; puis `lj-ingest \
         load-legal-corpus` (les citations s'attachent à la prochaine passe intégrale)."
    );
    Ok(())
}
