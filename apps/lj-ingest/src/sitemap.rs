//! Génération des sitemaps (ADR 0064).
//!
//! Pipeline :
//!
//! 1. [`build_sitemaps`] — itère `decisions` par pages de 50 000 URLs et
//!    construit **en mémoire** un fichier `sitemap-{n}.xml.gz` par page + un
//!    `sitemap-index.xml` qui les référence par leur URL publique finale.
//! 2. Côté appelant (`cmd_sitemap`), l'ensemble est upserté dans la table
//!    Postgres `sitemaps` ([`lj_store::repository::DecisionRepository::replace_sitemaps`]).
//!
//! Coté serveur HTTP, `lj-server` sert `/sitemap.xml` (ligne `sitemap-index.xml`)
//! et `/sitemaps/{file}` depuis cette table — plus de Worker Cloudflare ni de
//! bucket R2.

use std::io::Write;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use flate2::{Compression, GzBuilder};

/// Limites W3C Sitemaps : 50 000 URLs OU 50 MiB décompressé par sitemap.
/// On est dominés par la cardinalité, pas la taille. ~60 fichiers pour 3M.
pub const MAX_URLS_PER_SITEMAP: usize = 50_000;

/// Origine publique des URLs ; en dur — un site = une origine canonique.
pub const BASE_URL: &str = "https://librejustice.fr";

/// Nom de la ligne index en base ; `lj-server` la sert sous `/sitemap.xml`.
pub const SITEMAP_INDEX_NAME: &str = "sitemap-index.xml";

/// Un fichier sitemap construit en mémoire, prêt à upserter en base.
///
/// `body` est servi tel quel : `.xml.gz` = **fichier gzip** (`application/gzip`
/// sans `Content-Encoding`, surtout pas `application/xml` + `gzip` → double
/// compression CDN), `.xml` = `application/xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapFile {
    pub filename: String,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub lastmod: NaiveDate,
}

/// Source des décisions à inclure dans le sitemap.
///
/// Un trait laisse les tests fournir un fake sans tirer `lj-store`/Postgres dans
/// le graphe.
pub trait SitemapSource {
    /// Itère `(public_id, lastmod)` pour toutes les décisions à publier.
    fn iter_decisions_for_sitemap(&self) -> Result<Vec<(String, NaiveDate)>>;

    /// Itère `(slug, num, lastmod)` pour les articles de référentiel à publier
    /// (pages `/texte/{slug}/{num}`, ADR 0097).
    fn iter_referential_for_sitemap(&self) -> Result<Vec<(String, String, NaiveDate)>>;

    /// Itère `(ns, id, lastmod)` pour les entités de l'annuaire à publier
    /// (pages `/entite/{ns}/{id}`, ADR 0237). Les avocats (personnes physiques)
    /// sont exclus côté SQL (RGPD).
    fn iter_entities_for_sitemap(&self) -> Result<Vec<(String, String, NaiveDate)>>;

    /// Itère `(slug, lastmod)` pour les codes navigables (pages TDM
    /// `/texte/{slug}`, ADR 0237).
    fn iter_codes_for_sitemap(&self) -> Result<Vec<(String, NaiveDate)>>;

    /// Itère `(code, années)` pour les hubs juridiction (pages
    /// `/juridiction/{code}` + `/juridiction/{code}/{annee}`, ADR 0253).
    fn iter_jurisdiction_hubs_for_sitemap(&self) -> Result<Vec<(String, Vec<i32>)>>;

    /// Itère `(fond, tokens année)` pour les hubs du catalogue des normes
    /// (pages `/normes/{fond}` + `/normes/{fond}/{annee}`, ADR 0255) —
    /// `annee` = année ou `sans-date`.
    fn iter_norm_hubs_for_sitemap(&self) -> Result<Vec<(String, Vec<String>)>>;
}

/// Pages statiques indexables, servies en SSR mais absentes des tables (donc
/// non énumérables en base) — landing, recherche, hubs d'annuaire, mentions
/// légales… (ADR 0237). Chemins relatifs à [`BASE_URL`] ; leur `lastmod` est la
/// date du run (pas de signal de fraîcheur par page). Le hub `/annuaire/avocats`
/// est **volontairement absent** : il pagine les avocats (personnes physiques),
/// même exclusion SEO que leurs fiches.
const STATIC_PATHS: &[&str] = &[
    "/",
    "/decisions",
    "/textes",
    "/sources",
    "/codes",
    "/juridictions",
    "/normes",
    "/annuaire",
    "/annuaire/entreprises",
    "/annuaire/personnes-publiques",
    "/annuaire/associations",
    "/annuaire/cabinets",
    "/mcp-guide",
    "/mentions-legales",
    "/confidentialite",
];

fn decision_url(public_id: &str) -> String {
    format!("{BASE_URL}/decision/{public_id}")
}

fn law_url(code: &str, num: &str) -> String {
    format!("{BASE_URL}/texte/{code}/{num}")
}

fn entity_url(ns: &str, id: &str) -> String {
    format!("{BASE_URL}/entite/{ns}/{id}")
}

fn code_url(slug: &str) -> String {
    format!("{BASE_URL}/texte/{slug}")
}

fn static_url(path: &str) -> String {
    format!("{BASE_URL}{path}")
}

fn sitemap_url(filename: &str) -> String {
    format!("{BASE_URL}/sitemaps/{filename}")
}

/// Échappe `&`, `<`, `>` — équivalent `xml.sax.saxutils.escape` (défaut).
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Bloc `<url>` minimal — pas de `priority`/`changefreq` (ignorés par Google
/// depuis 2023). `loc` est échappé ici ; les appelants passent l'URL brute.
fn format_url_entry(loc: &str, lastmod: NaiveDate) -> String {
    format!(
        "<url><loc>{}</loc><lastmod>{}</lastmod></url>",
        xml_escape(loc),
        lastmod.format("%Y-%m-%d"),
    )
}

/// gzip avec `mtime=0` pour des bytes reproductibles entre runs. `gzip.compress`
/// par défaut écrit `time.time()` dans l'en-tête, ce qui casse le diff entre deux
/// runs (et l'upsert idempotent en base).
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
/// `entites`, `decisions`, ADR 0253) : Search Console expose alors
/// l'indexation par fond au lieu d'une numérotation opaque. Retourne la
/// liste des fichiers (index en dernier). L'index réfère les sub-sitemaps
/// via leur URL publique finale, comme l'exige sitemaps.org § sitemap index
/// (URLs absolues).
pub fn build_sitemaps<S: SitemapSource>(repo: &S, today: NaiveDate) -> Result<Vec<SitemapFile>> {
    let mut files: Vec<SitemapFile> = Vec::new();

    // Pages statiques indexables (landing, hubs annuaire…), `lastmod` = run.
    let mut pages = SectionWriter::new("pages");
    for path in STATIC_PATHS {
        pages.push(&static_url(path), today)?;
    }
    files.extend(pages.finish()?);

    // Hubs juridiction `/juridiction/{code}` + `/juridiction/{code}/{annee}`
    // (ADR 0253) — `lastmod` = run : leurs listes bougent à l'ingest quotidien.
    let mut hubs = SectionWriter::new("juridictions");
    for (code, years) in repo.iter_jurisdiction_hubs_for_sitemap()? {
        hubs.push(&format!("{BASE_URL}/juridiction/{code}"), today)?;
        for year in years {
            hubs.push(&format!("{BASE_URL}/juridiction/{code}/{year}"), today)?;
        }
    }
    files.extend(hubs.finish()?);

    // Hubs du catalogue des normes `/normes/{fond}` + `/normes/{fond}/{annee}`
    // (ADR 0255, `annee` = année ou `sans-date`) — même profil.
    let mut normes = SectionWriter::new("normes");
    for (fond, tokens) in repo.iter_norm_hubs_for_sitemap()? {
        normes.push(&format!("{BASE_URL}/normes/{fond}"), today)?;
        for token in tokens {
            normes.push(&format!("{BASE_URL}/normes/{fond}/{token}"), today)?;
        }
    }
    files.extend(normes.finish()?);

    // Textes : pages TDM `/texte/{slug}` (ADR 0237) + articles
    // `/texte/{slug}/{num}` (ADR 0097).
    let mut textes = SectionWriter::new("textes");
    for (slug, lastmod) in repo.iter_codes_for_sitemap()? {
        textes.push(&code_url(&slug), lastmod)?;
    }
    for (slug, num, lastmod) in repo.iter_referential_for_sitemap()? {
        textes.push(&law_url(&slug, &num), lastmod)?;
    }
    files.extend(textes.finish()?);

    // Fiches d'entités de l'annuaire `/entite/{ns}/{id}` (ADR 0237, avocats
    // exclus côté SQL).
    let mut entites = SectionWriter::new("entites");
    for (ns, id, lastmod) in repo.iter_entities_for_sitemap()? {
        entites.push(&entity_url(&ns, &id), lastmod)?;
    }
    files.extend(entites.finish()?);

    let mut decisions = SectionWriter::new("decisions");
    for (public_id, lastmod) in repo.iter_decisions_for_sitemap()? {
        decisions.push(&decision_url(&public_id), lastmod)?;
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
        lastmod: index_lastmod.unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()),
    });
    Ok(files)
}

/// Accumule les blocs `<url>` d'une **section** (un fond : `pages`,
/// `decisions`…) et découpe en fichiers `sitemap-{section}-{n}.xml.gz` de
/// [`MAX_URLS_PER_SITEMAP`] au plus. Tient le max `lastmod` par fichier
/// (= `lastmod` de la ligne d'index correspondante, permet à Google de ne
/// re-crawler que les subs modifiés).
struct SectionWriter {
    section: &'static str,
    files: Vec<SitemapFile>,
    entries: Vec<String>,
    max_lastmod: Option<NaiveDate>,
}

impl SectionWriter {
    fn new(section: &'static str) -> Self {
        Self {
            section,
            files: Vec::new(),
            entries: Vec::new(),
            max_lastmod: None,
        }
    }

    fn push(&mut self, loc: &str, lastmod: NaiveDate) -> Result<()> {
        self.entries.push(format_url_entry(loc, lastmod));
        if self.max_lastmod.is_none_or(|m| lastmod > m) {
            self.max_lastmod = Some(lastmod);
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
        let max_lastmod = self.max_lastmod.take().expect("entrées sans lastmod");
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
        decisions: Vec<(String, NaiveDate)>,
        referential: Vec<(String, String, NaiveDate)>,
        entities: Vec<(String, String, NaiveDate)>,
        codes: Vec<(String, NaiveDate)>,
        jurisdiction_hubs: Vec<(String, Vec<i32>)>,
        norm_hubs: Vec<(String, Vec<String>)>,
    }
    impl SitemapSource for FakeSource {
        fn iter_decisions_for_sitemap(&self) -> Result<Vec<(String, NaiveDate)>> {
            Ok(self.decisions.clone())
        }
        fn iter_referential_for_sitemap(&self) -> Result<Vec<(String, String, NaiveDate)>> {
            Ok(self.referential.clone())
        }
        fn iter_entities_for_sitemap(&self) -> Result<Vec<(String, String, NaiveDate)>> {
            Ok(self.entities.clone())
        }
        fn iter_codes_for_sitemap(&self) -> Result<Vec<(String, NaiveDate)>> {
            Ok(self.codes.clone())
        }
        fn iter_jurisdiction_hubs_for_sitemap(&self) -> Result<Vec<(String, Vec<i32>)>> {
            Ok(self.jurisdiction_hubs.clone())
        }
        fn iter_norm_hubs_for_sitemap(&self) -> Result<Vec<(String, Vec<String>)>> {
            Ok(self.norm_hubs.clone())
        }
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    // Spec : bloc <url> = loc + lastmod ISO, sans priority/changefreq.
    #[test]
    fn url_entry_format() {
        assert_eq!(
            format_url_entry(&decision_url("ce_42"), d("2026-05-29")),
            "<url><loc>https://librejustice.fr/decision/ce_42</loc>\
             <lastmod>2026-05-29</lastmod></url>"
        );
    }

    // Spec : URL page loi = {BASE_URL}/texte/{code}/{num}.
    #[test]
    fn law_url_format() {
        assert_eq!(
            law_url("code-civil", "1240"),
            "https://librejustice.fr/texte/code-civil/1240"
        );
    }

    // Spec : xml_escape n'échappe que & < > (pas les quotes).
    #[test]
    fn escapes_only_xml_specials() {
        assert_eq!(xml_escape("a&b<c>d\"e"), "a&amp;b&lt;c&gt;d\"e");
    }

    // Spec : gzip déterministe (mtime=0) → identique entre deux runs.
    #[test]
    fn gzip_reproducible() {
        let a = gzip_bytes(b"hello world").unwrap();
        let b = gzip_bytes(b"hello world").unwrap();
        assert_eq!(a, b);
        // Octets mtime (offset 4..8 de l'en-tête gzip) à zéro.
        assert_eq!(&a[4..8], &[0, 0, 0, 0]);
    }

    // Spec : build_sitemaps produit un sub PAR SECTION alimentée (nommé
    // sitemap-{section}-{n}, ADR 0253) + index ; l'index référence les sub
    // par URL absolue avec leur max lastmod ; les sub décompressent en urlset.
    #[test]
    fn builds_index_and_subs() {
        let source = FakeSource {
            decisions: vec![("a".into(), d("2026-01-01")), ("b".into(), d("2026-03-15"))],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2000-01-01")).unwrap();
        // Sections alimentées : pages (statiques) + decisions, puis l'index.
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].filename, "sitemap-pages-1.xml.gz");
        assert_eq!(files[1].filename, "sitemap-decisions-1.xml.gz");
        assert_eq!(files[1].content_type, "application/gzip");
        assert_eq!(files[2].filename, SITEMAP_INDEX_NAME);
        assert_eq!(files[2].content_type, "application/xml");

        let index = String::from_utf8(files[2].body.clone()).unwrap();
        assert!(index
            .contains("<loc>https://librejustice.fr/sitemaps/sitemap-decisions-1.xml.gz</loc>"));
        // Max lastmod du sub décisions (`today` ancien → celui des décisions).
        assert!(index.contains("<lastmod>2026-03-15</lastmod>"));
        assert_eq!(files[2].lastmod, d("2026-03-15"));

        // Le sub décisions décompresse en un urlset contenant les deux décisions.
        let mut decoder = flate2::read::GzDecoder::new(&files[1].body[..]);
        let mut body = String::new();
        decoder.read_to_string(&mut body).unwrap();
        assert!(body.contains("https://librejustice.fr/decision/a"));
        assert!(body.contains("https://librejustice.fr/decision/b"));
    }

    // Spec : un article de référentiel fourni par la source produit une <url>
    // /texte/{slug}/{num} dans le sub-sitemap.
    #[test]
    fn includes_referential_law_urls() {
        let source = FakeSource {
            decisions: vec![("a".into(), d("2026-01-01"))],
            referential: vec![("code-civil".into(), "1240".into(), d("2026-02-02"))],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-01-01")).unwrap();
        let body = decompress_subs(&files);
        assert!(body.contains("https://librejustice.fr/texte/code-civil/1240"));
    }

    // Spec : les hubs juridiction (ADR 0253) produisent une section dédiée
    // sitemap-juridictions-N avec le hub du code et une URL par année.
    #[test]
    fn includes_jurisdiction_hub_urls() {
        let source = FakeSource {
            jurisdiction_hubs: vec![("ca_paris".into(), vec![2026, 2025])],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-05-01")).unwrap();
        assert!(files
            .iter()
            .any(|f| f.filename == "sitemap-juridictions-1.xml.gz"));
        let body = decompress_subs(&files);
        assert!(body.contains("<loc>https://librejustice.fr/juridictions</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/juridiction/ca_paris</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/juridiction/ca_paris/2026</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/juridiction/ca_paris/2025</loc>"));
    }

    // Spec : les hubs normes (ADR 0255) produisent une section dédiée
    // sitemap-normes-N avec le hub du fond et une URL par token d'année
    // (année ou sans-date).
    #[test]
    fn includes_norm_hub_urls() {
        let source = FakeSource {
            norm_hubs: vec![("lois".into(), vec!["2026".into(), "sans-date".into()])],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-05-01")).unwrap();
        assert!(files
            .iter()
            .any(|f| f.filename == "sitemap-normes-1.xml.gz"));
        let body = decompress_subs(&files);
        assert!(body.contains("<loc>https://librejustice.fr/normes</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/normes/lois</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/normes/lois/2026</loc>"));
        assert!(body.contains("<loc>https://librejustice.fr/normes/lois/sans-date</loc>"));
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

    // Spec : URL fiche entité = {BASE_URL}/entite/{ns}/{id} ; URL code = /texte/{slug}.
    #[test]
    fn entity_and_code_url_format() {
        assert_eq!(
            entity_url("siren", "552081317"),
            "https://librejustice.fr/entite/siren/552081317"
        );
        assert_eq!(
            code_url("code-civil"),
            "https://librejustice.fr/texte/code-civil"
        );
    }

    // Spec : build_sitemaps émet les pages statiques (landing, hubs annuaire),
    // les codes et les fiches d'entités ; le hub /annuaire/avocats est exclu.
    #[test]
    fn includes_static_codes_and_entities() {
        let source = FakeSource {
            entities: vec![("siren".into(), "552081317".into(), d("2026-04-01"))],
            codes: vec![("code-civil".into(), d("2026-02-02"))],
            ..Default::default()
        };
        let files = build_sitemaps(&source, d("2026-05-01")).unwrap();
        let body = decompress_subs(&files);

        assert!(body.contains("<loc>https://librejustice.fr/</loc>"));
        assert!(body.contains("https://librejustice.fr/annuaire/entreprises"));
        assert!(body.contains("https://librejustice.fr/mentions-legales"));
        assert!(body.contains("https://librejustice.fr/texte/code-civil<"));
        assert!(body.contains("https://librejustice.fr/entite/siren/552081317"));
        // Avocats : ni le hub listing, ni une fiche (exclusion RGPD).
        assert!(!body.contains("/annuaire/avocats"));
    }
}
