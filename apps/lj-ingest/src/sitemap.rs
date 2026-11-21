//! Génération des sitemaps.
//!
//! Pipeline :
//!
//! 1. [`build_sitemaps`] walks `decisions` in pages of [`MAX_URLS_PER_SITEMAP`]
//!    URLs and builds **in memory** one `sitemap-{n}.xml.gz` per page, plus a
//!    `sitemap-index.xml` naming them by their final public URL.
//! 2. Côté appelant (`cmd_sitemap`), l'ensemble est upserté dans la table
//!    Postgres `sitemaps`
//!    ([`lj_store::repository::DecisionRepository::replace_sitemaps`]).
//!
//! Côté serveur HTTP, `lj-server` sert `/sitemap.xml` (ligne
//! `sitemap-index.xml`) et `/sitemaps/{file}` depuis cette table.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use flate2::{Compression, GzBuilder};

/// Sitemaps protocol cap on the URLs of one sitemap.
pub const MAX_URLS_PER_SITEMAP: usize = 50_000;

/// Origine publique des URLs ; en dur — un site = une origine canonique.
pub const BASE_URL: &str = "https://librejustice.fr";

/// Nom de la ligne index en base ; `lj-server` la sert sous `/sitemap.xml`.
pub const SITEMAP_INDEX_NAME: &str = "sitemap-index.xml";

/// Un fichier sitemap construit en mémoire, prêt à upserter en base.
///
/// `body` est servi tel quel : `.xml.gz` = **fichier gzip** (`application/gzip`
/// sans `Content-Encoding`), `.xml` = `application/xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapFile {
    pub filename: String,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub lastmod: NaiveDate,
}

/// Date de dernière modification telle que la source la connaît : `None` quand
/// elle l'ignore. La source **n'invente jamais** de date de remplacement — c'est
/// [`plausible_lastmod`] qui tranche ce qui est publiable.
pub type SourceLastmod = Option<NaiveDate>;

/// Sortie de [`SitemapSource::iter_annonces_for_sitemap`] : fiches
/// `(uid, lastmod)` et codes département de leurs pages de catalogue.
pub type AnnoncesSitemap = (Vec<(String, SourceLastmod)>, Vec<String>);

/// Hubs de crawl : par (segment de juridiction, clé de hub), les buckets
/// `(année, nombre de pages de listing)`, `i32` côté juridictions, token
/// d'URL côté normes.
pub type JurisdictionHubPages = Vec<(String, String, Vec<(i32, i64)>)>;
pub type NormHubPages = Vec<(String, String, Vec<(String, i64)>)>;

pub trait SitemapSource {
    /// Iterates `(path, lastmod)` over every decision to publish
    /// (`/decision/{juris}/{id}`).
    fn iter_decisions_for_sitemap(&self) -> Result<Vec<(String, SourceLastmod)>>;

    /// Itère `(slug, num, juridiction, lastmod)` pour les articles de référentiel
    /// à publier (pages `/norm/{juris}/{slug}/{num}`).
    fn iter_referential_for_sitemap(&self) -> Result<Vec<(String, String, String, SourceLastmod)>>;

    /// Itère `(ns, id, lastmod)` pour les entités de l'annuaire à publier
    /// (pages `/entity/{juris}/{ns}/{id}`). Les avocats (personnes physiques) sont exclus
    /// côté SQL (RGPD).
    fn iter_entities_for_sitemap(&self) -> Result<Vec<(String, String, SourceLastmod)>>;

    /// Itère `(slug, juridiction, lastmod)` pour les pages TDM.
    fn iter_texts_for_sitemap(&self) -> Result<Vec<(String, String, SourceLastmod)>>;

    /// Itère `(juris, code, [(année, nombre de pages)])` pour les hubs
    /// juridiction (pages `/decision-index/{juris}/{code}` +
    /// `/decision-index/{juris}/{code}/{year}?page=N`).
    fn iter_jurisdiction_hubs_for_sitemap(&self) -> Result<JurisdictionHubPages>;

    /// Itère `(juris, fond, [(token année, nombre de pages)])` pour les hubs du
    /// catalogue des normes (pages `/norm-index/{juris}/{fond}` +
    /// `/norm-index/{juris}/{fond}/{year}?page=N`), `year` = année ou
    /// `sans-date`.
    fn iter_norm_hubs_for_sitemap(&self) -> Result<NormHubPages>;

    /// Itère `(uid, lastmod)` pour les fiches de consultation (`/notice/fr/{uid}`)
    /// et `(code département)` pour leurs pages de catalogue
    /// (`/journal-index/fr/{code}`).
    fn iter_annonces_for_sitemap(&self) -> Result<AnnoncesSitemap>;
}

/// Indexable static pages, served by SSR but absent from the tables (so not
/// enumerable from the base): landing, search, directory hubs, legal notice…
/// Paths relative to [`BASE_URL`]; their `lastmod` is the run date (no per page
/// freshness signal). The hub `/entity-index/fr/avocats` is **deliberately
/// absent**: it pages through lawyers (natural persons), the same SEO exclusion
/// as their profiles.
const STATIC_PATHS: &[&str] = &[
    "/fr",
    "/fr/decision-search",
    "/fr/norm-search",
    "/sources",
    "/decision-index/fr",
    "/decision-index/eu",
    "/decision-index/intl",
    "/fr/journal-search",
    "/journal-index/fr",
    "/norm-index/fr/codes",
    "/norm-index/fr/in-force",
    "/entity-search",
    "/entity-index/fr/entreprises",
    "/entity-index/fr/personnes-publiques",
    "/entity-index/fr/associations",
    "/entity-index/fr/cabinets",
    "/mcp-guide",
    "/legal-notice",
    "/privacy",
    "/terms",
    "/open-source",
];

fn decision_url(path: &str) -> String {
    format!("{BASE_URL}{path}")
}

fn law_url(juris: &str, code: &str, num: &str) -> String {
    format!("{BASE_URL}{}/{num}", lj_vocab::norm_path(juris, code))
}

fn entity_url(ns: &str, id: &str) -> String {
    format!("{BASE_URL}{}", lj_vocab::entity_path(ns, id))
}

fn text_url(juris: &str, slug: &str) -> String {
    format!("{BASE_URL}{}", lj_vocab::norm_path(juris, slug))
}

fn static_url(path: &str) -> String {
    format!("{BASE_URL}{path}")
}

/// Une `loc` par page d'un listing paginé : `base` nue pour la première,
/// `?page=N` ensuite — la forme que le hub rend canonique. Un `pages` nul ou
/// négatif ne rend que la première.
fn paginated(base: &str, pages: i64) -> Vec<String> {
    let mut out = vec![base.to_string()];
    for n in 2..=pages {
        out.push(format!("{base}?page={n}"));
    }
    out
}

fn sitemap_url(filename: &str) -> String {
    format!("{BASE_URL}/sitemaps/{filename}")
}

/// Échappe `&`, `<`, `>`.
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Plus ancienne date de modification publiable : **l'epoch Unix**, plancher
/// imposé par le parseur de Google. Search Console rejette en « Date non
/// valide » tout `lastmod` antérieur.
///
/// Sous ce plancher tombent aussi les artefacts de source : DILA écrit
/// `0001-01-01` quand un début de vigueur est inconnu, et le corpus porte des
/// dates aberrantes (`0015-01-01`, `0025-12-01`).
const LASTMOD_FLOOR: NaiveDate = match NaiveDate::from_ymd_opt(1970, 1, 1) {
    Some(d) => d,
    None => unreachable!(),
};

/// **Seul** juge de ce qui part en `<lastmod>`. Une date hors de
/// `[LASTMOD_FLOOR, today]` est une sentinelle ou une aberration de la source,
/// pas un signal de fraîcheur : on l'écarte, sans la remplacer par une date
/// inventée.
///
/// `<lastmod>` est optionnel au schéma sitemaps.org : l'absence est la seule
/// réponse honnête quand la source ne sait pas.
fn plausible_lastmod(lastmod: SourceLastmod, today: NaiveDate) -> Option<NaiveDate> {
    lastmod.filter(|d| (LASTMOD_FLOOR..=today).contains(d))
}

/// Bloc `<url>` minimal — pas de `priority`/`changefreq` (ignorés par Google
/// depuis 2023). `loc` est échappé ici ; les appelants passent l'URL brute.
/// `lastmod` absent → tag omis (cf. [`plausible_lastmod`]).
fn format_url_entry(loc: &str, lastmod: Option<NaiveDate>) -> String {
    match lastmod {
        Some(d) => format!(
            "<url><loc>{}</loc><lastmod>{}</lastmod></url>",
            xml_escape(loc),
            d.format("%Y-%m-%d"),
        ),
        None => format!("<url><loc>{}</loc></url>", xml_escape(loc)),
    }
}

/// Taille maximale d'un sitemap **décompressé** (limite sitemaps.org).
const MAX_UNCOMPRESSED_BYTES: usize = 50 * 1024 * 1024;

/// Longueur maximale d'une URL dans un sitemap (limite Google).
const MAX_LOC_LEN: usize = 2048;

/// Vérifie que l'artefact produit respecte le contrat sitemaps.org + Google,
/// **en relisant les octets qui seront servis** (gzip décompressé), et non les
/// structures qui les ont produits.
///
/// Toute violation est une erreur franche : `cmd_sitemap` s'arrête avant
/// `replace_sitemaps`, donc **les sitemaps valides du run précédent restent
/// servis**.
///
/// Les problèmes sont accumulés puis rapportés ensemble.
fn validate_sitemaps(files: &[SitemapFile], today: NaiveDate) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    let mut locs_seen: HashMap<String, usize> = HashMap::new();
    let produced: HashSet<&str> = files
        .iter()
        .filter(|f| f.content_type == "application/gzip")
        .map(|f| f.filename.as_str())
        .collect();

    for f in files {
        let xml = decoded_body(f)?;

        if f.content_type == "application/xml" {
            // Ligne d'index : doit référencer exactement les subs produits.
            let listed: HashSet<&str> = tag_values(&xml, "loc")
                .into_iter()
                .filter_map(|l| l.rsplit('/').next())
                .collect();
            let missing: Vec<&&str> = produced.iter().filter(|p| !listed.contains(**p)).collect();
            let extra: Vec<&&str> = listed.iter().filter(|l| !produced.contains(**l)).collect();
            if !missing.is_empty() || !extra.is_empty() {
                problems.push(format!(
                    "index incohérent : {} sub(s) produit(s) non référencé(s) {:?}, \
                     {} référence(s) sans fichier {:?}",
                    missing.len(),
                    missing.iter().take(3).collect::<Vec<_>>(),
                    extra.len(),
                    extra.iter().take(3).collect::<Vec<_>>(),
                ));
            }
            continue;
        }

        if xml.len() > MAX_UNCOMPRESSED_BYTES {
            problems.push(format!(
                "{} : {} octets décompressés (max {})",
                f.filename,
                xml.len(),
                MAX_UNCOMPRESSED_BYTES
            ));
        }
        let url_count = xml.matches("<url>").count();
        if url_count > MAX_URLS_PER_SITEMAP {
            problems.push(format!(
                "{} : {url_count} URLs (max {MAX_URLS_PER_SITEMAP})",
                f.filename
            ));
        }

        for loc in tag_values(&xml, "loc") {
            *locs_seen.entry(loc.to_string()).or_default() += 1;
            if !loc.starts_with(BASE_URL) {
                problems.push(format!(
                    "{} : loc hors origine canonique — {loc}",
                    f.filename
                ));
            }
            if loc.len() > MAX_LOC_LEN {
                problems.push(format!(
                    "{} : loc de {} caractères (max {MAX_LOC_LEN}) — {loc}",
                    f.filename,
                    loc.len()
                ));
            }
            if loc.contains('<') || loc.contains('>') || unescaped_amp(loc) {
                problems.push(format!("{} : loc mal échappée — {loc}", f.filename));
            }
        }

        for lastmod in tag_values(&xml, "lastmod") {
            match NaiveDate::parse_from_str(lastmod, "%Y-%m-%d") {
                Ok(d) if plausible_lastmod(Some(d), today).is_some() => {}
                Ok(d) => problems.push(format!(
                    "{} : lastmod hors fenêtre publiable [{LASTMOD_FLOOR}, {today}] — {d}",
                    f.filename
                )),
                Err(_) => problems.push(format!(
                    "{} : lastmod non ISO YYYY-MM-DD — {lastmod}",
                    f.filename
                )),
            }
        }
    }

    let dups: Vec<(&String, &usize)> = locs_seen.iter().filter(|(_, n)| **n > 1).collect();
    if !dups.is_empty() {
        let repeated: usize = dups.iter().map(|(_, n)| **n - 1).sum();
        problems.push(format!(
            "{} URL(s) dupliquée(s) sur l'ensemble des sitemaps, {repeated} entrée(s) en trop \
             — ex. {:?} : la source n'est pas au grain de l'URL",
            dups.len(),
            dups.iter().take(3).map(|(l, n)| (l, n)).collect::<Vec<_>>(),
        ));
    }

    if !problems.is_empty() {
        // Cap l'affichage du log ; le total reste donné.
        let shown: Vec<&String> = problems.iter().take(10).collect();
        bail!(
            "sitemaps hors contrat, publication refusée — {} problème(s) :\n  {}{}",
            problems.len(),
            shown
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join("\n  "),
            if problems.len() > shown.len() {
                format!("\n  … et {} autre(s)", problems.len() - shown.len())
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

/// Corps d'un sitemap en texte — gunzip pour les `.xml.gz`, tel quel sinon.
fn decoded_body(f: &SitemapFile) -> Result<String> {
    if f.content_type == "application/gzip" {
        let mut out = String::new();
        flate2::read::GzDecoder::new(&f.body[..])
            .read_to_string(&mut out)
            .with_context(|| format!("sitemap: gunzip {}", f.filename))?;
        Ok(out)
    } else {
        String::from_utf8(f.body.clone())
            .with_context(|| format!("sitemap: {} n'est pas de l'UTF-8", f.filename))
    }
}

/// Valeurs de toutes les occurrences de `<tag>…</tag>`. Le format est celui
/// qu'on écrit nous-mêmes (pas de retour à la ligne, pas d'attribut) : un scan
/// suffit.
fn tag_values<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        match after.find(&close) {
            Some(end) => {
                out.push(&after[..end]);
                rest = &after[end + close.len()..];
            }
            None => break,
        }
    }
    out
}

/// `&` non suivi d'une entité — le signe d'un `xml_escape` oublié.
fn unescaped_amp(value: &str) -> bool {
    value.match_indices('&').any(|(i, _)| {
        !["amp;", "lt;", "gt;", "quot;", "apos;"]
            .iter()
            .any(|e| value[i + 1..].starts_with(e))
    })
}

/// gzip avec `mtime=0` pour des bytes reproductibles entre runs.
fn gzip_bytes(payload: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    encoder.write_all(payload).context("sitemap: gzip write")?;
    encoder.finish().context("sitemap: gzip finish")
}

/// Construit `sitemap-{section}-{n}.xml.gz` + `sitemap-index.xml` en mémoire.
///
/// Les fichiers sont nommés **par fond** (`pages`, `juridictions`, `textes`,
/// `entites`, `decisions`) : Search Console expose alors l'indexation par fond
/// au lieu d'une numérotation opaque. Retourne la liste des fichiers (index en
/// dernier). L'index réfère les sub-sitemaps via leur URL publique finale, comme
/// l'exige sitemaps.org § sitemap index (URLs absolues).
pub fn build_sitemaps<S: SitemapSource>(repo: &S, today: NaiveDate) -> Result<Vec<SitemapFile>> {
    let mut files: Vec<SitemapFile> = Vec::new();

    // Pages statiques indexables (landing, hubs annuaire…), `lastmod` = run.
    let mut pages = SectionWriter::new("pages", today);
    for path in STATIC_PATHS {
        pages.push(&static_url(path), Some(today))?;
    }
    files.extend(pages.finish()?);

    // Hubs juridiction `/decision-index/{juris}/{code}` + `…/{year}` ,
    // `lastmod` = run : leurs listes bougent à l'ingest quotidien.
    let mut hubs = SectionWriter::new("juridictions", today);
    for (juris, code, years) in repo.iter_jurisdiction_hubs_for_sitemap()? {
        let hub = format!("{BASE_URL}/decision-index/{juris}/{code}");
        hubs.push(&hub, Some(today))?;
        for (year, pages) in years {
            for loc in paginated(&format!("{hub}/{year}"), pages) {
                hubs.push(&loc, Some(today))?;
            }
        }
    }
    files.extend(hubs.finish()?);

    // Hubs du catalogue des normes `/norm-index/{juris}` + `…/{fond}` +
    // `…/{fond}/{year}` (`year` = année ou `sans-date`), même profil.
    let mut normes = SectionWriter::new("normes", today);
    let mut last_juris: Option<String> = None;
    for (juris, fond, tokens) in repo.iter_norm_hubs_for_sitemap()? {
        let index = format!("{BASE_URL}/norm-index/{juris}");
        if last_juris.as_deref() != Some(juris.as_str()) {
            normes.push(&index, Some(today))?;
            last_juris = Some(juris);
        }
        let hub = format!("{index}/{fond}");
        normes.push(&hub, Some(today))?;
        for (token, pages) in tokens {
            for loc in paginated(&format!("{hub}/{token}"), pages) {
                normes.push(&loc, Some(today))?;
            }
        }
    }
    files.extend(normes.finish()?);

    // Annonces : catalogue départemental + fiches de consultation. `lastmod` =
    // date de PARUTION de l'avis, seule date stable de la ligne : la dernière vue
    // d'un canal est réécrite à chaque moisson.
    let (annonces, departements) = repo.iter_annonces_for_sitemap()?;
    let mut section = SectionWriter::new("annonces", today);
    for code in departements {
        section.push(&format!("{BASE_URL}/journal-index/fr/{code}"), Some(today))?;
    }
    for (uid, lastmod) in annonces {
        section.push(
            &format!("{BASE_URL}{}", lj_vocab::notice_path("FR", &uid)),
            lastmod,
        )?;
    }
    files.extend(section.finish()?);

    // Textes : pages TDM et articles, sous le segment de la juridiction.
    let mut textes = SectionWriter::new("textes", today);
    for (slug, juris, lastmod) in repo.iter_texts_for_sitemap()? {
        textes.push(&text_url(&juris, &slug), lastmod)?;
    }
    for (slug, num, juris, lastmod) in repo.iter_referential_for_sitemap()? {
        textes.push(&law_url(&juris, &slug, &num), lastmod)?;
    }
    files.extend(textes.finish()?);

    // Fiches d'entités de l'annuaire `/entity/{juris}/{ns}/{id}` (avocats exclus côté
    // SQL).
    let mut entites = SectionWriter::new("entites", today);
    for (ns, id, lastmod) in repo.iter_entities_for_sitemap()? {
        entites.push(&entity_url(&ns, &id), lastmod)?;
    }
    files.extend(entites.finish()?);

    let mut decisions = SectionWriter::new("decisions", today);
    for (path, lastmod) in repo.iter_decisions_for_sitemap()? {
        decisions.push(&decision_url(&path), lastmod)?;
    }
    files.extend(decisions.finish()?);

    // Sitemap index — URLs publiques absolues (Google refuse le relatif).
    let mut index_entries = String::new();
    let mut index_lastmod: Option<NaiveDate> = None;
    for f in &files {
        index_entries.push_str(&format!(
            "<sitemap><loc>{}</loc><lastmod>{}</lastmod></sitemap>",
            xml_escape(&sitemap_url(&f.filename)),
            f.lastmod.format("%Y-%m-%d"),
        ));
        if index_lastmod.is_none_or(|m| f.lastmod > m) {
            index_lastmod = Some(f.lastmod);
        }
    }
    let index_body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{index_entries}</sitemapindex>"
    );

    files.push(SitemapFile {
        filename: SITEMAP_INDEX_NAME.to_string(),
        content_type: "application/xml",
        body: index_body.into_bytes(),
        // Index vide (corpus vide) → date neutre ; sinon le max des subs.
        lastmod: index_lastmod.unwrap_or(LASTMOD_FLOOR),
    });

    // Personne ne publie sans passer par ici.
    validate_sitemaps(&files, today)?;
    Ok(files)
}

/// Accumule les blocs `<url>` d'une **section** (un fond : `pages`,
/// `decisions`…) et découpe en fichiers `sitemap-{section}-{n}.xml.gz` de
/// [`MAX_URLS_PER_SITEMAP`] au plus. Tient le max `lastmod` par fichier
/// (= `lastmod` de la ligne d'index correspondante, permet à Google de ne
/// re-crawler que les subs modifiés).
struct SectionWriter {
    section: &'static str,
    today: NaiveDate,
    files: Vec<SitemapFile>,
    entries: Vec<String>,
    max_lastmod: Option<NaiveDate>,
}

impl SectionWriter {
    fn new(section: &'static str, today: NaiveDate) -> Self {
        Self {
            section,
            today,
            files: Vec::new(),
            entries: Vec::new(),
            max_lastmod: None,
        }
    }

    fn push(&mut self, loc: &str, lastmod: SourceLastmod) -> Result<()> {
        let lastmod = plausible_lastmod(lastmod, self.today);
        self.entries.push(format_url_entry(loc, lastmod));
        if let Some(d) = lastmod {
            if self.max_lastmod.is_none_or(|m| d > m) {
                self.max_lastmod = Some(d);
            }
        }
        if self.entries.len() >= MAX_URLS_PER_SITEMAP {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.entries.is_empty() {
            return Ok(());
        }
        // `lastmod` du fichier = max des dates publiables qu'il contient. Aucune
        // (section entière à dates inconnues) → la date du run.
        let max_lastmod = self.max_lastmod.take().unwrap_or(self.today);
        let filename = format!("sitemap-{}-{}.xml.gz", self.section, self.files.len() + 1);
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{}</urlset>",
            self.entries.concat()
        );
        self.files.push(SitemapFile {
            filename,
            content_type: "application/gzip",
            body: gzip_bytes(body.as_bytes())?,
            lastmod: max_lastmod,
        });
        self.entries.clear();
        Ok(())
    }

    /// Flushe le reliquat et rend les fichiers de la section (vide si la
    /// section n'a reçu aucune URL).
    fn finish(mut self) -> Result<Vec<SitemapFile>> {
        self.flush()?;
        Ok(self.files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[derive(Default)]
    struct FakeSource {
        decisions: Vec<(String, SourceLastmod)>,
        referential: Vec<(String, String, String, SourceLastmod)>,
        entities: Vec<(String, String, SourceLastmod)>,
        texts: Vec<(String, String, SourceLastmod)>,
        jurisdiction_hubs: JurisdictionHubPages,
        norm_hubs: NormHubPages,
        annonces: (Vec<(String, SourceLastmod)>, Vec<String>),
    }
    impl SitemapSource for FakeSource {
        fn iter_decisions_for_sitemap(&self) -> Result<Vec<(String, SourceLastmod)>> {
            Ok(self.decisions.clone())
        }
        fn iter_referential_for_sitemap(
            &self,
        ) -> Result<Vec<(String, String, String, SourceLastmod)>> {
            Ok(self.referential.clone())
        }
        fn iter_entities_for_sitemap(&self) -> Result<Vec<(String, String, SourceLastmod)>> {
            Ok(self.entities.clone())
        }
        fn iter_texts_for_sitemap(&self) -> Result<Vec<(String, String, SourceLastmod)>> {
            Ok(self.texts.clone())
        }
        fn iter_jurisdiction_hubs_for_sitemap(&self) -> Result<JurisdictionHubPages> {
            Ok(self.jurisdiction_hubs.clone())
        }
        fn iter_norm_hubs_for_sitemap(&self) -> Result<NormHubPages> {
            Ok(self.norm_hubs.clone())
        }
        fn iter_annonces_for_sitemap(&self) -> Result<AnnoncesSitemap> {
            Ok(self.annonces.clone())
        }
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn url_entry_format() {
        assert_eq!(
            format_url_entry(&decision_url("/decision/fr/ce_42"), Some(d("2026-05-29"))),
            "<url><loc>https://librejustice.fr/decision/fr/ce_42</loc>\
             <lastmod>2026-05-29</lastmod></url>"
        );
    }

    #[test]
    fn lastmod_hors_fenetre_est_ecarte() {
        let today = d("2026-07-31");
        assert_eq!(
            plausible_lastmod(Some(d("1970-01-01")), today),
            Some(d("1970-01-01"))
        );
        assert_eq!(
            plausible_lastmod(Some(d("2026-07-29")), today),
            Some(d("2026-07-29"))
        );
        assert_eq!(plausible_lastmod(Some(today), today), Some(today));
        assert_eq!(plausible_lastmod(Some(d("1969-12-31")), today), None);
        assert_eq!(plausible_lastmod(Some(d("1848-02-24")), today), None);
        assert_eq!(plausible_lastmod(Some(d("0001-01-01")), today), None);
        assert_eq!(plausible_lastmod(Some(d("0015-01-01")), today), None);
        assert_eq!(plausible_lastmod(Some(d("2999-01-01")), today), None);
        assert_eq!(plausible_lastmod(Some(d("5489-12-30")), today), None);
        assert_eq!(plausible_lastmod(Some(d("2026-08-01")), today), None);
        assert_eq!(plausible_lastmod(None, today), None);
    }

    #[test]
    fn url_entry_sans_lastmod_omet_le_tag() {
        assert_eq!(
            format_url_entry(&decision_url("/decision/fr/ce_42"), None),
            "<url><loc>https://librejustice.fr/decision/fr/ce_42</loc></url>"
        );
    }

    #[test]
    fn sentinelle_source_nexporte_aucun_lastmod() {
        let source = FakeSource {
            referential: vec![
                (
                    "code-civil".into(),
                    "1240".into(),
                    "FR".into(),
                    Some(d("0001-01-01")),
                ),
                (
                    "code-civil".into(),
                    "1241".into(),
                    "FR".into(),
                    Some(d("2026-02-02")),
                ),
                ("code-civil".into(), "1242".into(), "FR".into(), None),
            ],
            texts: vec![("code-penal".into(), "FR".into(), Some(d("2999-01-01")))],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-07-31")).unwrap();
        let body = decompress_subs(&files);

        assert!(!body.contains("0001-01-01"));
        assert!(!body.contains("2999-01-01"));
        assert!(
            body.contains("<url><loc>https://librejustice.fr/norm/fr/code-civil/1240</loc></url>")
        );
        assert!(
            body.contains("<url><loc>https://librejustice.fr/norm/fr/code-civil/1242</loc></url>")
        );
        assert!(body.contains("<url><loc>https://librejustice.fr/norm/fr/code-penal</loc></url>"));
        assert!(body.contains(
            "<url><loc>https://librejustice.fr/norm/fr/code-civil/1241</loc>\
             <lastmod>2026-02-02</lastmod></url>"
        ));
        let textes = files
            .iter()
            .find(|f| f.filename == "sitemap-textes-1.xml.gz")
            .expect("section textes");
        assert_eq!(textes.lastmod, d("2026-02-02"));
    }

    #[test]
    fn section_sans_date_publiable_est_datee_du_run() {
        let source = FakeSource {
            decisions: vec![
                ("/decision/fr/a".into(), None),
                ("/decision/fr/b".into(), Some(d("0001-01-01"))),
            ],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-07-31")).unwrap();
        let decisions = files
            .iter()
            .find(|f| f.filename == "sitemap-decisions-1.xml.gz")
            .expect("section decisions");
        assert_eq!(decisions.lastmod, d("2026-07-31"));
    }

    /// Fabrique un sub-sitemap à partir de blocs `<url>` bruts — un artefact que
    /// les writers ne produiraient pas.
    fn forged_sub(entries: &str) -> SitemapFile {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{entries}</urlset>"
        );
        SitemapFile {
            filename: "sitemap-decisions-1.xml.gz".to_string(),
            content_type: "application/gzip",
            body: gzip_bytes(body.as_bytes()).unwrap(),
            lastmod: d("2026-08-01"),
        }
    }

    #[test]
    fn valide_un_artefact_conforme() {
        let source = FakeSource {
            decisions: vec![("/decision/fr/a".into(), Some(d("2026-01-01")))],
            referential: vec![("code-civil".into(), "1240".into(), "FR".into(), None)],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-08-01")).unwrap();
        validate_sitemaps(&files, d("2026-08-01")).unwrap();
    }

    #[test]
    fn refuse_une_loc_dupliquee() {
        let source = FakeSource {
            decisions: vec![
                ("/decision/fr/a".into(), Some(d("2026-01-01"))),
                ("/decision/fr/a".into(), Some(d("2026-03-15"))),
            ],
            ..Default::default()
        };
        let err = build_sitemaps(&source, d("2026-08-01"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("dupliquée"), "message inattendu : {err}");
        assert!(err.contains("grain de l'URL"), "message inattendu : {err}");
    }

    #[test]
    fn refuse_un_lastmod_hors_contrat() {
        let pre_epoch = forged_sub(
            "<url><loc>https://librejustice.fr/decision/fr/a</loc>\
             <lastmod>1969-12-31</lastmod></url>",
        );
        let err = validate_sitemaps(&[pre_epoch], d("2026-08-01"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("hors fenêtre publiable"), "{err}");

        let mal_forme = forged_sub(
            "<url><loc>https://librejustice.fr/decision/fr/a</loc>\
             <lastmod>01/08/2026</lastmod></url>",
        );
        let err = validate_sitemaps(&[mal_forme], d("2026-08-01"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("non ISO"), "{err}");
    }

    #[test]
    fn refuse_une_loc_invalide() {
        let hors_origine = forged_sub("<url><loc>https://exemple.fr/decision/fr/a</loc></url>");
        let err = validate_sitemaps(&[hors_origine], d("2026-08-01"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("hors origine canonique"), "{err}");

        let mal_echappee = forged_sub("<url><loc>https://librejustice.fr/norm/a&b/1</loc></url>");
        let err = validate_sitemaps(&[mal_echappee], d("2026-08-01"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("mal échappée"), "{err}");
    }

    #[test]
    fn refuse_un_index_incoherent() {
        let sub = forged_sub("<url><loc>https://librejustice.fr/decision/fr/a</loc></url>");
        let index = SitemapFile {
            filename: SITEMAP_INDEX_NAME.to_string(),
            content_type: "application/xml",
            body: format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><sitemapindex>\
                 <sitemap><loc>{}</loc></sitemap></sitemapindex>",
                sitemap_url("sitemap-decisions-99.xml.gz")
            )
            .into_bytes(),
            lastmod: d("2026-08-01"),
        };
        let err = validate_sitemaps(&[sub, index], d("2026-08-01"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("index incohérent"), "{err}");
    }

    #[test]
    fn law_url_format() {
        assert_eq!(
            law_url("FR", "code-civil", "1240"),
            "https://librejustice.fr/norm/fr/code-civil/1240"
        );
    }

    #[test]
    fn escapes_only_xml_specials() {
        assert_eq!(xml_escape("a&b<c>d\"e"), "a&amp;b&lt;c&gt;d\"e");
    }

    #[test]
    fn gzip_reproducible() {
        let a = gzip_bytes(b"hello world").unwrap();
        let b = gzip_bytes(b"hello world").unwrap();
        assert_eq!(a, b);
        assert_eq!(&a[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn builds_index_and_subs() {
        let source = FakeSource {
            decisions: vec![
                ("/decision/fr/a".into(), Some(d("2026-01-01"))),
                ("/decision/fr/b".into(), Some(d("2026-03-15"))),
            ],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-05-01")).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].filename, "sitemap-pages-1.xml.gz");
        assert_eq!(files[1].filename, "sitemap-decisions-1.xml.gz");
        assert_eq!(files[1].content_type, "application/gzip");
        assert_eq!(files[2].filename, SITEMAP_INDEX_NAME);
        assert_eq!(files[2].content_type, "application/xml");

        let index = String::from_utf8(files[2].body.clone()).unwrap();
        assert!(index
            .contains("<loc>https://librejustice.fr/sitemaps/sitemap-decisions-1.xml.gz</loc>"));
        assert!(index.contains("<lastmod>2026-03-15</lastmod>"));
        assert_eq!(files[1].lastmod, d("2026-03-15"));
        assert_eq!(files[2].lastmod, d("2026-05-01"));

        let mut decoder = flate2::read::GzDecoder::new(&files[1].body[..]);
        let mut body = String::new();
        decoder.read_to_string(&mut body).unwrap();
        assert!(body.contains("https://librejustice.fr/decision/fr/a"));
        assert!(body.contains("https://librejustice.fr/decision/fr/b"));
    }

    #[test]
    fn includes_referential_law_urls() {
        let source = FakeSource {
            decisions: vec![("/decision/fr/a".into(), Some(d("2026-01-01")))],
            referential: vec![(
                "code-civil".into(),
                "1240".into(),
                "FR".into(),
                Some(d("2026-02-02")),
            )],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-01-01")).unwrap();
        let body = decompress_subs(&files);
        assert!(body.contains("https://librejustice.fr/norm/fr/code-civil/1240"));
    }

    #[test]
    fn includes_jurisdiction_hub_urls() {
        let source = FakeSource {
            jurisdiction_hubs: vec![("fr".into(), "ca_paris".into(), vec![(2026, 3), (2025, 1)])],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-05-01")).unwrap();
        assert!(files
            .iter()
            .any(|f| f.filename == "sitemap-juridictions-1.xml.gz"));
        let body = decompress_subs(&files);
        assert!(body.contains("<loc>https://librejustice.fr/decision-index/fr</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/decision-index/fr/ca_paris</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/decision-index/fr/ca_paris/2026</loc>"));
        assert!(body
            .contains("<loc>https://librejustice.fr/decision-index/fr/ca_paris/2026?page=2</loc>"));
        assert!(body
            .contains("<loc>https://librejustice.fr/decision-index/fr/ca_paris/2026?page=3</loc>"));
        assert!(!body.contains("ca_paris/2026?page=4"));
        assert!(body.contains("<loc>https://librejustice.fr/decision-index/fr/ca_paris/2025</loc>"));
        assert!(!body.contains("ca_paris/2025?page="));
    }

    #[test]
    fn includes_norm_hub_urls() {
        let source = FakeSource {
            norm_hubs: vec![(
                "fr".into(),
                "lois".into(),
                vec![("2026".into(), 2), ("sans-date".into(), 1)],
            )],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-05-01")).unwrap();
        assert!(files
            .iter()
            .any(|f| f.filename == "sitemap-normes-1.xml.gz"));
        let body = decompress_subs(&files);
        assert!(body.contains("<loc>https://librejustice.fr/norm-index/fr</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/norm-index/fr/lois</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/norm-index/fr/lois/2026</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/norm-index/fr/lois/2026?page=2</loc>"));
        assert!(!body.contains("lois/2026?page=3"));
        assert!(body.contains("<loc>https://librejustice.fr/norm-index/fr/lois/sans-date</loc>"));
        assert!(!body.contains("sans-date?page="));
    }

    #[test]
    fn includes_annonce_urls() {
        let source = FakeSource {
            annonces: (
                vec![
                    ("boamp-25-1234".into(), Some(d("2026-07-20"))),
                    ("boamp-25-1235".into(), None),
                ],
                vec!["44".into(), "2A".into()],
            ),
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-08-01")).unwrap();
        assert!(files
            .iter()
            .any(|f| f.filename == "sitemap-annonces-1.xml.gz"));
        let body = decompress_subs(&files);
        assert!(body.contains("<loc>https://librejustice.fr/journal-index/fr/44</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/journal-index/fr/2A</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/notice/fr/boamp-25-1234</loc>"));
        assert!(body.contains("<lastmod>2026-07-20</lastmod>"));
        assert!(body.contains("<loc>https://librejustice.fr/notice/fr/boamp-25-1235</loc>"));
    }

    /// Décompresse tous les sub-sitemaps `.xml.gz` en une seule chaîne.
    fn decompress_subs(files: &[SitemapFile]) -> String {
        let mut all = String::new();
        for f in files
            .iter()
            .filter(|f| f.content_type == "application/gzip")
        {
            let mut decoder = flate2::read::GzDecoder::new(&f.body[..]);
            decoder.read_to_string(&mut all).unwrap();
        }
        all
    }

    #[test]
    fn entity_and_text_url_format() {
        assert_eq!(
            entity_url("siren", "552081317"),
            "https://librejustice.fr/entity/fr/siren/552081317"
        );
        assert_eq!(
            text_url("FR", "code-civil"),
            "https://librejustice.fr/norm/fr/code-civil"
        );
    }

    #[test]
    fn includes_static_texts_and_entities() {
        let source = FakeSource {
            entities: vec![("siren".into(), "552081317".into(), Some(d("2026-04-01")))],
            texts: vec![("code-civil".into(), "FR".into(), Some(d("2026-02-02")))],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-05-01")).unwrap();
        let body = decompress_subs(&files);

        assert!(body.contains("<loc>https://librejustice.fr/fr</loc>"));
        assert!(body.contains("https://librejustice.fr/entity-index/fr/entreprises"));
        assert!(body.contains("https://librejustice.fr/legal-notice"));
        assert!(body.contains("https://librejustice.fr/norm/fr/code-civil<"));
        assert!(body.contains("https://librejustice.fr/entity/fr/siren/552081317"));
        assert!(!body.contains("/entity-index/fr/avocats"));
    }
}
