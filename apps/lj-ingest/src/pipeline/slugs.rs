//! Assignation des slugs de textes (ADR 0162, cascade sans uid ADR 0206) :
//! unique écrivain de `legal_text.slug`. Tout texte reçoit un slug
//! déterministe, immuable et **lisible** — jamais d'uid dedans. Cascade en
//! collision : slug complet du titre, puis date du texte, puis NOR, puis
//! ordinal. Les textes hors juridiction FR/UE/INTL portent le pays en suffixe
//! ISO 3166-1 alpha-3, radical débarrassé de ses marqueurs pays
//! (`code-civil-bel`). La passe tourne en fin de
//! chaque ingest référentiel et s'expose en commande `assign-slugs`
//! (backfill). Idempotente (#7) : elle ne remplit que les `slug NULL`.

use anyhow::{anyhow, bail, Result};
use lj_store::repository::{DecisionRepository, SlugSourceRow};
use std::collections::HashSet;

use crate::config::Settings;

/// Longueur max du slug nu (frontière de mot) — les titres de lois/arrêtés
/// font couramment 150-250 chars, l'URL n'a pas à porter tout l'intitulé.
const MAX_SLUG_LEN: usize = 80;

/// Tronque un slug déjà formé à [`MAX_SLUG_LEN`], sur une frontière de tiret.
fn truncate_slug(full: &str) -> String {
    if full.len() <= MAX_SLUG_LEN {
        return full.to_string();
    }
    match full[..=MAX_SLUG_LEN].rfind('-') {
        Some(cut) if cut > 0 => full[..cut].to_string(),
        _ => full[..MAX_SLUG_LEN].to_string(),
    }
}

/// Pays d'une juridiction hors corpus propre (FR/UE/INTL) : suffixe ISO
/// 3166-1 alpha-3 minuscule, et tokens de radical à stripper (gentilés, nom
/// du pays) — l'identité pays vit dans le seul suffixe, jamais dans le
/// radical (`code-civil-syr`, pas `code-civil-syrien-syr`). Code inconnu =
/// erreur franche (#12) : étendre la table à la curation du texte.
fn country(a2: &str) -> Result<(&'static str, &'static [&'static str])> {
    Ok(match a2 {
        "AM" => ("arm", &["armenien", "armenienne", "armenie"]),
        "AO" => ("ago", &["angola", "angolais", "angolaise"]),
        "AT" => ("aut", &["autrichien", "autrichienne", "autriche"]),
        "BE" => ("bel", &["belge", "belgique"]),
        "BF" => ("bfa", &["burkinabe", "burkina-faso", "burkina"]),
        "BG" => ("bgr", &["bulgare", "bulgarie"]),
        "BI" => ("bdi", &["burundi", "burundais", "burundaise"]),
        "BJ" => ("ben", &["beninois", "beninoise", "benin"]),
        "CD" => ("cod", &["rd-congo", "rdc", "congolais", "congolaise"]),
        "CF" => ("caf", &["centrafrique", "centrafricain", "centrafricaine"]),
        "CG" => (
            "cog",
            &[
                "congo-brazzaville",
                "brazzaville",
                "congolais",
                "congolaise",
                "congo",
            ],
        ),
        "CH" => ("che", &["suisse"]),
        "CI" => ("civ", &["cote-d-ivoire", "ivoirien", "ivoirienne"]),
        "CM" => ("cmr", &["cameroun", "camerounais", "camerounaise"]),
        "DE" => ("deu", &["allemand", "allemande", "allemagne"]),
        "DJ" => ("dji", &["djibouti", "djiboutien", "djiboutienne"]),
        "DO" => (
            "dom",
            &["republique-dominicaine", "dominicain", "dominicaine"],
        ),
        "DZ" => ("dza", &["algerie", "algerien", "algerienne"]),
        "EG" => ("egy", &["egyptien", "egyptienne", "egypte"]),
        "ES" => ("esp", &["espagnol", "espagnole", "espagne"]),
        "GA" => ("gab", &["gabon", "gabonais", "gabonaise"]),
        "GN" => ("gin", &["guinee", "guineen", "guineenne"]),
        "GR" => ("grc", &["grec", "grecque", "grece"]),
        "HT" => ("hti", &["haitien", "haitienne", "haiti"]),
        "HU" => ("hun", &["hongrois", "hongroise", "hongrie"]),
        "IQ" => ("irq", &["irakien", "irakienne", "irak"]),
        "IT" => ("ita", &["italien", "italienne", "italie"]),
        "JO" => ("jor", &["jordanien", "jordanienne", "jordanie"]),
        "KM" => ("com", &["comores", "comorien", "comorienne"]),
        "LB" => ("lbn", &["libanais", "libanaise", "liban"]),
        "LU" => ("lux", &["luxembourgeois", "luxembourgeoise", "luxembourg"]),
        "MA" => ("mar", &["marocain", "marocaine", "maroc"]),
        "MC" => ("mco", &["monaco", "monegasque"]),
        "MG" => ("mdg", &["malgache", "madagascar"]),
        "ML" => ("mli", &["malien", "malienne", "mali"]),
        "MR" => ("mrt", &["mauritanien", "mauritanienne", "mauritanie"]),
        "MU" => ("mus", &["mauricien", "mauricienne", "maurice"]),
        "NC" => ("ncl", &["nouvelle-caledonie", "caledonien", "caledonienne"]),
        "NE" => ("ner", &["nigerien", "nigerienne", "niger"]),
        "NG" => ("nga", &["nigerian", "nigeriane", "nigeria"]),
        "NL" => ("nld", &["neerlandais", "neerlandaise", "pays-bas"]),
        "PE" => ("per", &["peruvien", "peruvienne", "perou"]),
        "PF" => ("pyf", &["polynesie-francaise", "polynesien", "polynesie"]),
        "PL" => ("pol", &["polonais", "polonaise", "pologne"]),
        "PT" => ("prt", &["portugais", "portugaise", "portugal"]),
        "RO" => ("rou", &["roumain", "roumaine", "roumanie"]),
        "RS" => ("srb", &["serbe", "serbie"]),
        "RU" => ("rus", &["russe", "russie"]),
        "RW" => ("rwa", &["rwandais", "rwandaise", "rwanda"]),
        "SN" => ("sen", &["senegalais", "senegalaise", "senegal"]),
        "ST" => ("stp", &["sao-tome-et-principe", "sao-tome", "santomeen"]),
        "SY" => ("syr", &["syrien", "syrienne", "syrie"]),
        "TD" => ("tcd", &["tchadien", "tchadienne", "tchad"]),
        "TG" => ("tgo", &["togolais", "togolaise", "togo"]),
        "TN" => ("tun", &["tunisien", "tunisienne", "tunisie"]),
        "TR" => ("tur", &["turc", "turque", "turquie"]),
        "UA" => ("ukr", &["ukrainien", "ukrainienne", "ukraine"]),
        "VE" => ("ven", &["venezuelien", "venezuelienne", "venezuela"]),
        "VN" => ("vnm", &["vietnam", "vietnamien", "vietnamienne"]),
        other => bail!("juridiction '{other}' absente de la table pays alpha-3 — l'y ajouter"),
    })
}

/// Retire du slug les séquences de tokens pays (bornées aux tirets : `belge`,
/// `cote-d-ivoire`) et le token alpha-2 minuscule. Les composés dont un seul
/// token matche (`franco-algerienne`) relèvent de la curation, pas de la passe.
fn strip_country_tokens(slug: &str, tokens: &[&str], a2: &str) -> String {
    let mut toks: Vec<String> = slug.split('-').map(str::to_string).collect();
    let a2_low = a2.to_lowercase();
    let mut seqs: Vec<Vec<&str>> = tokens.iter().map(|s| s.split('-').collect()).collect();
    seqs.push(vec![&a2_low]);
    seqs.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for seq in &seqs {
        let mut i = 0;
        while i + seq.len() <= toks.len() {
            if toks[i..i + seq.len()]
                .iter()
                .map(String::as_str)
                .eq(seq.iter().copied())
            {
                toks.drain(i..i + seq.len());
            } else {
                i += 1;
            }
        }
    }
    toks.join("-")
}

/// Slug d'un texte face aux slugs déjà pris : premier candidat libre de la
/// cascade tronqué → complet → complet+date → complet+date+NOR → ordinal.
/// Hors FR/UE/INTL, le radical est débarrassé de ses marqueurs pays et chaque
/// candidat porte le suffixe alpha-3.
fn pick_slug(src: &SlugSourceRow, taken: &HashSet<String>) -> Result<String> {
    if src.title.trim().is_empty() {
        bail!("texte {} sans titre : aucun slug dérivable", src.text_uid);
    }
    let mut full_raw = lj_extract::legi::slugify_code(&src.title);
    let mut suffix = String::new();
    if !matches!(src.jurisdiction.as_str(), "FR" | "UE" | "INTL") {
        let (a3, tokens) = country(&src.jurisdiction)?;
        full_raw = strip_country_tokens(&full_raw, tokens, &src.jurisdiction);
        if full_raw.is_empty() {
            bail!(
                "texte {} : titre réduit à son seul marqueur pays, aucun radical",
                src.text_uid
            );
        }
        suffix = format!("-{a3}");
    }
    let base = format!("{}{suffix}", truncate_slug(&full_raw));
    let full = format!("{full_raw}{suffix}");
    let mut candidates = vec![base, full.clone()];
    let mut stem = full;
    if let Some(date) = &src.date_texte {
        stem = format!("{stem}-{date}");
        candidates.push(stem.clone());
    }
    if let Some(nor) = &src.nor {
        stem = format!("{stem}-{}", lj_extract::legi::slugify_code(nor));
        candidates.push(stem.clone());
    }
    for cand in candidates {
        if !taken.contains(&cand) {
            return Ok(cand);
        }
    }
    Ok((2..)
        .map(|i| format!("{stem}-{i}"))
        .find(|cand| !taken.contains(cand))
        .expect("les ordinaux finissent par trouver un slug libre"))
}

/// Remplit les `slug NULL` : candidats triés par `text_uid` (déterminisme),
/// cascade [`pick_slug`] contre l'existant et le lot. Renvoie le nombre de
/// slugs posés.
pub async fn assign_text_slugs(repo: &DecisionRepository<'_>) -> Result<u64> {
    let pending = repo
        .texts_without_slug()
        .await
        .map_err(|e| anyhow!("texts_without_slug: {e}"))?;
    if pending.is_empty() {
        return Ok(0);
    }
    let mut taken: HashSet<String> = repo
        .existing_text_slugs()
        .await
        .map_err(|e| anyhow!("existing_text_slugs: {e}"))?
        .into_iter()
        .collect();

    let mut assign: Vec<(String, String)> = Vec::with_capacity(pending.len());
    for src in pending {
        let slug = pick_slug(&src, &taken)?;
        taken.insert(slug.clone());
        assign.push((src.text_uid, slug));
    }

    let mut written = 0u64;
    for chunk in assign.chunks(10_000) {
        written += repo
            .set_text_slugs(chunk)
            .await
            .map_err(|e| anyhow!("set_text_slugs: {e}"))?;
    }
    Ok(written)
}

/// Commande `assign-slugs` : backfill autonome (pool + migrations + passe).
pub async fn assign_slugs() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let n = assign_text_slugs(&repo).await?;
    tracing::info!(written = n, "slugs de textes assignés");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(
        title: &str,
        jurisdiction: &str,
        date: Option<&str>,
        nor: Option<&str>,
    ) -> SlugSourceRow {
        SlugSourceRow {
            text_uid: "t".into(),
            title: title.into(),
            jurisdiction: jurisdiction.into(),
            date_texte: date.map(str::to_string),
            nor: nor.map(str::to_string),
        }
    }

    #[test]
    fn slug_court_intact_long_tronque_sur_tiret() {
        let slug = |title: &str| pick_slug(&src(title, "FR", None, None), &HashSet::new()).unwrap();
        assert_eq!(slug("Code civil"), "code-civil");
        assert_eq!(
            slug("Arrêté du 12 janvier 2012"),
            "arrete-du-12-janvier-2012"
        );
        let long = slug(
            "LOI n° 79-587 du 11 juillet 1979 relative à la motivation des actes \
             administratifs et à l'amélioration des relations entre l'administration \
             et le public",
        );
        assert!(long.len() <= MAX_SLUG_LEN, "tronqué: {long}");
        assert!(!long.ends_with('-'), "frontière de mot: {long}");
        assert!(long.starts_with("loi-n-79-587-du-11-juillet-1979-relative-a-la-motivation"));
    }

    /// Spec ADR 0206 : cascade de désambiguïsation sans uid — slug complet,
    /// puis date, puis NOR, puis ordinal ; jamais l'uid.
    #[test]
    fn cascade_collision_sans_uid() {
        let mut taken = HashSet::new();
        let long_title = "Arrêté du 12 juin 2014 portant désignation de la mission \
             Commerce-exportation-consommation du service du contrôle général \
             économique et financier pour exercer le contrôle budgétaire";

        // Base libre → tronqué.
        let s1 = pick_slug(&src(long_title, "FR", Some("2014-06-12"), None), &taken).unwrap();
        assert!(s1.len() <= MAX_SLUG_LEN);
        taken.insert(s1.clone());

        // Base prise → slug complet non tronqué.
        let s2 = pick_slug(&src(long_title, "FR", Some("2014-06-12"), None), &taken).unwrap();
        assert!(s2.starts_with(&s1));
        assert!(s2.len() > MAX_SLUG_LEN);
        taken.insert(s2.clone());

        // Complet pris → date.
        let s3 = pick_slug(&src(long_title, "FR", Some("2014-06-12"), None), &taken).unwrap();
        assert_eq!(s3, format!("{s2}-2014-06-12"));
        taken.insert(s3.clone());

        // Date prise → NOR.
        let s4 = pick_slug(
            &src(long_title, "FR", Some("2014-06-12"), Some("AGRS1413134A")),
            &taken,
        )
        .unwrap();
        assert_eq!(s4, format!("{s3}-agrs1413134a"));
        taken.insert(s4.clone());

        // Tout pris → ordinal sur le radical le plus riche.
        let s5 = pick_slug(&src(long_title, "FR", Some("2014-06-12"), None), &taken).unwrap();
        assert_eq!(s5, format!("{s3}-2"));
    }

    /// Spec ADR 0206 : juridiction étrangère → radical débarrassé de ses
    /// marqueurs pays (gentilé, nom, alpha-2), identité portée par le seul
    /// suffixe alpha-3 ; code hors table = erreur franche.
    #[test]
    fn juridiction_etrangere_suffixe_alpha3() {
        let taken = HashSet::new();
        assert_eq!(
            pick_slug(&src("Code civil syrien", "SY", None, None), &taken).unwrap(),
            "code-civil-syr"
        );
        assert_eq!(
            pick_slug(
                &src("Code civil (Côte d'Ivoire) — obligations", "CI", None, None),
                &taken
            )
            .unwrap(),
            "code-civil-obligations-civ"
        );
        assert_eq!(
            pick_slug(&src("Constitution belge", "BE", None, None), &taken).unwrap(),
            "constitution-bel"
        );
        assert!(pick_slug(&src("Code civil", "ZZ", None, None), &taken).is_err());
        assert!(
            pick_slug(&src("Syrie", "SY", None, None), &taken).is_err(),
            "titre réduit au pays : erreur franche"
        );
    }
}
