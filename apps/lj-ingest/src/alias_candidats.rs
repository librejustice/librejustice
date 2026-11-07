//! Job `alias-candidats` : remplit `alias_candidate` depuis la locution
//! « dite loi » des corps. N'active rien : l'activation est un commit dans
//! `legal_aliases.json`. Les noms d'arrêts, eux, ne passent plus par une revue :
//! le job `renvois` les pose directement sur la décision.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_core::case_names::case_name_key;
use lj_extract::eponyme::{parse_dite_loi, parse_titre_loi};
use lj_store::repository::{AliasCandidat, DecisionRepository};

use crate::config::Settings;

const LOT: i64 = 2_000;

type Cle = (String, String, Option<NaiveDate>);

/// La graphie ne fait pas partie de la clé : deux casses d'un même surnom sont
/// un seul candidat, et la plus vue sert d'affichage.
#[derive(Default)]
struct Compte {
    support: i32,
    formes: HashMap<String, i32>,
}

pub async fn run(revue: Option<i64>) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    match revue {
        Some(limite) => relire(&repo, "loi", limite).await?,
        None => dite_loi(&repo).await?,
    }
    Ok(())
}

/// Sort la revue en TSV : c'est un humain qui décide de l'activation, et la
/// colonne `second` porte la décision plus que le support.
async fn relire(repo: &DecisionRepository<'_>, genre: &str, limite: i64) -> Result<()> {
    let lignes = repo.alias_candidates_revue(genre, limite).await?;
    println!("key\tnickname\ttarget\tdate\tsupport\tsecond\tin_corpus");
    for l in &lignes {
        let date = l.target_date.map(|d| d.to_string()).unwrap_or_default();
        let fonds = match l.decision_id {
            Some(id) => id.to_string(),
            None => "-".into(),
        };
        println!(
            "{}\t{}\t{}\t{date}\t{}\t{}\t{fonds}",
            l.alias_key, l.alias_vu, l.target_ref, l.support, l.support_second
        );
    }
    println!("{} surnoms, genre {genre}", lignes.len());
    Ok(())
}

/// Le surnom sans numéro ni date à portée est compté et jeté : il ne se relit
/// pas.
async fn dite_loi(repo: &DecisionRepository<'_>) -> Result<()> {
    repo.alias_candidates_reset("loi", "dite-loi").await?;
    let catalogue = lois_par_date(repo).await?;
    let mut compte: HashMap<Cle, Compte> = HashMap::new();
    let (mut apres, mut sources, mut orphelins, mut par_date) = (0i64, 0usize, 0usize, 0usize);
    loop {
        let lot = repo.dite_loi_batch(apres, LOT).await?;
        if lot.is_empty() {
            break;
        }
        for (id, texte) in &lot {
            apres = *id;
            sources += 1;
            let mut vus: HashMap<Cle, String> = HashMap::new();
            for c in parse_dite_loi(texte) {
                let par_le_catalogue = c.numero.is_none();
                let numero = c.numero.or_else(|| {
                    let date = c.date_iso.as_deref()?;
                    match catalogue.get(date)?.as_slice() {
                        [seule] => Some(seule.clone()),
                        _ => None,
                    }
                });
                match numero {
                    Some(num) => {
                        par_date += usize::from(par_le_catalogue);
                        vus.insert((case_name_key(&c.surnom), num, None), c.surnom);
                    }
                    None => orphelins += 1,
                }
            }
            fusionne(&mut compte, vus);
        }
    }
    let ecrits = ecrire(repo, "loi", "dite-loi", compte).await?;
    println!(
        "dite-loi : {sources} décisions, {ecrits} couples, dont {par_date} voix résolues par la \
         seule date, {orphelins} surnoms sans référence à portée"
    );
    Ok(())
}

/// Numéros de loi par date de signature, depuis les titres du catalogue. Une
/// date que deux lois partagent ne résout rien : elle sort avec ses deux
/// numéros, et le candidat qui n'a que la date est jeté.
async fn lois_par_date(repo: &DecisionRepository<'_>) -> Result<HashMap<String, Vec<String>>> {
    let mut par_date: HashMap<String, Vec<String>> = HashMap::new();
    for titre in repo.titres_de_lois().await? {
        let Some((numero, date)) = parse_titre_loi(&titre) else {
            continue;
        };
        let numeros = par_date.entry(date).or_default();
        if !numeros.contains(&numero) {
            numeros.push(numero);
        }
    }
    Ok(par_date)
}

fn fusionne(compte: &mut HashMap<Cle, Compte>, vus: HashMap<Cle, String>) {
    for (cle, forme) in vus {
        let e = compte.entry(cle).or_default();
        e.support += 1;
        *e.formes.entry(forme).or_default() += 1;
    }
}

async fn ecrire(
    repo: &DecisionRepository<'_>,
    genre: &str,
    gisement: &str,
    compte: HashMap<Cle, Compte>,
) -> Result<usize> {
    let lot: Vec<AliasCandidat> = compte
        .into_iter()
        .map(|((alias_key, target_ref, target_date), c)| AliasCandidat {
            alias_vu: c
                .formes
                .into_iter()
                .max_by_key(|(f, n)| (*n, f.clone()))
                .map(|(f, _)| f)
                .unwrap_or_else(|| alias_key.clone()),
            alias_key,
            target_ref,
            target_date,
            support: c.support,
        })
        .collect();
    for tranche in lot.chunks(5_000) {
        repo.alias_candidates_bulk(genre, gisement, tranche).await?;
    }
    Ok(lot.len())
}
