//! Text slug assignment: the only writer of `legal_text.slug`. Every text
//! gets a deterministic, immutable and **readable** slug, never a uid inside.
//! A foreign text is first named by its title before the first comma, where its
//! register reference starts (`code-civil` for « Code civil (Suisse), RS 210 »).
//! Cascade on collision: a wider cut of the title, then the date of the text,
//! then the NOR, then an ordinal, inside the jurisdiction, uniqueness bearing on the
//! pair since the URL names the jurisdiction in its first segment. The stem of
//! a foreign text is stripped of its country markers, which the segment says
//! again (`/norm/be/nouveau-code-civil`). The pass runs at the end of every
//! referential ingest and is exposed as the `assign-slugs` command (backfill).
//! Idempotent: it fills `slug NULL` and nothing else.

use anyhow::{anyhow, bail, Result};
use lj_store::repository::{DecisionRepository, SlugSourceRow};
use std::collections::HashSet;

use crate::config::Settings;

/// Longueur max du slug nu (frontière de mot) — les titres de lois/arrêtés
/// font couramment 150-250 chars, l'URL n'a pas à porter tout l'intitulé.
const MAX_SLUG_LEN: usize = 80;

/// The cut tried when the first is taken, where the whole title would give slugs
/// of over a thousand chars.
const WIDE_SLUG_LEN: usize = 120;

/// Cuts an already formed slug at `max`, on a dash boundary.
fn truncate_slug(full: &str, max: usize) -> String {
    if full.len() <= max {
        return full.to_string();
    }
    match full[..=max].rfind('-') {
        Some(cut) if cut > 0 => full[..cut].to_string(),
        _ => full[..max].to_string(),
    }
}

/// The country of a jurisdiction outside the home corpora (FR/UE/INTL): its
/// lowercase ISO 3166-1 alpha-3 code, as the old suffix carried it, and the
/// stem tokens to strip (demonyms, country name). The country identity lives
/// in the URL segment, never in the slug (`code-civil`, not
/// `code-civil-syrien`). An unknown code is a hard error: extend the table
/// when the text is curated.
pub(super) fn country(a2: &str) -> Result<(&'static str, &'static [&'static str])> {
    Ok(match a2 {
        "AM" => ("arm", &["armenien", "armenienne", "armenie"]),
        "AO" => ("ago", &["angola", "angolais", "angolaise"]),
        "AT" => ("aut", &["autrichien", "autrichienne", "autriche"]),
        "BE" => ("bel", &["belge", "belgique"]),
        "BF" => ("bfa", &["burkinabe", "burkina-faso", "burkina"]),
        "BG" => ("bgr", &["bulgare", "bulgarie"]),
        "BI" => ("bdi", &["burundi", "burundais", "burundaise"]),
        "BJ" => ("ben", &["beninois", "beninoise", "benin"]),
        "CA" => ("can", &["canadien", "canadienne", "canada"]),
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
        "OHADA" => ("ohada", &["ohada"]),
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

/// What a country marker taken off the end of a slug leaves hanging: `loi-sur-la-preuve-au`.
const DANGLING: &[&str] = &[
    "au", "aux", "d", "de", "des", "du", "en", "l", "la", "le", "les",
];

/// Retire du slug les séquences de tokens pays (bornées aux tirets : `belge`,
/// `cote-d-ivoire`) et le token alpha-2 minuscule. Les composés dont un seul
/// token matche (`franco-algerienne`) relèvent de la curation, pas de la passe.
fn strip_country_tokens(slug: &str, tokens: &[&str], a2: &str) -> String {
    let mut toks: Vec<String> = slug.split('-').map(str::to_string).collect();
    let a2_low = a2.to_lowercase();
    let mut seqs: Vec<Vec<&str>> = tokens.iter().map(|s| s.split('-').collect()).collect();
    seqs.push(vec![&a2_low]);
    seqs.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut tail_removed = false;
    for seq in &seqs {
        let mut i = 0;
        while i + seq.len() <= toks.len() {
            if toks[i..i + seq.len()]
                .iter()
                .map(String::as_str)
                .eq(seq.iter().copied())
            {
                tail_removed |= i + seq.len() == toks.len();
                toks.drain(i..i + seq.len());
            } else {
                i += 1;
            }
        }
    }
    if tail_removed {
        while toks.last().is_some_and(|t| DANGLING.contains(&t.as_str())) {
            toks.pop();
        }
    }
    toks.join("-")
}

/// The slug of a text against the `(jurisdiction, slug)` pairs already taken:
/// the first free candidate of the cascade truncated, truncated wider, plus
/// date, plus date plus NOR, then an ordinal. Outside FR/UE/INTL the stem is
/// stripped of its country markers, which the URL segment carries, and the name
/// before the title's first comma is tried first.
pub(super) fn pick_slug(src: &SlugSourceRow, taken: &HashSet<(String, String)>) -> Result<String> {
    let (candidates, stem) = cascade(src, |s| lj_fold::slug(s, '-'))?;
    let libre = |cand: &str| !taken.contains(&(src.jurisdiction.clone(), cand.to_string()));
    if let Some(cand) = candidates.into_iter().find(|c| libre(c)) {
        return Ok(cand);
    }
    Ok((2..)
        .map(|i| format!("{stem}-{i}"))
        .find(|cand| libre(cand))
        .expect("les ordinaux finissent par trouver un slug libre"))
}

/// The date a slug may carry: the date of the text, else its publication
/// date, the DILA setting the 2999-01-01 sentinel on some JORF texts.
fn slug_date(src: &SlugSourceRow) -> Option<&str> {
    [&src.date_texte, &src.date_publi]
        .into_iter()
        .filter_map(|d| d.as_deref())
        .find(|d| ("1500-01-01".."2100-01-01").contains(d))
}

/// The candidates of the cascade in order, and the stem its ordinals extend.
fn cascade(src: &SlugSourceRow, slugify: impl Fn(&str) -> String) -> Result<(Vec<String>, String)> {
    if src.title.trim().is_empty() {
        bail!("texte {} sans titre : aucun slug dérivable", src.text_uid);
    }
    let foreign = !matches!(src.jurisdiction.as_str(), "FR" | "EU" | "INTL");
    let markers = match foreign {
        true => Some(country(&src.jurisdiction)?.1),
        false => None,
    };
    let stem_of = |title: &str| match markers {
        Some(tokens) => strip_country_tokens(&slugify(title), tokens, &src.jurisdiction),
        None => slugify(title),
    };
    let full_raw = stem_of(&src.title);
    if foreign && full_raw.is_empty() {
        bail!(
            "texte {} : titre réduit à son seul marqueur pays, aucun radical",
            src.text_uid
        );
    }
    let wide = truncate_slug(&full_raw, WIDE_SLUG_LEN);
    let mut candidates = vec![truncate_slug(&full_raw, MAX_SLUG_LEN), wide.clone()];
    if let Some((name, _)) = src.title.split_once(',').filter(|_| foreign) {
        let name = truncate_slug(&stem_of(name), MAX_SLUG_LEN);
        if !name.is_empty() {
            candidates.insert(0, name);
        }
    }
    let mut stem = wide;
    if let Some(date) = slug_date(src) {
        stem = format!("{stem}-{date}");
        candidates.push(stem.clone());
    }
    if let Some(nor) = &src.nor {
        stem = format!("{stem}-{}", slugify(nor));
        candidates.push(stem.clone());
    }
    Ok((candidates, stem))
}

/// Remplit les `slug NULL` : candidats triés par `text_uid` (déterminisme),
/// cascade [`pick_slug`] contre l'existant et le lot. Renvoie le nombre de
/// slugs posés.
pub async fn assign_text_slugs(repo: &DecisionRepository<'_>) -> Result<u64> {
    repo.begin_slug_pass()
        .await
        .map_err(|e| anyhow!("begin_slug_pass: {e}"))?;
    let written = slug_pass(repo).await;
    repo.end_slug_pass(written.is_ok())
        .await
        .map_err(|e| anyhow!("end_slug_pass: {e}"))?;
    written
}

async fn slug_pass(repo: &DecisionRepository<'_>) -> Result<u64> {
    let pending = repo
        .texts_without_slug()
        .await
        .map_err(|e| anyhow!("texts_without_slug: {e}"))?;
    if pending.is_empty() {
        return Ok(0);
    }
    let mut taken: HashSet<(String, String)> = repo
        .existing_text_slugs()
        .await
        .map_err(|e| anyhow!("existing_text_slugs: {e}"))?
        .into_iter()
        .collect();

    let mut assign: Vec<(String, String)> = Vec::with_capacity(pending.len());
    for src in pending {
        let slug = pick_slug(&src, &taken)?;
        taken.insert((src.jurisdiction.clone(), slug.clone()));
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

/// Command `drop-country-suffix`: a single pass that strips from the 247
/// foreign slugs the alpha-3 suffix the URL segment now carries, and bequeaths
/// every old address as a redirect. Replays as a no-op, a bare slug having no
/// suffix left to strip. Refuses a collision rather than cascading: a name
/// twice under one jurisdiction is curated by hand.
pub async fn drop_country_suffix() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let mut taken: HashSet<(String, String)> = repo
        .existing_text_slugs()
        .await
        .map_err(|e| anyhow!("existing_text_slugs: {e}"))?
        .into_iter()
        .collect();
    let mut renamed = 0u64;
    let mut kept = 0u64;
    for (text_uid, jurisdiction, ancien) in repo
        .foreign_text_slugs()
        .await
        .map_err(|e| anyhow!("foreign_text_slugs: {e}"))?
    {
        let (a3, _) = country(&jurisdiction)?;
        let Some(neuf) = ancien.strip_suffix(&format!("-{a3}")) else {
            kept += 1;
            continue;
        };
        if neuf.is_empty() {
            bail!("texte {text_uid} : slug réduit à son suffixe pays, rien à garder");
        }
        let cle = (jurisdiction.clone(), neuf.to_string());
        if taken.contains(&cle) {
            bail!("texte {text_uid} : « {neuf} » est déjà pris sous {jurisdiction}");
        }
        repo.rename_text_slug(&text_uid, &ancien, neuf)
            .await
            .map_err(|e| anyhow!("rename_text_slug({text_uid}): {e}"))?;
        taken.remove(&(jurisdiction, ancien));
        taken.insert(cle);
        renamed += 1;
    }
    tracing::info!(
        renamed,
        kept,
        "country suffix removed from the foreign slugs"
    );
    Ok(())
}

/// Whether the cascade can have produced `slug`: one of its candidates, or an
/// ordinal of its stem.
fn minted_by(slug: &str, candidates: &[String], stem: &str) -> bool {
    candidates.iter().any(|c| c == slug)
        || slug
            .strip_prefix(stem)
            .and_then(|t| t.strip_prefix('-'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether every word of `slug` comes from the title, the date or the NOR of
/// the text: a name chosen by hand, as opposed to a slug minted from a title
/// the text no longer bears, or from a badly decoded one. A hand name is no
/// longer than the widest cut.
fn named_from(slug: &str, src: &SlugSourceRow) -> bool {
    if slug.len() > WIDE_SLUG_LEN {
        return false;
    }
    let words: HashSet<String> = [Some(src.title.as_str()), slug_date(src), src.nor.as_deref()]
        .into_iter()
        .flatten()
        .flat_map(|s| {
            lj_fold::slug(s, '-')
                .split('-')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    slug.split('-').all(|w| words.contains(w))
}

/// Command `remint-text-slugs`: renames every slug the cascade no longer
/// mints from its text, each old address kept as a redirect. A slug whose words
/// all come from the text was named by hand and stays. **Dry-run by default.**
/// Replays as a no-op.
pub async fn remint_text_slugs(applique: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let texts = repo
        .texts_with_slug()
        .await
        .map_err(|e| anyhow!("texts_with_slug: {e}"))?;
    let mut taken: HashSet<(String, String)> = repo
        .existing_text_slugs()
        .await
        .map_err(|e| anyhow!("existing_text_slugs: {e}"))?
        .into_iter()
        .collect();
    let redirects: std::collections::HashMap<(String, String), String> = repo
        .text_slug_redirects()
        .await
        .map_err(|e| anyhow!("text_slug_redirects: {e}"))?
        .into_iter()
        .collect();

    let mut bequeathed: std::collections::HashMap<&str, Vec<&str>> =
        std::collections::HashMap::new();
    for ((_, ancien), uid) in &redirects {
        bequeathed.entry(uid).or_default().push(ancien);
    }

    let mut plan: Vec<(String, String, String)> = Vec::new();
    let mut by_hand = 0u64;
    for (src, stored) in &texts {
        let (candidates, stem) = cascade(src, |s| lj_fold::slug(s, '-'))?;
        if minted_by(stored, &candidates, &stem) {
            continue;
        }
        if named_from(stored, src) {
            by_hand += 1;
            continue;
        }
        // A text may take back one of its own old addresses.
        let own: Vec<(String, String)> = bequeathed
            .get(src.text_uid.as_str())
            .into_iter()
            .flatten()
            .map(|ancien| (src.jurisdiction.clone(), ancien.to_string()))
            .collect();
        for key in &own {
            taken.remove(key);
        }
        let neuf = pick_slug(src, &taken)?;
        taken.extend(own);
        taken.insert((src.jurisdiction.clone(), neuf.clone()));
        if redirects
            .get(&(src.jurisdiction.clone(), stored.clone()))
            .is_some_and(|uid| uid != &src.text_uid)
        {
            bail!(
                "text {}: the redirect {stored:?} already answers for another text",
                src.text_uid
            );
        }
        plan.push((src.text_uid.clone(), stored.clone(), neuf));
    }

    for (text_uid, ancien, neuf) in plan.iter().take(20) {
        tracing::info!(text_uid, ancien, neuf, "remint slug");
    }
    tracing::info!(
        renamed = plan.len(),
        by_hand,
        applique,
        "text slugs reminted"
    );
    if !applique {
        return Ok(());
    }
    for (text_uid, ancien, neuf) in &plan {
        repo.rename_text_slug(text_uid, ancien, neuf)
            .await
            .map_err(|e| anyhow!("rename_text_slug({text_uid}): {e}"))?;
    }
    Ok(())
}

/// Commande `assign-slugs` : backfill autonome (pool + migrations + passe).
pub async fn assign_slugs() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let n = assign_text_slugs(&repo).await?;
    tracing::info!(written = n, "text slugs assigned");
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
            date_publi: None,
            nor: nor.map(str::to_string),
        }
    }

    #[test]
    fn un_slug_que_la_cascade_ne_frappe_plus_se_renomme_sauf_le_nom_a_la_main() {
        let texte = src("Aide médicale de l'État", "FR", Some("2005-09-27"), None);
        let (candidates, stem) = cascade(&texte, |s| lj_fold::slug(s, '-')).unwrap();
        assert!(minted_by("aide-medicale-de-l-etat", &candidates, &stem));
        assert!(minted_by(
            "aide-medicale-de-l-etat-2005-09-27-3",
            &candidates,
            &stem
        ));
        assert!(!minted_by("aide-medicale-de-l-tat", &candidates, &stem));
        assert!(!named_from("aide-medicale-de-l-tat", &texte));
        assert!(!minted_by("aide-medicale-etat", &candidates, &stem));
        assert!(named_from("aide-medicale-etat", &texte));
    }

    #[test]
    fn la_date_sentinelle_cede_a_la_date_de_publication() {
        let mut texte = src(
            "Avis de vacance",
            "FR",
            Some("2999-01-01"),
            Some("ECOX9900001V"),
        );
        let taken = HashSet::from([("FR".to_string(), "avis-de-vacance".to_string())]);
        assert_eq!(
            pick_slug(&texte, &taken).unwrap(),
            "avis-de-vacance-ecox9900001v"
        );
        assert!(!named_from("avis-de-vacance-2999-01-01", &texte));
        texte.date_publi = Some("2024-03-15".into());
        assert_eq!(
            pick_slug(&texte, &taken).unwrap(),
            "avis-de-vacance-2024-03-15"
        );
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

    #[test]
    fn cascade_collision_sans_uid() {
        let mut taken = HashSet::new();
        let long_title = "Arrêté du 12 juin 2014 portant désignation de la mission \
             Commerce-exportation-consommation du service du contrôle général \
             économique et financier pour exercer le contrôle budgétaire";

        // Base libre → tronqué.
        let s1 = pick_slug(&src(long_title, "FR", Some("2014-06-12"), None), &taken).unwrap();
        assert!(s1.len() <= MAX_SLUG_LEN);
        taken.insert(("FR".to_string(), s1.clone()));

        // Base prise → coupe plus large.
        let s2 = pick_slug(&src(long_title, "FR", Some("2014-06-12"), None), &taken).unwrap();
        assert!(s2.starts_with(&s1));
        assert!(s2.len() > MAX_SLUG_LEN && s2.len() <= WIDE_SLUG_LEN);
        taken.insert(("FR".to_string(), s2.clone()));

        // Coupe large prise → date.
        let s3 = pick_slug(&src(long_title, "FR", Some("2014-06-12"), None), &taken).unwrap();
        assert_eq!(s3, format!("{s2}-2014-06-12"));
        taken.insert(("FR".to_string(), s3.clone()));

        // Date prise → NOR.
        let s4 = pick_slug(
            &src(long_title, "FR", Some("2014-06-12"), Some("AGRS1413134A")),
            &taken,
        )
        .unwrap();
        assert_eq!(s4, format!("{s3}-agrs1413134a"));
        taken.insert(("FR".to_string(), s4.clone()));

        // Tout pris → ordinal sur le radical le plus riche.
        let s5 = pick_slug(&src(long_title, "FR", Some("2014-06-12"), None), &taken).unwrap();
        assert_eq!(s5, format!("{s3}-2"));
    }

    #[test]
    fn juridiction_etrangere_perd_ses_marqueurs_pays() {
        let taken = HashSet::new();
        assert_eq!(
            pick_slug(&src("Code civil syrien", "SY", None, None), &taken).unwrap(),
            "code-civil"
        );
        assert_eq!(
            pick_slug(
                &src("Code civil (Côte d'Ivoire) — obligations", "CI", None, None),
                &taken
            )
            .unwrap(),
            "code-civil-obligations"
        );
        assert_eq!(
            pick_slug(&src("Constitution belge", "BE", None, None), &taken).unwrap(),
            "constitution"
        );
        assert!(pick_slug(&src("Code civil", "ZZ", None, None), &taken).is_err());
        assert!(
            pick_slug(&src("Syrie", "SY", None, None), &taken).is_err(),
            "titre réduit au pays : erreur franche"
        );
    }

    /// The same name lives under two jurisdictions: the pair carries the
    /// identity, and many countries have a civil code.
    #[test]
    fn le_meme_nom_vit_sous_deux_juridictions() {
        let mut taken = HashSet::new();
        let be = pick_slug(&src("Code civil belge", "BE", None, None), &taken).unwrap();
        taken.insert(("BE".to_string(), be.clone()));
        let ch = pick_slug(&src("Code civil suisse", "CH", None, None), &taken).unwrap();
        assert_eq!((be.as_str(), ch.as_str()), ("code-civil", "code-civil"));
        taken.insert(("CH".to_string(), ch));
        let deuxieme = pick_slug(&src("Code civil belge", "BE", None, None), &taken).unwrap();
        assert_eq!(deuxieme, "code-civil-2");
    }
}
