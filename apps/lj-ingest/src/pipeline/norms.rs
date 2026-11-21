//! `sync-norms`: one country's norms read from its official consolidations, every dated state
//! of a text written as the versions of its articles. A run reads the states its visits do not
//! hold, and rebuilds the states it did not read from the versions the base holds.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use lj_norm::text_aliases::{self, TextAlias};
use lj_parse::{ConsolidatedArticle, JustelListed, JustelUntil};
use lj_sources::adala::AdalaClient;
use lj_sources::anbenin::AnbeninClient;
use lj_sources::bwb::BwbClient;
use lj_sources::cnlegis::CnlegisClient;
use lj_sources::codex::CodexClient;
use lj_sources::ejo::EjoClient;
use lj_sources::elaws::ElawsClient;
use lj_sources::error::SourceError;
use lj_sources::fedlex::FedlexClient;
use lj_sources::justel::JustelClient;
use lj_sources::justice_laws::JusticeLawsClient;
use lj_sources::legilux::LegiluxClient;
use lj_sources::lexfind::LexfindClient;
use lj_sources::lexwork::{LexworkClient, Listing};
use lj_sources::senlii::SenliiClient;
use lj_sources::vd::VdClient;
use lj_sources::wallex::WallexClient;
use lj_store::repository::{
    ArticleLanguageWrite, DecisionRepository, NormVisit, PublicationRow, StoredArticleVersion,
};

use super::legal_corpus::{
    article_status, load_doc, run_corpus_finishers, write_text_links, CorpusArticle, CorpusDoc,
    CorpusVersion,
};
use crate::config::Settings;

mod adala;
mod anbenin;
mod belgium;
mod bwb;
mod cnlegis;
mod codex;
mod dglex;
mod ejo;
mod elaws;
mod expressions;
mod gallilex;
mod justel;
mod justice_laws;
mod legilux;
mod lims;
mod publications;
mod senlii;
mod wallex;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum NormsCountry {
    Ch,
    Lu,
    /// Every federal Act and regulation on Justice Laws, French and English.
    Ca,
    /// Every Ontario statute and regulation on e-Laws, French where it has a French page.
    CaOn,
    /// Québec's statutes and regulations, from the LégisQuébec pages in the cache.
    CaQc,
    /// New Brunswick's acts and regulations, from the pages of its site in the cache.
    CaNb,
    /// The acts in force of the Swiss cantons: 19 on LexWork, Vaud on its BLV, six on LexFind.
    ChCantons,
    /// The federal codes and the Constitution on Justel.
    Be,
    /// The Flemish acts with a text on the Vlaamse Codex.
    BeVlg,
    /// The acts of the Walloon Region on Wallex.
    BeWal,
    /// The federal acts on Justel, beside the codes [`BELGIUM`] follows.
    BeFed,
    /// The acts of the Brussels-Capital Region, the COCOM and the COCOF on Justel.
    BeBru,
    /// The acts of the French Community on Justel.
    BeCfr,
    /// The acts of the German-speaking Community on Justel.
    BeCde,
    /// The German texts of the German-speaking Community's acts on DGLex, hung on their Justel texts.
    BeDg,
    /// The French Community's acts Justel does not list, from Gallilex's pages and PDFs in the cache.
    BeGallilex,
    /// The texts of Djibouti's electronic Journal officiel, as published.
    Dj,
    /// The texts of Madagascar's CNLEGIS, as published.
    Mg,
    /// The laws of Benin's National Assembly, as promulgated.
    Bj,
    /// Morocco's texts on the Justice ministry's Adala portal, as consolidated.
    Ma,
    /// Senegal's texts on SenegalLII, each dated expression a version.
    Sn,
    /// OHADA's uniform acts, Treaty and rules on SenegalLII.
    Ohada,
    /// The Netherlands' regelingen in the BWB, each consolidated state a version.
    Nl,
}

/// Which listed states a run reads, beside those never visited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum NormsScan {
    /// Those the listing says were modified since their last visit.
    #[default]
    Daily,
    /// Those too, and every state not visited for [`RELIST_AFTER_DAYS`]: the weekly net, which
    /// warns about what the daily scan missed.
    Relist,
}

/// Under a week, so that a relist reads every state the previous one read, whatever the hour it
/// reached it.
const RELIST_AFTER_DAYS: i64 = 6;

fn relist_before(scan: NormsScan, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (scan == NormsScan::Relist).then(|| now - chrono::Duration::days(RELIST_AFTER_DAYS))
}

/// Why a run reads a listed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    New,
    Modified,
    Relist,
}

/// Why a run reads a listed state its publisher last modified on `modified`, given its last
/// visit, `relist_before` set on a relist. A modification on the day of the visit may have followed
/// it.
fn reason(
    modified: Option<NaiveDate>,
    visit: Option<&NormVisit>,
    relist_before: Option<DateTime<Utc>>,
) -> Option<Reason> {
    let Some(visit) = visit else {
        return Some(Reason::New);
    };
    if modified.is_some_and(|day| day >= visit.visited_at.date_naive()) {
        return Some(Reason::Modified);
    }
    relist_before
        .filter(|&before| visit.visited_at < before)
        .map(|_| Reason::Relist)
}

impl NormsCountry {
    /// The jurisdiction, the texts followed by name, and whether an article's status is read from
    /// its wording. The cantons follow every act their index lists, Belgium the texts of [`BELGIUM`].
    fn followed(self) -> (&'static str, &'static [Followed], bool) {
        match self {
            NormsCountry::Ch => ("CH", SWISS, false),
            NormsCountry::Lu => ("LU", LUXEMBOURG, true),
            NormsCountry::Ca => ("CA", CANADA, false),
            NormsCountry::CaOn | NormsCountry::CaQc | NormsCountry::CaNb => ("CA", &[], false),
            NormsCountry::ChCantons => ("CH", &[], false),
            NormsCountry::Be => ("BE", &[], true),
            NormsCountry::BeVlg => ("BE", &[], false),
            NormsCountry::BeWal => ("BE", &[], false),
            NormsCountry::BeFed
            | NormsCountry::BeBru
            | NormsCountry::BeCfr
            | NormsCountry::BeCde
            | NormsCountry::BeDg
            | NormsCountry::BeGallilex => ("BE", &[], false),
            NormsCountry::Dj => ("DJ", &[], false),
            NormsCountry::Mg => ("MG", &[], false),
            NormsCountry::Bj => ("BJ", &[], false),
            NormsCountry::Ma => ("MA", &[], false),
            NormsCountry::Sn => ("SN", &[], false),
            NormsCountry::Ohada => ("OHADA", &[], false),
            NormsCountry::Nl => ("NL", &[], false),
        }
    }
}

/// A text the pipeline follows: its key at the publisher (an RS number, a Legilux code, a
/// consolidated Act number) and the text it writes.
struct Followed {
    key: &'static str,
    url: &'static str,
    text_uid: &'static str,
    title: &'static str,
    title_key: Option<&'static str>,
    nature: &'static str,
}

const SWISS: &[Followed] = &[
    Followed {
        key: "101",
        url: "https://www.fedlex.admin.ch/eli/cc/1999/404/fr",
        text_uid: "CH/CONSTITUTION-FEDERALE",
        title: "Constitution fédérale de la Confédération suisse (RS 101)",
        title_key: Some("Constitution fédérale"),
        nature: "CONSTITUTION",
    },
    Followed {
        key: "210",
        url: "https://www.fedlex.admin.ch/eli/cc/24/233_245_233/fr",
        text_uid: "CH/CODE-CIVIL",
        title: "Code civil (Suisse), RS 210",
        title_key: None,
        nature: "code_civil",
    },
    Followed {
        key: "220",
        url: "https://www.fedlex.admin.ch/eli/cc/27/317_321_377/fr",
        text_uid: "CH/CODE-OBLIGATIONS",
        title: "Code des obligations (Suisse), RS 220",
        title_key: None,
        nature: "code_obligations",
    },
    Followed {
        key: "272",
        url: "https://www.fedlex.admin.ch/eli/cc/2010/262/fr",
        text_uid: "CH/CODE-PROCEDURE-CIVILE",
        title: "Code de procédure civile suisse",
        title_key: Some("code de procédure civile suisse"),
        nature: "code_procedure_civile",
    },
    Followed {
        key: "281.1",
        url: "https://www.fedlex.admin.ch/eli/cc/11/529_488_529/fr",
        text_uid: "CH/LOI-POURSUITE-DETTES-FAILLITE-LP",
        title: "Loi fédérale sur la poursuite pour dettes et la faillite (Suisse), LP, RS 281.1",
        title_key: Some("Loi fédérale suisse sur la poursuite"),
        nature: "loi",
    },
    Followed {
        key: "291",
        url: "https://www.fedlex.admin.ch/eli/cc/1988/1776_1776_1776/fr",
        text_uid: "CH/LOI-DROIT-INTERNATIONAL-PRIVE-LDIP",
        title: "Loi fédérale sur le droit international privé (Suisse), LDIP, RS 291",
        title_key: Some("Loi fédérale suisse sur le droit international privé"),
        nature: "loi",
    },
    Followed {
        key: "830.1",
        url: "https://www.fedlex.admin.ch/eli/cc/2002/510/fr",
        text_uid: "CH/LOI-PARTIE-GENERALE-ASSURANCES-SOCIALES-LPGA",
        title: "Loi fédérale sur la partie générale du droit des assurances sociales (Suisse), LPGA, RS 830.1",
        title_key: Some(
            "Loi fédérale suisse sur la partie générale du droit des assurances sociales",
        ),
        nature: "loi",
    },
];

const LUXEMBOURG: &[Followed] = &[
    Followed {
        key: "civil",
        url: "https://legilux.public.lu/eli/etat/leg/code/civil",
        text_uid: "LU/CODE-CIVIL",
        title: "Code civil luxembourgeois",
        title_key: None,
        nature: "code_civil",
    },
    Followed {
        key: "penal",
        url: "https://legilux.public.lu/eli/etat/leg/code/penal",
        text_uid: "LU/CODE-PENAL",
        title: "Code pénal luxembourgeois",
        title_key: None,
        nature: "code_penal",
    },
    Followed {
        key: "procedure_civile",
        url: "https://legilux.public.lu/eli/etat/leg/code/procedure_civile",
        text_uid: "LU/CODE-PROCEDURE-CIVILE",
        title: "Nouveau Code de procédure civile luxembourgeois",
        title_key: Some("code de procédure civile luxembourgeois"),
        nature: "code_procedure_civile",
    },
];

const CANADA: &[Followed] = &[
    Followed {
        key: "C-46",
        url: "https://laws-lois.justice.gc.ca/fra/lois/C-46/",
        text_uid: "CA/C-46",
        title: "Code criminel (Canada), L.R.C. 1985, ch. C-46",
        title_key: None,
        nature: "code_penal",
    },
    Followed {
        key: "D-3.4",
        url: "https://laws-lois.justice.gc.ca/fra/lois/D-3.4/",
        text_uid: "CA/D-3.4",
        title: "Loi sur le divorce (Canada), L.R.C. 1985, ch. 3 (2e suppl.)",
        title_key: None,
        nature: "loi",
    },
    Followed {
        key: "C-5",
        url: "https://laws-lois.justice.gc.ca/fra/lois/C-5/",
        text_uid: "CA/C-5",
        title: "Loi sur la preuve au Canada, L.R.C. 1985, ch. C-5",
        title_key: None,
        nature: "loi",
    },
    Followed {
        key: "B-3",
        url: "https://laws-lois.justice.gc.ca/fra/lois/B-3/",
        text_uid: "CA/B-3",
        title: "Loi sur la faillite et l'insolvabilité (Canada), L.R.C. 1985, ch. B-3",
        title_key: None,
        nature: "loi",
    },
    Followed {
        key: "I-2.5",
        url: "https://laws-lois.justice.gc.ca/fra/lois/I-2.5/",
        text_uid: "CA/I-2.5",
        title: "Loi sur l'immigration et la protection des réfugiés (Canada), L.C. 2001, ch. 27",
        title_key: None,
        nature: "loi",
    },
    Followed {
        key: "H-6",
        url: "https://laws-lois.justice.gc.ca/fra/lois/H-6/",
        text_uid: "CA/H-6",
        title: "Loi canadienne sur les droits de la personne, L.R.C. 1985, ch. H-6",
        title_key: None,
        nature: "loi",
    },
];

/// A Belgian text on Justel: the documents its articles are spread over, in reading order, the day
/// it was adopted, and whether its article numbers write a Latin suffix apart (`7 bis`).
struct JustelText {
    followed: Followed,
    documents: &'static [JustelDocument<'static>],
    adopted: Option<&'static str>,
    spaced: bool,
}

/// A consolidated document, published as `numac` and numbered `cn` by Justel, with the day it
/// entered into force when every amendment since bears a note, and the book of a code it carries.
struct JustelDocument<'a> {
    numac: &'a str,
    cn: Option<&'a str>,
    born: Option<&'a str>,
    book: Option<justel::Book>,
}

impl JustelDocument<'_> {
    /// What its visits are kept under: its number, else its numac.
    fn key(&self) -> &str {
        self.cn.unwrap_or(self.numac)
    }
}

const fn document(numac: &'static str, cn: &'static str) -> JustelDocument<'static> {
    JustelDocument {
        numac,
        cn: Some(cn),
        born: None,
        book: None,
    }
}

const fn born(numac: &'static str, cn: &'static str, day: &'static str) -> JustelDocument<'static> {
    JustelDocument {
        born: Some(day),
        ..document(numac, cn)
    }
}

const fn book(
    numac: &'static str,
    cn: &'static str,
    day: &'static str,
    prefix: &'static str,
    label: &'static str,
) -> JustelDocument<'static> {
    JustelDocument {
        book: Some(justel::Book { prefix, label }),
        ..born(numac, cn, day)
    }
}

const BELGIUM: &[JustelText] = &[
    JustelText {
        followed: Followed {
            key: "constitution",
            url: "https://www.ejustice.just.fgov.be/eli/constitution/1994/02/17/1994021048/justel",
            text_uid: "BE/CONSTITUTION-1994",
            title: "Constitution belge",
            title_key: Some("Constitution belge"),
            nature: "CONSTITUTION",
        },
        documents: &[born("1994021048", "1994021730", "1994-02-27")],
        adopted: Some("1994-02-17"),
        spaced: true,
    },
    JustelText {
        followed: Followed {
            key: "code-civil",
            url: "https://www.ejustice.just.fgov.be/eli/loi/1804/03/21/1804032130/justel",
            text_uid: "BE/CODE-CIVIL",
            title: "Code civil belge (Ancien Code civil)",
            title_key: Some("code civil belge"),
            nature: "code_civil",
        },
        documents: &[
            document("1804032150", "1804032130"),
            document("1804032151", "1804032131"),
            document("1804032152", "1804032132"),
            document("1804032153", "1804032133"),
            document("1804032154", "1804032134"),
            document("1804032155", "1804032135"),
        ],
        adopted: Some("1804-03-21"),
        spaced: true,
    },
    JustelText {
        followed: Followed {
            key: "code-civil-nouveau",
            url: "https://www.ejustice.just.fgov.be/eli/loi/2022/04/28/2022A32057/justel",
            text_uid: "BE/CODE-CIVIL-NOUVEAU",
            title: "Code civil belge (nouveau Code civil)",
            title_key: Some("nouveau code civil belge"),
            nature: "code_civil",
        },
        documents: &[
            book(
                "2022A32057",
                "2022042826",
                "2023-01-01",
                "1.",
                "LIVRE 1er. - Dispositions générales",
            ),
            book(
                "2022A30600",
                "2022011919",
                "2022-07-01",
                "2.3.",
                "LIVRE 2, TITRE 3. - Les relations patrimoniales des couples",
            ),
            book(
                "2020A20347",
                "2020020419",
                "2021-09-01",
                "3.",
                "LIVRE 3. - Les biens",
            ),
            book(
                "2022B30600",
                "2022011920",
                "2022-07-01",
                "4.",
                "LIVRE 4. - Les successions, donations et testaments",
            ),
            book(
                "2022A32058",
                "2022042827",
                "2023-01-01",
                "5.",
                "LIVRE 5. - Les obligations",
            ),
            book(
                "2024001600",
                "2024020718",
                "2025-01-01",
                "6.",
                "LIVRE 6. - La responsabilité extracontractuelle",
            ),
            book(
                "2019A12168",
                "2019041329",
                "2020-11-01",
                "8.",
                "LIVRE 8. - La preuve",
            ),
            book(
                "2025A05089",
                "2025060510",
                "2026-01-01",
                "9.1.",
                "LIVRE 9. - Les sûretés > TITRE 1er. - Les sûretés personnelles",
            ),
        ],
        adopted: None,
        spaced: false,
    },
    JustelText {
        followed: Followed {
            key: "code-judiciaire",
            url: "https://www.ejustice.just.fgov.be/eli/loi/1967/10/10/1967101052/justel",
            text_uid: "BE/CODE-JUDICIAIRE",
            title: "Code judiciaire belge",
            title_key: Some("code judiciaire belge"),
            nature: "code_procedure_civile",
        },
        documents: &[
            document("1967101052", "1967101001"),
            document("1967101053", "1967101002"),
            document("1967101054", "1967101003"),
            document("1967101055", "1967101004"),
            document("1967101056", "1967101005"),
            document("1967101057", "1967101006"),
            document("1967101063", "1967101035"),
            document("1967101064", "1967101052"),
        ],
        adopted: Some("1967-10-10"),
        spaced: false,
    },
    JustelText {
        followed: Followed {
            key: "code-penal",
            url: "https://www.ejustice.just.fgov.be/eli/loi/1867/06/08/1867060850/justel",
            text_uid: "BE/CODE-PENAL",
            title: "Code pénal belge (ancien Code pénal)",
            title_key: Some("code pénal belge"),
            nature: "code_penal",
        },
        documents: &[document("1867060850", "1867060801")],
        adopted: Some("1867-06-08"),
        spaced: true,
    },
    JustelText {
        followed: Followed {
            key: "code-instruction-criminelle",
            url: "https://www.ejustice.just.fgov.be/eli/loi/1808/11/17/1808111701/justel",
            text_uid: "BE/CODE-INSTRUCTION-CRIMINELLE",
            title: "Code d'instruction criminelle belge",
            title_key: Some("code d'instruction criminelle belge"),
            nature: "code_procedure_penale",
        },
        documents: &[document("1808111701", "1808111730")],
        adopted: Some("1808-11-17"),
        spaced: true,
    },
    JustelText {
        followed: Followed {
            key: "code-droit-economique",
            url: "https://www.ejustice.just.fgov.be/eli/loi/2013/02/28/2013A11134/justel",
            text_uid: "BE/CODE-DROIT-ECONOMIQUE",
            title: "Code de droit économique belge",
            title_key: Some("code de droit économique belge"),
            nature: "code_commerce",
        },
        documents: &[born("2013A11134", "2013022819", "2013-12-12")],
        adopted: Some("2013-02-28"),
        spaced: false,
    },
];

/// How a Belgian text is read on Justel: over the documents its table names, or as the one act a
/// listing names.
enum JustelRead {
    Followed(&'static JustelText),
    Listed(JustelListed),
}

/// A text a run reads: its key at the publisher, the text it writes, and the publisher's identifier
/// of its current state when an index names it.
struct Target {
    key: String,
    url: String,
    text_uid: String,
    title: String,
    title_key: Option<String>,
    nature: String,
    current: Option<u64>,
    justel: Option<JustelRead>,
    /// The names its publisher's index gives it besides its title.
    aliases: Vec<TextAlias>,
}

impl From<&Followed> for Target {
    fn from(f: &Followed) -> Self {
        Self {
            key: f.key.to_string(),
            url: f.url.to_string(),
            text_uid: f.text_uid.to_string(),
            title: f.title.to_string(),
            title_key: f.title_key.map(str::to_string),
            nature: f.nature.to_string(),
            current: None,
            justel: None,
            aliases: Vec::new(),
        }
    }
}

/// The names a text is cited by besides its title: the hand-written ones and its index's.
/// The visit that keeps an act's Moniteur publication.
fn moniteur_visit(key: &str, row: &PublicationRow) -> (String, u64) {
    let read = format!(
        "{}{:?}{:?}{}",
        row.title, row.date_texte, row.date_publi, row.url
    );
    (
        publications::moniteur_item(key),
        xxhash_rust::xxh3::xxh3_64(read.as_bytes()),
    )
}

/// A Walloon act's Moniteur publication, read from its page.
fn wallex_publication(t: &Target, page: &lj_parse::WallexPage) -> Result<PublicationRow> {
    let adopted = page
        .adopted
        .as_deref()
        .map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d"))
        .transpose()?;
    let title = wallex::title(&page.title, adopted);
    publications::moniteur(
        &t.text_uid,
        &title,
        belgium::nature(&title),
        page.moniteur_url.as_deref().unwrap_or(&t.url),
        page.adopted.as_deref(),
        page.published.as_deref(),
    )
}

fn aliases(t: &Target) -> Vec<TextAlias> {
    [text_aliases::of(&t.text_uid), t.aliases.clone()].concat()
}

/// The nature of a cantonal act, from the first words of its title.
fn cantonal_nature(title: &str) -> &'static str {
    let head = title
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let starts = |words: &[&str]| words.iter().any(|w| head.starts_with(w));
    let has = |words: &[&str]| words.iter().any(|w| head.contains(w));
    if starts(&["constitution", "costituzione"]) || has(&["verfassung"]) {
        "CONSTITUTION"
    } else if starts(&["loi ", "legge "]) || has(&["gesetz"]) {
        "loi"
    } else if starts(&["ordonnance", "règlement", "reglement", "regolamento"])
        || has(&["verordnung", "bestimmungen"])
    {
        "ordonnance"
    } else if starts(&["arrêté", "décret", "dekret", "decreto"]) || has(&["beschluss"]) {
        "DECRET"
    } else {
        "acte"
    }
}

/// The Codex's acts of an issuer outside Flanders: the federal State, Brussels, the Union.
const NOT_FLEMISH: &[&str] = &[
    "Koninklijk Besluit",
    "Genummerd koninklijk besluit",
    "Wet",
    "Bijzondere Wet",
    "Gecoördineerde Wet",
    "Wetboek",
    "Grondwet",
    "Grondwetwijziging",
    "Ministerieel besluit (Federaal)",
    "Besluitwet",
    "Besluit van de Regent",
    "Keizerlijk Decreet",
    "Ordonnantie van het Brusselse Hoofdstedelijke Parlement",
    "Bijzondere Ordonnantie",
    "Gedelegeerde Verordening",
    "Verordening van het verzekeringscomité van de Dienst voor geneeskundige verzorging van het RIZIV",
];

const MONTHS_NL: [&str; 12] = [
    "januari",
    "februari",
    "maart",
    "april",
    "mei",
    "juni",
    "juli",
    "augustus",
    "september",
    "oktober",
    "november",
    "december",
];

/// `Decreet van 5 april 1995 houdende …`, the heading the Codex gives without its type and date.
fn flemish_title(act: &lj_sources::codex::ListedAct) -> String {
    let kind = act.wetgeving_document_type.trim();
    let heading = act
        .opschrift
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let rest = match heading.get(..kind.len()) {
        Some(head) if head.eq_ignore_ascii_case(kind) => heading[kind.len()..].trim_start(),
        _ => heading.as_str(),
    };
    let date = act
        .datum
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok());
    match date {
        Some(d) => format!(
            "{kind} van {} {} {} {rest}",
            d.day(),
            MONTHS_NL[d.month0() as usize],
            d.year()
        ),
        None => format!("{kind} {rest}"),
    }
}

fn flemish_nature(kind: &str) -> &'static str {
    let kind = kind.to_lowercase();
    if kind.contains("decreet") {
        "DECRET"
    } else if kind.contains("besluit") {
        "ARRETE"
    } else if kind == "samenwerkingsakkoord" {
        "ACCORD"
    } else if kind == "verdrag" {
        "TRAITE"
    } else if kind == "protocol" {
        "PROTOCOLE"
    } else if kind == "omzendbrief" {
        "CIRCULAIRE"
    } else if kind == "overeenkomst" {
        "CONVENTION"
    } else if kind == "verordening" {
        "REGLEMENT"
    } else {
        "ACTE"
    }
}

/// A Walloon act, its title and nature read from its page.
fn wallex_target(id: u64) -> Target {
    Target {
        key: id.to_string(),
        url: WallexClient::page_url(id),
        text_uid: format!("BE-WAL/{id}"),
        title: String::new(),
        title_key: None,
        nature: String::new(),
        current: None,
        justel: None,
        aliases: Vec::new(),
    }
}

/// The nature of a federal act, from its title: a code, else as a cantonal act's.
fn federal_nature(title: &str) -> &'static str {
    match title.to_lowercase().starts_with("code ") {
        true => "CODE",
        false => cantonal_nature(title),
    }
}

/// Vaud's consolidated acts in force. A cote numbers an act and the acts that execute it, so an
/// act is keyed by its cote and its id, and only the one major act of its cote has the cote alone
/// as its uid.
async fn vd_lane(proxy: &str, selected: &dyn Fn(&str) -> bool) -> Result<Lane> {
    let client = VdClient::new(proxy)?;
    let acts = client.acts().await?;
    let mut majors: HashMap<&str, usize> = HashMap::new();
    for act in acts.iter().filter(|a| a.importance == "MAJEUR") {
        *majors.entry(act.cote.as_str()).or_default() += 1;
    }
    let mut targets = Vec::new();
    for act in &acts {
        let key = format!("VD/{}/{}", act.cote, act.id);
        if !selected(&key) {
            continue;
        }
        let sole = act.importance == "MAJEUR" && majors.get(act.cote.as_str()) == Some(&1);
        let short_id = act.id.split('-').next().unwrap_or(&act.id);
        let title = act.title();
        let listed = format!("{}@{:?}@{title}", act.id, act.date_mise_en_vigueur_version);
        targets.push(Target {
            url: VdClient::page_url(&act.id, &act.cote),
            text_uid: match sole {
                true => format!("CH-VD/{}", act.cote),
                false => format!("CH-VD/{}-{short_id}", act.cote),
            },
            title: lj_vocab::cantons::cantonal_title(&title, "VD", &act.cote),
            title_key: None,
            nature: cantonal_nature(&title).to_string(),
            current: Some(xxhash_rust::xxh3::xxh3_64(listed.as_bytes())),
            justel: None,
            aliases: [
                vec![TextAlias::number(&act.cote)],
                text_aliases::in_title("fr", &title),
            ]
            .concat(),
            key,
        });
    }
    tracing::info!(canton = "VD", acts = targets.len(), "cantonal index read");
    Ok(Lane {
        publisher: Publisher::Vd(client),
        targets,
    })
}

/// The cantons read from LexFind's copies, a lane each, every lane through one pace. An act is
/// keyed by its systematic number and its LexFind id.
async fn lexfind_lanes(
    proxy: &str,
    only: Option<&str>,
    selected: &dyn Fn(&str) -> bool,
) -> Result<Vec<Lane>> {
    let mut lanes = Vec::new();
    for client in LexfindClient::per_canton(proxy, |code| {
        only.is_none_or(|o| o.split('/').next() == Some(code))
    })? {
        let canton = client.canton;
        let mut targets = Vec::new();
        for act in client.acts().await? {
            let key = format!("{}/{}/{}", canton.code, act.systematic_number, act.id);
            if !selected(&key) {
                continue;
            }
            let number = act.systematic_number.split_whitespace().collect::<Vec<_>>();
            let title = act.title.trim();
            targets.push(Target {
                url: client.page_url(act.id),
                text_uid: format!("CH-{}/{}", canton.code, number.join("-")),
                title: lj_vocab::cantons::cantonal_title(title, canton.code, &number.join(" ")),
                title_key: None,
                nature: cantonal_nature(title).to_string(),
                current: Some(act.id),
                justel: None,
                aliases: [
                    vec![TextAlias::number(&number.join(" "))],
                    text_aliases::in_title(canton.lang, title),
                ]
                .concat(),
                key,
            });
        }
        tracing::info!(
            canton = canton.code,
            acts = targets.len(),
            "cantonal index read"
        );
        lanes.push(Lane {
            publisher: Publisher::Lexfind(client),
            targets,
        });
    }
    Ok(lanes)
}

/// The acts a Belgian government's issuers adopted, as Justel lists them: every one once a week,
/// written to the cache, and in between those the cache holds and those published since.
async fn justel_listing(
    client: &JustelClient,
    government: &belgium::Government,
    cache: &std::path::Path,
    relist: bool,
) -> Result<Vec<JustelListed>> {
    let (prefix, issuers) = (government.prefix, government.issuers);
    let path = cache.join("justel").join(format!("{prefix}.tsv"));
    let today = Utc::now().date_naive();
    let cached = match relist {
        true => None,
        false => read_listing(&path)?.filter(|(day, _)| today - *day < chrono::Duration::days(7)),
    };
    if let Some((day, mut acts)) = cached {
        for issuer in issuers {
            let since = day - chrono::Duration::days(7);
            acts.extend(
                client
                    .listed(issuer, (since, today), true)
                    .await?
                    .ok_or_else(|| anyhow!("{issuer}: Justel lists too many acts since {since}"))?,
            );
        }
        return Ok(acts);
    }
    let mut acts = Vec::new();
    for issuer in issuers {
        let before = acts.len();
        let mut ranges = vec![(NaiveDate::from_ymd_opt(1800, 1, 1).unwrap(), today)];
        while let Some((from, to)) = ranges.pop() {
            match client.listed(issuer, (from, to), false).await? {
                Some(listed) => acts.extend(listed),
                None if from < to => {
                    let middle = from + (to - from) / 2;
                    ranges.push((middle.succ_opt().unwrap(), to));
                    ranges.push((from, middle));
                }
                None => bail!("{issuer}: Justel lists too many acts adopted on {from}"),
            }
        }
        tracing::info!(
            prefix,
            issuer,
            acts = acts.len() - before,
            "justel issuer listed"
        );
    }
    write_listing(&path, today, &acts)?;
    Ok(acts)
}

/// A listing cached on `path`: the day it was made, then a line an act.
fn read_listing(path: &std::path::Path) -> Result<Option<(NaiveDate, Vec<JustelListed>)>> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let mut lines = raw.lines();
    let day = lines
        .next()
        .and_then(|l| NaiveDate::parse_from_str(l, "%Y-%m-%d").ok())
        .ok_or_else(|| anyhow!("{}: no day on its first line", path.display()))?;
    let field = |f: &str| (!f.is_empty()).then(|| f.to_string());
    let acts = lines
        .map(|l| match l.split('\t').collect::<Vec<_>>()[..] {
            [numac, issuer, adopted, published, heading] => Ok(JustelListed {
                numac: numac.to_string(),
                issuer: issuer.to_string(),
                heading: heading.to_string(),
                adopted: field(adopted),
                published: field(published),
            }),
            _ => Err(anyhow!("{}: not five fields: {l:?}", path.display())),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some((day, acts)))
}

fn write_listing(path: &std::path::Path, day: NaiveDate, acts: &[JustelListed]) -> Result<()> {
    let mut out = format!("{day}\n");
    for a in acts {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            a.numac,
            a.issuer,
            a.adopted.as_deref().unwrap_or_default(),
            a.published.as_deref().unwrap_or_default(),
            a.heading.replace(['\t', '\n'], " ")
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// The acts a listing names, once each and without the German translations the Moniteur numbers
/// apart, as texts `<prefix>/<numac>`: those whose page the cache holds first, then the codes, the
/// laws, ordinances and decrees, the regulations, the orders and the rest, each newest first.
fn justel_targets(
    acts: Vec<JustelListed>,
    government: &belgium::Government,
    selected: &dyn Fn(&str) -> bool,
    cache: &std::path::Path,
) -> Vec<Target> {
    let prefix = government.prefix;
    let mut seen = HashSet::new();
    let mut targets: Vec<(bool, usize, Option<String>, Target)> = acts
        .into_iter()
        .filter(|a| {
            !a.heading.trim_end().ends_with("Traduction allemande")
                && seen.insert(a.numac.clone())
                && selected(&a.numac)
        })
        .map(|act| {
            let adopted = act
                .adopted
                .as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
            let title = government.title(&act.heading, adopted);
            let nature = belgium::nature(&title);
            let rank = match nature {
                _ if act
                    .heading
                    .to_lowercase()
                    .split_once(" portant le code ")
                    .is_some_and(|(head, _)| !head.contains(' ')) =>
                {
                    0
                }
                "CODE" => 0,
                "LOI" | "ORDONNANCE" | "DECRET" => 1,
                "REGLEMENT" => 2,
                "ARRETE" => 3,
                _ => 4,
            };
            let day = act.adopted.clone();
            let uncached = !justel_cached(cache, &act.numac, None).exists();
            let target = Target {
                key: act.numac.clone(),
                url: JustelClient::url(&act.numac, None, None),
                text_uid: format!("{prefix}/{}", act.numac),
                title,
                title_key: None,
                nature: nature.to_string(),
                current: None,
                aliases: text_aliases::in_title("fr", &act.heading),
                justel: Some(JustelRead::Listed(act)),
            };
            (uncached, rank, day, target)
        })
        .collect();
    targets.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| b.2.cmp(&a.2)));
    targets.into_iter().map(|(.., t)| t).collect()
}

/// Where a listed document's current page, or its `archive`th, waits when fetched ahead of the lane.
fn justel_cached(
    cache: &std::path::Path,
    numac: &str,
    archive: Option<usize>,
) -> std::path::PathBuf {
    let name = match archive {
        Some(k) => format!("{numac}@{k:03}"),
        None => numac.to_string(),
    };
    cache
        .join("justel")
        .join("pages")
        .join(format!("{name}.html.zst"))
}

/// The publisher a group of texts is read from, one request at a time.
struct Lane {
    publisher: Publisher,
    targets: Vec<Target>,
}

fn ids(only: Option<&str>, keyed: &str) -> Result<Vec<u64>> {
    only.into_iter()
        .flat_map(|o| o.split(','))
        .map(|id| {
            id.trim()
                .parse::<u64>()
                .map_err(|_| anyhow!("{keyed}, not {id:?}"))
        })
        .collect()
}

/// The lanes of a run: one for a country's publisher, one per canton, each holding the texts
/// `only` selects: a key, or every key under it.
async fn lanes(
    country: NormsCountry,
    proxy: &str,
    only: Option<&str>,
    cache: &std::path::Path,
    relist: bool,
) -> Result<Vec<Lane>> {
    let selected = |key: &str| {
        only.is_none_or(|o| key == o || key.strip_prefix(o).is_some_and(|r| r.starts_with('/')))
    };
    let (publisher, listed) = match country {
        NormsCountry::BeDg => unreachable!("sync_norms reads be-dg on its own"),
        NormsCountry::BeGallilex => unreachable!("sync_norms loads be-gallilex on its own"),
        NormsCountry::CaQc | NormsCountry::CaNb => {
            unreachable!("sync_norms loads ca-qc and ca-nb on their own")
        }
        NormsCountry::Ch => {
            let client = FedlexClient::new(proxy)?;
            let mut targets = Vec::new();
            for act in client.index().await? {
                if act.rs.starts_with("131.") || !selected(&act.rs) {
                    continue;
                }
                let mut aliases = vec![TextAlias::number(&act.rs)];
                for (lang, (title, short)) in &act.titles {
                    aliases.extend(
                        short
                            .as_deref()
                            .map(|s| TextAlias::abbreviation(lang, s.trim())),
                    );
                    aliases.extend(text_aliases::in_title(lang, title));
                }
                if let Some(f) = SWISS.iter().find(|f| f.key == act.rs) {
                    targets.push(Target {
                        aliases,
                        ..Target::from(f)
                    });
                    continue;
                }
                let Some((title, short)) = act.titles.get("fr") else {
                    continue;
                };
                let title = title.trim();
                targets.push(Target {
                    url: format!(
                        "{}/fr",
                        act.eli
                            .replace("fedlex.data.admin.ch", "www.fedlex.admin.ch")
                    ),
                    text_uid: format!("CH/{}", act.rs),
                    title: match short {
                        Some(short) => format!("{title} (Suisse), {short}, RS {}", act.rs),
                        None => format!("{title} (Suisse), RS {}", act.rs),
                    },
                    title_key: None,
                    nature: federal_nature(title).to_string(),
                    current: None,
                    justel: None,
                    aliases,
                    key: act.rs,
                });
            }
            tracing::info!(acts = targets.len(), "systematic collection read");
            return Ok(vec![Lane {
                publisher: Publisher::Fedlex(client),
                targets,
            }]);
        }
        NormsCountry::Lu => {
            let client = LegiluxClient::new(proxy)?;
            let (acts, index) = legilux::targets(client.index().await?, cache, &selected);
            (Publisher::Legilux(client, index), acts)
        }
        NormsCountry::Ca => {
            let client = JusticeLawsClient::new(proxy)?;
            let index = lj_parse::justice_laws_index(&client.index().await?)
                .map_err(|e| anyhow!("{}: {e}", lj_sources::justice_laws::INDEX_URL))?;
            let mut folders = justice_laws::Folders::new();
            for collection in lj_sources::justice_laws::COLLECTIONS {
                let listed = client.folders(collection).await?;
                folders.insert(
                    collection,
                    listed.into_iter().map(|l| (l.name, l.changed)).collect(),
                );
            }
            let (targets, instruments) = justice_laws::targets(index, &folders, &selected);
            return Ok(vec![Lane {
                publisher: Publisher::JusticeLaws(client, instruments),
                targets,
            }]);
        }
        NormsCountry::CaOn => {
            let client = ElawsClient::new(proxy)?;
            let (english, french) = client.listing().await?;
            let (targets, index) = elaws::targets(english, french, &selected);
            return Ok(vec![Lane {
                publisher: Publisher::Elaws(client, index),
                targets,
            }]);
        }
        NormsCountry::ChCantons => {
            let mut lanes = Vec::new();
            for canton in lj_sources::lexwork::CANTONS {
                if only.is_some_and(|o| o.split('/').next() != Some(canton.code)) {
                    continue;
                }
                let client = LexworkClient::new(proxy, canton)?;
                let (mut targets, mut pdf_only) = (Vec::new(), 0usize);
                for act in client.index().await? {
                    let key = format!("{}/{}", canton.code, act.systematic_number);
                    if !selected(&key) {
                        continue;
                    }
                    let Some(current) = act.structured_document_id else {
                        pdf_only += 1;
                        continue;
                    };
                    let number = &act.systematic_number;
                    targets.push(Target {
                        url: client.page_url(number),
                        text_uid: format!(
                            "CH-{}/{}",
                            canton.code,
                            number.split_whitespace().collect::<Vec<_>>().join("-")
                        ),
                        title: lj_vocab::cantons::cantonal_title(
                            act.title.trim(),
                            canton.code,
                            number,
                        ),
                        title_key: None,
                        nature: cantonal_nature(&act.title).to_string(),
                        current: Some(current),
                        justel: None,
                        aliases: [
                            vec![TextAlias::number(number)],
                            text_aliases::in_title(canton.lang, &act.title),
                        ]
                        .concat(),
                        key,
                    });
                }
                tracing::info!(
                    canton = canton.code,
                    acts = targets.len(),
                    pdf_only,
                    "cantonal index read"
                );
                lanes.push(Lane {
                    publisher: Publisher::Lexwork(client),
                    targets,
                });
            }
            if only.is_none_or(|o| o.split('/').next() == Some("VD")) {
                lanes.push(vd_lane(proxy, &selected).await?);
            }
            lanes.extend(lexfind_lanes(proxy, only, &selected).await?);
            return Ok(lanes);
        }
        NormsCountry::BeVlg => {
            let client = CodexClient::new(proxy)?;
            let targets = client
                .listing()
                .await?
                .into_iter()
                .filter(|a| {
                    a.heeft_inhoud
                        && !NOT_FLEMISH.contains(&a.wetgeving_document_type.as_str())
                        && selected(&a.id.to_string())
                })
                .map(|a| Target {
                    key: a.id.to_string(),
                    url: CodexClient::page_url(a.id),
                    text_uid: format!("BE-VLG/{}", a.id),
                    title: flemish_title(&a),
                    title_key: None,
                    nature: flemish_nature(&a.wetgeving_document_type).to_string(),
                    current: None,
                    justel: None,
                    aliases: Vec::new(),
                })
                .collect();
            return Ok(vec![Lane {
                publisher: Publisher::Codex(client),
                targets,
            }]);
        }
        NormsCountry::Nl => {
            let client = BwbClient::new(proxy)?;
            let targets = bwb::targets(client.listing().await?, &selected);
            return Ok(vec![Lane {
                publisher: Publisher::Bwb(client),
                targets,
            }]);
        }
        NormsCountry::Dj => {
            let posts = ids(only, "an eJO text is keyed by its post id")?;
            return Ok(vec![Lane {
                publisher: Publisher::Ejo(EjoClient::new(proxy)?, posts),
                targets: Vec::new(),
            }]);
        }
        NormsCountry::Mg => {
            let ids = ids(only, "a CNLEGIS text is keyed by its id")?;
            return Ok(vec![Lane {
                publisher: Publisher::Cnlegis(CnlegisClient::new(proxy)?, ids),
                targets: Vec::new(),
            }]);
        }
        NormsCountry::Bj => {
            return Ok(vec![Lane {
                publisher: Publisher::Anbenin(AnbeninClient::new(proxy)?),
                targets: Vec::new(),
            }]);
        }
        NormsCountry::Ma => {
            return Ok(vec![Lane {
                publisher: Publisher::Adala(AdalaClient::new(proxy)?),
                targets: Vec::new(),
            }]);
        }
        NormsCountry::Sn | NormsCountry::Ohada => {
            let place = match country {
                NormsCountry::Sn => "sn",
                _ => "aa-ohada",
            };
            return Ok(vec![Lane {
                publisher: Publisher::Senlii(SenliiClient::new(proxy)?, place),
                targets: Vec::new(),
            }]);
        }
        NormsCountry::BeFed | NormsCountry::BeBru | NormsCountry::BeCfr | NormsCountry::BeCde => {
            let government = match country {
                NormsCountry::BeFed => &belgium::FEDERAL,
                NormsCountry::BeBru => &belgium::BRUSSELS,
                NormsCountry::BeCfr => &belgium::FRENCH_COMMUNITY,
                _ => &belgium::GERMAN_COMMUNITY,
            };
            let followed: HashSet<&str> = BELGIUM
                .iter()
                .flat_map(|b| b.documents.iter().map(|d| d.numac))
                .collect();
            let client = JustelClient::new(proxy)?;
            let acts = justel_listing(&client, government, cache, relist).await?;
            return Ok(vec![Lane {
                targets: justel_targets(
                    acts,
                    government,
                    &|numac| selected(numac) && !followed.contains(numac),
                    cache,
                ),
                publisher: Publisher::Justel(client),
            }]);
        }
        NormsCountry::BeWal => {
            let targets = match only {
                Some(id) => vec![wallex_target(
                    id.parse()
                        .map_err(|e| anyhow!("not a Wallex number: {id:?}: {e}"))?,
                )],
                None => Vec::new(),
            };
            return Ok(vec![Lane {
                publisher: Publisher::Wallex(WallexClient::new(proxy)?),
                targets,
            }]);
        }
        NormsCountry::Be => {
            return Ok(vec![Lane {
                publisher: Publisher::Justel(JustelClient::new(proxy)?),
                targets: BELGIUM
                    .iter()
                    .filter(|b| selected(b.followed.key))
                    .map(|b| Target {
                        justel: Some(JustelRead::Followed(b)),
                        ..Target::from(&b.followed)
                    })
                    .collect(),
            }])
        }
    };
    let (_, table, _) = country.followed();
    Ok(vec![Lane {
        publisher,
        targets: table
            .iter()
            .filter(|f| selected(f.key))
            .map(Target::from)
            .chain(listed)
            .collect(),
    }])
}

enum Publisher {
    Fedlex(FedlexClient),
    /// The client and the states its index gave each act, the codes [`LUXEMBOURG`] follows aside.
    Legilux(LegiluxClient, legilux::Index),
    JusticeLaws(JusticeLawsClient, justice_laws::Index),
    Elaws(ElawsClient, elaws::Index),
    Lexwork(LexworkClient),
    Justel(JustelClient),
    Codex(CodexClient),
    Wallex(WallexClient),
    Vd(VdClient),
    Lexfind(LexfindClient),
    /// The client and the posts asked by `--only`, read whatever their visit.
    Ejo(EjoClient, Vec<u64>),
    /// The client and the texts asked by `--only`, by CNLEGIS id, read whatever their visit.
    Cnlegis(CnlegisClient, Vec<u64>),
    Anbenin(AnbeninClient),
    Adala(AdalaClient),
    /// The client and the place of the works it reads.
    Senlii(SenliiClient, &'static str),
    Bwb(BwbClient),
}

impl Publisher {
    /// The source a run writes under: the cantons share LexWork's, whichever collection a
    /// canton is read from, so that a run on one canton finds the visits of a run on all.
    fn name(&self) -> &'static str {
        match self {
            Self::Fedlex(_) => "fedlex",
            Self::Legilux(..) => "legilux",
            Self::JusticeLaws(..) => "justice_laws",
            Self::Elaws(..) => elaws::SOURCE,
            Self::Lexwork(_) => "lexwork",
            Self::Justel(_) => "ejustice-be",
            Self::Codex(_) => "vlaamse-codex",
            Self::Wallex(_) => "wallex",
            Self::Vd(_) | Self::Lexfind(_) => "lexwork",
            Self::Ejo(..) => ejo::SOURCE,
            Self::Cnlegis(..) => cnlegis::SOURCE,
            Self::Anbenin(_) => anbenin::SOURCE,
            Self::Adala(_) => adala::SOURCE,
            Self::Senlii(..) => senlii::SOURCE,
            Self::Bwb(_) => bwb::SOURCE,
        }
    }

    async fn listing(&self, key: &str) -> Result<Listing> {
        let consolidations = match self {
            Self::Fedlex(c) => c.consolidations(key).await?,
            Self::Legilux(c, index) => match index.get(key) {
                Some(act) => {
                    return Ok(Listing {
                        adopted: act.adopted.clone(),
                        consolidations: act.states.clone(),
                        abbreviation: None,
                    })
                }
                None => c.consolidations(key).await?,
            },
            Self::JusticeLaws(..) => bail!("a Justice Laws text is listed in its collection"),
            Self::Elaws(..) => bail!("an e-Laws text is listed in the sitemaps"),
            Self::Lexwork(c) => {
                let (_, number) = key
                    .split_once('/')
                    .ok_or_else(|| anyhow!("not a cantonal key: {key:?}"))?;
                return Ok(c.listing(number).await?);
            }
            Self::Justel(_) => bail!("a Justel text is read by its documents, not listed"),
            Self::Codex(_) => bail!("a Codex act is read by its states, not listed"),
            Self::Wallex(_) => bail!("a Wallex act is read by its page, not listed"),
            Self::Vd(c) => {
                let (cote, id) = key
                    .strip_prefix("VD/")
                    .and_then(|rest| rest.rsplit_once('/'))
                    .ok_or_else(|| anyhow!("not a Vaud key: {key:?}"))?;
                return Ok(c.listing(id, cote).await?);
            }
            Self::Lexfind(c) => {
                let id = key
                    .rsplit_once('/')
                    .and_then(|(_, id)| id.parse().ok())
                    .ok_or_else(|| anyhow!("not a LexFind key: {key:?}"))?;
                return Ok(c.listing(id).await?);
            }
            Self::Ejo(..) => bail!("an eJO text is read by hundreds, not listed"),
            Self::Cnlegis(..) => bail!("a CNLEGIS text is read by its type's listing"),
            Self::Anbenin(_) => bail!("an Assembly law is read by the collection's listing"),
            Self::Adala(_) => bail!("an Adala file is read by its folders' listing"),
            Self::Senlii(..) => bail!("a SenegalLII work is read by the sitemap"),
            Self::Bwb(_) => bail!("a regeling is read by its manifest"),
        };
        Ok(Listing {
            adopted: None,
            consolidations,
            abbreviation: None,
        })
    }

    async fn xml(&self, url: &str) -> Result<String> {
        Ok(match self {
            Self::Fedlex(c) => c.xml(url).await?,
            Self::Legilux(c, _) => c.xml(url).await?,
            Self::JusticeLaws(c, _) => c.xml(url).await?,
            Self::Elaws(..) => bail!("an e-Laws text is read by its versions: {url}"),
            Self::Lexwork(c) => c.xml(url).await?,
            Self::Vd(c) => c.html(url).await?,
            Self::Lexfind(c) => c.xml(url).await?,
            Self::Justel(_) => bail!("a Justel text is read by its documents: {url}"),
            Self::Codex(_) => bail!("a Codex act is read by its states: {url}"),
            Self::Wallex(_) => bail!("a Wallex act is read by its page: {url}"),
            Self::Ejo(..) => bail!("an eJO text is read by hundreds: {url}"),
            Self::Cnlegis(..) => bail!("a CNLEGIS text is read by its type's listing: {url}"),
            Self::Anbenin(_) => bail!("an Assembly law is read by the collection's listing: {url}"),
            Self::Adala(_) => bail!("an Adala file is read by its folders' listing: {url}"),
            Self::Senlii(..) => bail!("a SenegalLII work is read by the sitemap: {url}"),
            Self::Bwb(_) => bail!("a regeling is read by its manifest: {url}"),
        })
    }

    /// The articles of a state, their bodies normalised as the base stores them, so that a state
    /// read compares with one rebuilt from the base.
    fn articles(&self, url: &str, xml: &str) -> Result<Vec<ConsolidatedArticle>> {
        let parsed = match self {
            Self::Fedlex(_) => lj_parse::fedlex_articles(xml),
            Self::Legilux(..) => lj_parse::legilux_articles(xml),
            Self::JusticeLaws(..) => lj_parse::justice_laws_articles(xml),
            Self::Elaws(..) => bail!("an e-Laws text is read by its versions: {url}"),
            Self::Lexwork(c) => lj_parse::lexwork_articles(xml, c.canton.lang),
            Self::Vd(_) => lj_parse::vd_articles(xml),
            Self::Lexfind(c) => lj_parse::lexfind_articles(xml, c.canton.layout),
            Self::Justel(_) => bail!("a Justel text is read by its documents: {url}"),
            Self::Codex(_) => bail!("a Codex act is read by its states: {url}"),
            Self::Wallex(_) => bail!("a Wallex act is read by its page: {url}"),
            Self::Ejo(..) => bail!("an eJO text is read by hundreds: {url}"),
            Self::Cnlegis(..) => bail!("a CNLEGIS text is read by its type's listing: {url}"),
            Self::Anbenin(_) => bail!("an Assembly law is read by the collection's listing: {url}"),
            Self::Adala(_) => bail!("an Adala file is read by its folders' listing: {url}"),
            Self::Senlii(..) => bail!("a SenegalLII work is read by the sitemap: {url}"),
            Self::Bwb(_) => bail!("a regeling is read by its manifest: {url}"),
        };
        let mut articles = parsed.map_err(|e| anyhow!("{url}: {e}"))?;
        for a in &mut articles {
            a.texte = lj_core::normalizer::normalize_chars(&a.texte);
            a.nota = a.nota.as_deref().map(lj_core::normalizer::normalize_chars);
        }
        Ok(articles)
    }
}

/// The articles of a state on `day` as the stored versions give them, in their stored order. A
/// version ends the day before the next one starts, or on the day its article left the act.
fn stored_state(stored: &[StoredArticleVersion], day: NaiveDate) -> Vec<ConsolidatedArticle> {
    let mut present: Vec<(Option<i32>, ConsolidatedArticle)> = stored
        .chunk_by(|a, b| a.num == b.num)
        .filter_map(|versions| {
            let started = versions.partition_point(|v| v.date_debut <= day);
            let v = versions[..started].last()?;
            let next = versions.get(started).map(|n| n.date_debut);
            let holds = v
                .date_fin
                .is_none_or(|fin| day < fin || next == fin.succ_opt());
            holds.then(|| {
                (
                    v.position,
                    ConsolidatedArticle {
                        num: v.num.clone(),
                        title_path: v.title_path.clone(),
                        texte: v.texte.clone(),
                        nota: v.nota.clone(),
                    },
                )
            })
        })
        .collect();
    present.sort_by_key(|(position, _)| *position);
    present.into_iter().map(|(_, a)| a).collect()
}

/// The articles of a text across its dated states, oldest state first: an article gets a version
/// each time its text changes, open until the next change, closed on the day it leaves the act.
fn article_versions(states: &[(NaiveDate, Vec<ConsolidatedArticle>)]) -> Vec<CorpusArticle> {
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, &ConsolidatedArticle> = HashMap::new();
    for (_, arts) in states.iter().rev() {
        for a in arts {
            if !latest.contains_key(&a.num) {
                order.push(a.num.clone());
                latest.insert(a.num.clone(), a);
            }
        }
    }
    let by_state: Vec<HashMap<&str, &str>> = states
        .iter()
        .map(|(_, arts)| {
            arts.iter()
                .map(|a| (a.num.as_str(), a.texte.as_str()))
                .collect()
        })
        .collect();
    order
        .into_iter()
        .map(|num| {
            let mut versions: Vec<CorpusVersion> = Vec::new();
            let mut previous: Option<&str> = None;
            for ((day, _), texts) in states.iter().zip(&by_state) {
                let current = texts.get(num.as_str()).copied();
                if current == previous {
                    continue;
                }
                if let Some(open) = versions.last_mut().filter(|v| v.date_fin.is_none()) {
                    open.date_fin = Some(day.to_string());
                }
                if let Some(texte) = current {
                    versions.push(CorpusVersion {
                        date_debut: Some(day.to_string()),
                        date_fin: None,
                        status: article_status(texte).to_string(),
                        texte: texte.to_string(),
                        source: None,
                        source_url: None,
                        source_asof: None,
                        source_upstream_url: None,
                    });
                }
                previous = current;
            }
            let last = latest[&num];
            CorpusArticle {
                num_key: None,
                title_path: last.title_path.clone(),
                nota: last.nota.clone(),
                num,
                texte: None,
                texte_original: None,
                lang_original: None,
                translation: None,
                versions: Some(versions),
            }
        })
        .collect()
}

/// The publisher still refuses after its pauses: the lane's next text would be refused too.
fn refused(e: &anyhow::Error) -> bool {
    matches!(e.downcast_ref(), Some(SourceError::Refused { .. }))
}

/// What every lane of a run shares.
struct Run<'a> {
    source: &'static str,
    jurisdiction: &'static str,
    detect_abrogation: bool,
    relist_before: Option<DateTime<Utc>>,
    read_on: NaiveDate,
    dry_run: bool,
    visits: &'a HashMap<String, NormVisit>,
    pool: &'a lj_pg::db::Pool,
    cache: std::path::PathBuf,
    /// Past it, an act never visited is left to the next run.
    deadline: Option<std::time::Instant>,
}

#[derive(Default)]
struct Tally {
    listed: usize,
    unchanged: usize,
    written: usize,
    repaired: Vec<String>,
    failed: Vec<String>,
    publie_targets: HashSet<String>,
}

impl Run<'_> {
    /// A text whose index names the state its last visit read, visited since the relist bound; a
    /// Codex, Wallex, Fedlex, Vaud or LexFind act visited and not among those its publisher
    /// changed since.
    fn unchanged(&self, t: &Target, changed: Option<&HashSet<String>>) -> bool {
        if let Some(changed) = changed {
            return self.visits.contains_key(&t.key) && !changed.contains(&t.key);
        }
        let (Some(current), Some(visit)) = (t.current, self.visits.get(&t.key)) else {
            return false;
        };
        visit.content_checksum == current
            && self
                .relist_before
                .is_none_or(|before| visit.visited_at >= before)
    }

    /// Reads the texts of a lane in turn. A text that fails is reported and the next one read; a
    /// publisher that still refuses after its pauses ends the lane.
    async fn lane(&self, lane: Lane, limit: Option<usize>) -> Tally {
        let mut tally = Tally::default();
        let Lane {
            publisher,
            mut targets,
        } = lane;
        if let Publisher::Ejo(client, posts) = &publisher {
            return self.ejo_lane(client, posts, limit).await;
        }
        if let Publisher::Cnlegis(client, ids) = &publisher {
            return self.cnlegis_lane(client, ids, limit).await;
        }
        if let Publisher::Anbenin(client) = &publisher {
            return self.anbenin_lane(client, limit).await;
        }
        if let Publisher::Adala(client) = &publisher {
            return self.adala_lane(client, limit).await;
        }
        if let Publisher::Senlii(client, place) = &publisher {
            return self.senlii_lane(client, place, limit).await;
        }
        if let Publisher::Bwb(client) = &publisher {
            return self.bwb_lane(client, &targets, limit).await;
        }
        if let Publisher::Elaws(client, index) = &publisher {
            return self.elaws_lane(client, index, &targets, limit).await;
        }
        let changed = match &publisher {
            Publisher::Codex(client) => Some(self.codex_changed(client).await),
            Publisher::Wallex(client) => Some(self.wallex_changed(client, &mut targets).await),
            Publisher::Vd(_) => Some(Ok(self.listed_moved(&targets))),
            Publisher::JusticeLaws(_, index) => Some(Ok(self.justice_laws_moved(index, &targets))),
            Publisher::Fedlex(client) => self.fedlex_changed(client).await.transpose(),
            Publisher::Lexfind(client) => self.lexfind_changed(client, &targets).await.transpose(),
            Publisher::Justel(client)
                if targets
                    .iter()
                    .any(|t| matches!(t.justel, Some(JustelRead::Listed(_)))) =>
            {
                Some(self.justel_changed(client, &targets).await)
            }
            _ => None,
        };
        let changed = match changed.transpose() {
            Ok(changed) => changed,
            Err(e) => {
                tracing::error!(publisher = self.source, error = %e, "changes not listed");
                tally.failed.push("changes".to_string());
                return tally;
            }
        };
        let mut left = 0usize;
        let mut unpublished: Vec<&Target> = Vec::new();
        for t in &targets {
            if limit.is_some_and(|l| tally.listed >= l) {
                break;
            }
            if self.unchanged(t, changed.as_ref()) {
                tally.unchanged += 1;
                if let Publisher::Fedlex(client) = &publisher {
                    if self.compilation_due(t) {
                        if let Err(e) = self.compilation_read(client, t, &mut tally).await {
                            tracing::error!(publisher = self.source, key = t.key, error = %e, "RO acts failed");
                            tally.failed.push(publications::compilation_item(&t.key));
                            if refused(&e) {
                                break;
                            }
                        }
                    }
                    if self.expressions_due(t) {
                        if let Err(e) = self.expressions(client, t, false).await {
                            tracing::error!(publisher = self.source, key = t.key, error = %e, "German and Italian texts failed");
                            tally.failed.push(expressions::text_item(&t.key, "de-it"));
                            if refused(&e) {
                                break;
                            }
                        }
                    }
                }
                if matches!(publisher, Publisher::Codex(_) | Publisher::Wallex(_))
                    && !self
                        .visits
                        .contains_key(&publications::moniteur_item(&t.key))
                {
                    unpublished.push(t);
                }
                continue;
            }
            if self.left_for_later(&t.key) {
                left += 1;
                continue;
            }
            tally.listed += 1;
            if let Err(e) = self.text(&publisher, t, &mut tally).await {
                tracing::error!(publisher = self.source, key = t.key, error = %e, "followed text failed");
                tally.failed.push(t.key.clone());
                if refused(&e) {
                    break;
                }
            }
        }
        let lane = targets
            .first()
            .map_or("", |t| t.key.split('/').next().unwrap_or_default());
        if !self.dry_run {
            match self.add_aliases(&targets).await {
                Ok(0) => {}
                Ok(added) => tracing::warn!(
                    publisher = self.source,
                    lane,
                    added,
                    "text aliases its reads had not written, added"
                ),
                Err(e) => {
                    tracing::error!(publisher = self.source, lane, error = %e, "text aliases not written");
                    tally.failed.push(format!("{lane} aliases"));
                }
            }
        }
        if !self.dry_run && !unpublished.is_empty() {
            if let Err(e) = self.moniteur_catch_up(&publisher, &unpublished).await {
                tracing::error!(publisher = self.source, error = %e, "Moniteur publications not written");
                tally.failed.push("publications".to_string());
            }
        }
        if !self.dry_run && matches!(publisher, Publisher::Justel(_)) {
            let visited: Vec<&Target> = targets
                .iter()
                .filter(|t| self.visits.contains_key(&t.key))
                .collect();
            if let Err(e) = self.moniteur_publications(&visited).await {
                tracing::error!(publisher = self.source, lane, error = %e, "Moniteur publications not written");
                tally.failed.push(format!("{lane} publications"));
            }
        }
        self.budget_spent(lane, left);
        tally
    }

    /// Whether the RO acts of `t` were never read, and the time budget lets them be.
    fn compilation_due(&self, t: &Target) -> bool {
        let item = publications::compilation_item(&t.key);
        !self.visits.contains_key(&item) && !self.left_for_later(&item)
    }

    /// Whether the German or Italian texts of `t` were never hung, and the time budget lets them be.
    fn expressions_due(&self, t: &Target) -> bool {
        expressions::LANGUAGES.iter().any(|language| {
            let item = expressions::text_item(&t.key, language);
            !self.visits.contains_key(&item) && !self.left_for_later(&item)
        })
    }

    /// Hangs the German and Italian texts of a federal act on its French versions: each state from
    /// the cache, else from Fedlex. `rehang` rewrites a language whose states did not move.
    async fn expressions(&self, client: &FedlexClient, t: &Target, rehang: bool) -> Result<()> {
        let listed = client.expressions(&t.key, &expressions::LANGUAGES).await?;
        let mut read: Vec<(String, u64)> = Vec::new();
        let mut hung: Vec<(&str, Vec<expressions::State>)> = Vec::new();
        for language in expressions::LANGUAGES {
            let consolidations = listed.get(language).map_or(&[][..], Vec::as_slice);
            let mut states = Vec::with_capacity(consolidations.len());
            let mut fetched = 0usize;
            let mut moved = 0usize;
            for c in consolidations {
                let day = NaiveDate::parse_from_str(&c.applies_from, "%Y-%m-%d")
                    .map_err(|e| anyhow!("{} state {:?}: {e}", t.key, c.applies_from))?;
                let item = expressions::state_item(&t.key, day, language);
                let visit = self.visits.get(&item);
                let modified = visit.is_some() && reason(c.modified, visit, None).is_some();
                let cached = if modified {
                    None
                } else {
                    expressions::cached(&self.cache, "fedlex", &t.key, day, language)?
                };
                let xml = match cached {
                    Some(xml) => xml,
                    None => {
                        let xml = client.xml(&c.xml_url).await?;
                        expressions::cache(&self.cache, "fedlex", &t.key, day, language, &xml)?;
                        fetched += 1;
                        xml
                    }
                };
                let checksum = xxhash_rust::xxh3::xxh3_64(xml.as_bytes());
                if visit.is_none_or(|v| v.content_checksum != checksum) {
                    moved += 1;
                    read.push((item, checksum));
                }
                let articles = lj_parse::fedlex_articles(&xml)
                    .map_err(|e| anyhow!("{} {language} state {day}: {e}", t.key))?;
                states.push(expressions::State {
                    day,
                    checksum,
                    articles,
                });
            }
            let days: String = consolidations
                .iter()
                .map(|c| c.applies_from.as_str())
                .collect();
            let item = expressions::text_item(&t.key, language);
            let days = xxhash_rust::xxh3::xxh3_64(days.as_bytes());
            let hung_before = self
                .visits
                .get(&item)
                .is_some_and(|v| v.content_checksum == days);
            tracing::info!(
                publisher = self.source,
                key = t.key,
                language,
                states = states.len(),
                fetched,
                moved,
                "federal act's states in another language read"
            );
            if moved > 0 || rehang || !hung_before {
                read.push((item, days));
                hung.push((language, states));
            }
        }
        if self.dry_run || read.is_empty() {
            return Ok(());
        }
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let versions: Vec<expressions::Version> = repo
            .article_version_ends(&t.text_uid)
            .await
            .map_err(|e| anyhow!("article_version_ends {}: {e}", t.text_uid))?
            .into_iter()
            .map(|(id, num, date_fin)| expressions::Version { id, num, date_fin })
            .collect();
        for (language, states) in &hung {
            let rows: Vec<ArticleLanguageWrite> = expressions::hang(&versions, states)
                .into_iter()
                .map(|(article_id, a, checksum)| ArticleLanguageWrite {
                    article_id,
                    text_status: "official".to_string(),
                    texte: a.texte.clone(),
                    title_path: a.title_path.clone(),
                    content_checksum: i64::from_ne_bytes(checksum.to_ne_bytes()),
                })
                .collect();
            let written = repo
                .replace_language_texts(&t.text_uid, language, &rows)
                .await
                .map_err(|e| anyhow!("replace_language_texts {} {language}: {e}", t.text_uid))?;
            tracing::info!(
                publisher = self.source,
                key = t.key,
                text_uid = t.text_uid,
                language,
                versions = versions.len(),
                hung = rows.len(),
                written,
                "federal act's versions hung in another language"
            );
        }
        repo.put_norm_visits(self.source, &read)
            .await
            .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))
    }

    /// The RO acts of a Swiss federal act whose consolidation did not change.
    async fn compilation_read(
        &self,
        client: &FedlexClient,
        t: &Target,
        tally: &mut Tally,
    ) -> Result<()> {
        let acts = client.compilation_acts(&t.key).await?;
        let (rows, facts, links) = publications::compilation(&acts)?;
        if self.dry_run {
            return Ok(());
        }
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        self.write_compilation(&repo, t, &rows, &facts).await?;
        write_text_links(&repo, &t.text_uid, &links, &mut tally.publie_targets).await?;
        let checksum = xxhash_rust::xxh3::xxh3_64(format!("{acts:?}").as_bytes());
        repo.put_norm_visits(
            self.source,
            &[(publications::compilation_item(&t.key), checksum)],
        )
        .await
        .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))
    }

    async fn write_compilation(
        &self,
        repo: &DecisionRepository<'_>,
        t: &Target,
        rows: &[PublicationRow],
        facts: &[serde_json::Value],
    ) -> Result<()> {
        repo.upsert_publications(publications::RO, rows)
            .await
            .map_err(|e| anyhow!("upsert_publications {}: {e}", t.text_uid))?;
        repo.upsert_publication_facts(publications::RO, facts)
            .await
            .map_err(|e| anyhow!("upsert_publication_facts {}: {e}", t.text_uid))?;
        tracing::info!(
            publisher = self.source,
            key = t.key,
            acts = rows.len(),
            "RO acts written"
        );
        Ok(())
    }

    /// The Moniteur publications of Flemish and Walloon acts read before their lane wrote them: a
    /// Flemish act's from the latest state the cache holds, else from a request, made only in a
    /// run given a time budget, since the chain's Codex and Wallex steps have none and run at night.
    async fn moniteur_catch_up(&self, publisher: &Publisher, targets: &[&Target]) -> Result<()> {
        let cache = self.cache.join("codex");
        let (mut rows, mut visits) = (Vec::new(), Vec::new());
        let (mut written, mut asked, mut left, mut failed) = (0usize, 0usize, 0usize, 0usize);
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        for t in targets {
            let budget = self.deadline.is_some_and(|d| std::time::Instant::now() < d);
            let row = match self
                .moniteur_read(publisher, &cache, t, budget, &mut asked)
                .await
            {
                Ok(row) => row,
                Err(e) if refused(&e) => {
                    self.write_moniteur(&repo, rows, &visits).await?;
                    return Err(e);
                }
                Err(e) => {
                    tracing::warn!(publisher = self.source, key = t.key, error = %e, "Moniteur publication not read");
                    failed += 1;
                    continue;
                }
            };
            let Some(row) = row else {
                left += 1;
                continue;
            };
            visits.push(moniteur_visit(&t.key, &row));
            rows.push(row);
            if rows.len() == 500 {
                written += rows.len();
                self.write_moniteur(&repo, std::mem::take(&mut rows), &visits)
                    .await?;
                visits.clear();
            }
        }
        written += rows.len();
        self.write_moniteur(&repo, rows, &visits).await?;
        if written + left + failed > 0 {
            tracing::warn!(
                publisher = self.source,
                written,
                asked,
                left,
                failed,
                "Moniteur publications of acts read before, written"
            );
        }
        Ok(())
    }

    /// The Moniteur publication of one act read before, `None` when it waits for a run with a budget.
    async fn moniteur_read(
        &self,
        publisher: &Publisher,
        cache: &std::path::Path,
        t: &Target,
        budget: bool,
        asked: &mut usize,
    ) -> Result<Option<PublicationRow>> {
        Ok(match publisher {
            Publisher::Codex(client) => {
                let id: u64 = t
                    .key
                    .parse()
                    .map_err(|e| anyhow!("codex key {}: {e}", t.key))?;
                let state = match codex::cached_latest(cache, id)? {
                    Some(state) => state,
                    None if budget => {
                        *asked += 1;
                        lj_parse::codex_state(&client.state(id, None).await?)
                            .map_err(|e| anyhow!("codex {id}: {e}"))?
                    }
                    None => return Ok(None),
                };
                Some(publications::moniteur(
                    &t.text_uid,
                    &t.title,
                    &t.nature,
                    &t.url,
                    state.adopted.as_deref(),
                    state.published.as_deref(),
                )?)
            }
            Publisher::Wallex(client) if budget => {
                *asked += 1;
                let page = wallex::page(&client.page(&t.url).await?, &t.url)?;
                Some(wallex_publication(t, &page)?)
            }
            _ => None,
        })
    }

    /// Writes publications with the visits that keep them.
    async fn write_moniteur(
        &self,
        repo: &DecisionRepository<'_>,
        rows: Vec<PublicationRow>,
        visits: &[(String, u64)],
    ) -> Result<()> {
        let mut by_journal: HashMap<String, Vec<PublicationRow>> = HashMap::new();
        for r in rows {
            by_journal
                .entry(r.journal_code.clone())
                .or_default()
                .push(r);
        }
        for (journal, rows) in &by_journal {
            repo.upsert_publications(publications::moniteur_edition(journal), rows)
                .await
                .map_err(|e| anyhow!("upsert_publications {journal}: {e}"))?;
        }
        repo.put_norm_visits(self.source, visits)
            .await
            .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))
    }

    /// The Moniteur publications of the acts Justel lists among `targets`.
    async fn moniteur_publications(&self, targets: &[&Target]) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        let mut by_journal: HashMap<String, Vec<PublicationRow>> = HashMap::new();
        for t in targets {
            if let Some(JustelRead::Listed(act)) = &t.justel {
                let row = publications::moniteur(
                    &t.text_uid,
                    &t.title,
                    &t.nature,
                    &t.url,
                    act.adopted.as_deref(),
                    act.published.as_deref(),
                )?;
                by_journal
                    .entry(row.journal_code.clone())
                    .or_default()
                    .push(row);
            }
        }
        if by_journal.is_empty() {
            return Ok(());
        }
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        for (journal, rows) in &by_journal {
            repo.upsert_publications(publications::moniteur_edition(journal), rows)
                .await
                .map_err(|e| anyhow!("upsert_publications {journal}: {e}"))?;
        }
        Ok(())
    }

    /// Whether `item`, never visited, is left to the next run, the time budget spent.
    fn left_for_later(&self, item: &str) -> bool {
        !self.visits.contains_key(item)
            && self
                .deadline
                .is_some_and(|d| std::time::Instant::now() >= d)
    }

    fn budget_spent(&self, lane: &str, left: usize) {
        if left > 0 {
            tracing::warn!(
                publisher = self.source,
                lane,
                left,
                "acts never visited left to the next run, the time budget spent"
            );
        }
    }

    /// Adds the names of the lane's texts to those the base holds, a text read before them
    /// getting them without a read.
    async fn add_aliases(&self, targets: &[Target]) -> Result<u64> {
        let names: Vec<(&str, Vec<TextAlias>)> = targets
            .iter()
            .map(|t| (t.text_uid.as_str(), aliases(t)))
            .collect();
        let rows: Vec<(&str, &TextAlias)> = names
            .iter()
            .flat_map(|(uid, names)| names.iter().map(move |a| (*uid, a)))
            .collect();
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        DecisionRepository::new(&conn)
            .put_text_aliases(&[], &rows)
            .await
            .map_err(|e| anyhow!("put_text_aliases: {e}"))
    }

    /// The listed acts Justel's feed says it consolidated since the day before the latest visit:
    /// the act of the day and heading the feed names, else every act of that day.
    async fn justel_changed(
        &self,
        client: &JustelClient,
        targets: &[Target],
    ) -> Result<HashSet<String>> {
        let feed = client.consolidated().await?;
        let since = self.changes_since().map(|d| d.date_naive().to_string());
        if let (Some(since), Some(oldest)) = (&since, feed.last()) {
            if oldest.day > *since {
                tracing::warn!(
                    publisher = self.source,
                    since,
                    oldest = oldest.day,
                    "the feed of recent consolidations no longer reaches back to the latest visit"
                );
            }
        }
        let listed: Vec<(&Target, &JustelListed)> = targets
            .iter()
            .filter_map(|t| match &t.justel {
                Some(JustelRead::Listed(act)) => Some((t, act)),
                _ => None,
            })
            .collect();
        let mut changed = HashSet::new();
        for c in feed
            .iter()
            .filter(|c| since.as_ref().is_none_or(|s| c.day >= *s))
            .filter(|c| c.adopted.is_some())
        {
            let of_day: Vec<&(&Target, &JustelListed)> = listed
                .iter()
                .filter(|(_, a)| a.adopted == c.adopted)
                .collect();
            let named: Vec<&Target> = of_day
                .iter()
                .filter(|(_, a)| a.heading == c.heading)
                .map(|(t, _)| *t)
                .collect();
            match named.is_empty() {
                true => changed.extend(of_day.iter().map(|(t, _)| t.key.clone())),
                false => changed.extend(named.iter().map(|t| t.key.clone())),
            }
        }
        Ok(changed)
    }

    /// The day before the latest visit, or the relist bound: a publisher's changes since are read.
    fn changes_since(&self) -> Option<DateTime<Utc>> {
        match (
            self.relist_before,
            self.visits.values().map(|v| v.visited_at).max(),
        ) {
            (Some(before), _) => Some(before),
            (None, latest) => latest.map(|l| l - chrono::Duration::days(1)),
        }
    }

    /// The acts never visited or whose listed record moved since their visit: a relist reads no
    /// more, the record naming the day the act's current version came into force.
    fn listed_moved(&self, targets: &[Target]) -> HashSet<String> {
        targets
            .iter()
            .filter(|t| {
                t.current.is_some_and(|current| {
                    self.visits
                        .get(&t.key)
                        .is_none_or(|v| v.content_checksum != current)
                })
            })
            .map(|t| t.key.clone())
            .collect()
    }

    /// The instruments whose folder changed in either language since their visit and, on a
    /// relist, those listed longest ago.
    fn justice_laws_moved(
        &self,
        index: &justice_laws::Index,
        targets: &[Target],
    ) -> HashSet<String> {
        let visited = |item: &str| self.visits.get(item).map(|v| v.visited_at.date_naive());
        let mut moved: HashSet<String> = targets
            .iter()
            .filter(|t| {
                let Some(i) = index.get(&t.key) else {
                    return false;
                };
                let english = expressions::text_item(&t.key, "en");
                visited(&t.key).is_none_or(|day| i.changed >= day)
                    || i.english_changed
                        .is_some_and(|c| visited(&english).is_none_or(|day| c >= day))
            })
            .map(|t| t.key.clone())
            .collect();
        if self.relist_before.is_some() {
            let before = Utc::now() - chrono::Duration::days(justice_laws::RELIST_AFTER_DAYS);
            let mut due: Vec<(DateTime<Utc>, &str)> = targets
                .iter()
                .filter_map(|t| Some((self.visits.get(&t.key)?.visited_at, t.key.as_str())))
                .filter(|(at, _)| *at < before)
                .collect();
            due.sort();
            moved.extend(
                due.into_iter()
                    .take(justice_laws::RELIST_AT_MOST)
                    .map(|(_, key)| key.to_string()),
            );
        }
        moved
    }

    /// A Justice Laws state from the run's files when `kept` allows it, else from the FTP host,
    /// then kept.
    async fn justice_laws_xml(
        &self,
        client: &JusticeLawsClient,
        key: &str,
        day: NaiveDate,
        language: &str,
        path: &str,
        kept: bool,
    ) -> Result<String> {
        if kept {
            if let Some(xml) =
                expressions::cached(&self.cache, justice_laws::CACHE, key, day, language)?
            {
                return Ok(xml);
            }
        }
        let xml = client.xml(path).await?;
        expressions::cache(&self.cache, justice_laws::CACHE, key, day, language, &xml)?;
        Ok(xml)
    }

    /// Hangs the English text of a federal instrument on its French versions, each state kept
    /// unless its folder changed since its visit. `rehang` rewrites states that did not move.
    async fn justice_laws_english(
        &self,
        client: &JusticeLawsClient,
        index: &justice_laws::Index,
        t: &Target,
        rehang: bool,
    ) -> Result<()> {
        const LANGUAGE: &str = "en";
        let Some((collection, folder)) = index.get(&t.key).and_then(|i| i.english.as_ref()) else {
            return Ok(());
        };
        let consolidations = client.consolidations(collection, folder).await?;
        let (mut states, mut read, mut moved) =
            (Vec::with_capacity(consolidations.len()), Vec::new(), 0usize);
        for c in &consolidations {
            let day = NaiveDate::parse_from_str(&c.applies_from, "%Y-%m-%d")
                .map_err(|e| anyhow!("{} {LANGUAGE} state {:?}: {e}", t.key, c.applies_from))?;
            let token = c
                .token
                .ok_or_else(|| anyhow!("{} {LANGUAGE} state {day} has no change stamp", t.key))?;
            let item = expressions::state_item(&t.key, day, LANGUAGE);
            let visit = self.visits.get(&item);
            let fresh = visit.is_none_or(|v| v.content_checksum != token);
            let changed = visit.is_some() && fresh;
            let xml = self
                .justice_laws_xml(client, &t.key, day, LANGUAGE, &c.xml_url, !changed)
                .await?;
            if fresh {
                moved += 1;
                read.push((item, token));
            }
            let mut articles = lj_parse::justice_laws_articles(&xml)
                .map_err(|e| anyhow!("{} {LANGUAGE} state {day}: {e}", t.key))?;
            for a in &mut articles {
                a.texte = lj_core::normalizer::normalize_chars(&a.texte);
            }
            states.push(expressions::State {
                day,
                checksum: token,
                articles,
            });
        }
        let item = expressions::text_item(&t.key, LANGUAGE);
        let days: String = consolidations
            .iter()
            .map(|c| c.applies_from.as_str())
            .collect();
        let days = xxhash_rust::xxh3::xxh3_64(days.as_bytes());
        let hung_before = self
            .visits
            .get(&item)
            .is_some_and(|v| v.content_checksum == days);
        tracing::info!(
            publisher = self.source,
            key = t.key,
            language = LANGUAGE,
            states = states.len(),
            moved,
            "federal instrument's English states read"
        );
        if self.dry_run || !(moved > 0 || rehang || !hung_before) {
            return Ok(());
        }
        read.push((item, days));
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let versions: Vec<expressions::Version> = repo
            .article_version_ends(&t.text_uid)
            .await
            .map_err(|e| anyhow!("article_version_ends {}: {e}", t.text_uid))?
            .into_iter()
            .map(|(id, num, date_fin)| expressions::Version { id, num, date_fin })
            .collect();
        let rows: Vec<ArticleLanguageWrite> = expressions::hang(&versions, &states)
            .into_iter()
            .map(|(article_id, a, checksum)| ArticleLanguageWrite {
                article_id,
                text_status: "official".to_string(),
                texte: a.texte.clone(),
                title_path: a.title_path.clone(),
                content_checksum: i64::from_ne_bytes(checksum.to_ne_bytes()),
            })
            .collect();
        let written = repo
            .replace_language_texts(&t.text_uid, LANGUAGE, &rows)
            .await
            .map_err(|e| anyhow!("replace_language_texts {} {LANGUAGE}: {e}", t.text_uid))?;
        tracing::info!(
            publisher = self.source,
            key = t.key,
            text_uid = t.text_uid,
            language = LANGUAGE,
            versions = versions.len(),
            hung = rows.len(),
            written,
            "federal instrument's versions hung in English"
        );
        repo.put_norm_visits(self.source, &read)
            .await
            .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))
    }

    /// The acts with a consolidation Fedlex modified since; `None` on a first read, when every
    /// act is read.
    async fn fedlex_changed(&self, client: &FedlexClient) -> Result<Option<HashSet<String>>> {
        match self.changes_since() {
            Some(since) => Ok(Some(client.changed_since(since.date_naive()).await?)),
            None => Ok(None),
        }
    }

    /// The acts among the canton's changes LexFind lists since; `None` on a first read, when
    /// every act is read.
    async fn lexfind_changed(
        &self,
        client: &LexfindClient,
        targets: &[Target],
    ) -> Result<Option<HashSet<String>>> {
        let Some(since) = self.changes_since() else {
            return Ok(None);
        };
        let ids = client.changed_since(since.date_naive()).await?;
        Ok(Some(
            targets
                .iter()
                .filter(|t| {
                    t.key
                        .rsplit_once('/')
                        .and_then(|(_, id)| id.parse().ok())
                        .is_some_and(|id| ids.contains(&id))
                })
                .map(|t| t.key.clone())
                .collect(),
        ))
    }

    /// The acts Wallex changed since; an unkeyed lane reads the visited ones, or every act on a relist.
    async fn wallex_changed(
        &self,
        client: &WallexClient,
        targets: &mut Vec<Target>,
    ) -> Result<HashSet<String>> {
        let changed = match self.changes_since() {
            Some(since) => client.changed_since(since).await?,
            None => Default::default(),
        };
        if targets.is_empty() {
            let ids = if self.relist_before.is_some() || self.visits.is_empty() {
                client.listing(self.read_on).await?
            } else {
                changed
                    .iter()
                    .copied()
                    .filter(|id| self.visits.contains_key(&id.to_string()))
                    .collect()
            };
            tracing::info!(
                publisher = self.source,
                acts = ids.len(),
                "wallex acts listed"
            );
            *targets = ids.into_iter().map(wallex_target).collect();
        }
        Ok(changed.into_iter().map(|id| id.to_string()).collect())
    }

    async fn codex_changed(&self, client: &CodexClient) -> Result<HashSet<String>> {
        let Some(since) = self.changes_since() else {
            return Ok(HashSet::new());
        };
        Ok(client
            .changed_since(&since.date_naive().to_string())
            .await?
            .into_iter()
            .map(|a| a.id.to_string())
            .collect())
    }

    async fn text(&self, publisher: &Publisher, t: &Target, tally: &mut Tally) -> Result<()> {
        if let (Publisher::Justel(client), Some(read)) = (publisher, &t.justel) {
            return match read {
                JustelRead::Followed(text) => {
                    self.justel_text(client, t, text.documents, text.adopted, text.spaced, tally)
                        .await
                }
                JustelRead::Listed(act) => {
                    let document = JustelDocument {
                        numac: &act.numac,
                        cn: None,
                        born: None,
                        book: None,
                    };
                    self.justel_text(client, t, &[document], act.adopted.as_deref(), false, tally)
                        .await?;
                    self.moniteur_publications(&[t]).await
                }
            };
        }
        if let Publisher::Codex(client) = publisher {
            return self.codex_text(client, t, tally).await;
        }
        if let Publisher::Wallex(client) = publisher {
            return self.wallex_text(client, t, tally).await;
        }
        let source = self.source;
        let listing = match publisher {
            Publisher::JusticeLaws(client, index) => {
                let collection = index
                    .get(&t.key)
                    .ok_or_else(|| anyhow!("{} is not in the Justice Laws index", t.key))?
                    .french;
                Listing {
                    adopted: None,
                    consolidations: client.consolidations(collection, &t.key).await?,
                    abbreviation: None,
                }
            }
            _ => publisher.listing(&t.key).await?,
        };
        let mut states: Vec<(NaiveDate, Option<Vec<ConsolidatedArticle>>)> =
            Vec::with_capacity(listing.consolidations.len());
        let mut read: Vec<(String, u64)> = Vec::new();
        let mut moved = 0usize;
        for c in &listing.consolidations {
            let day = NaiveDate::parse_from_str(&c.applies_from, "%Y-%m-%d")
                .map_err(|e| anyhow!("{} state {:?}: {e}", t.key, c.applies_from))?;
            let item = format!("{}@{}", t.key, c.applies_from);
            let visit = self.visits.get(&item);
            let why = match (c.token, visit) {
                (Some(_), None) => Some(Reason::New),
                (Some(token), Some(v)) => (v.content_checksum != token).then_some(Reason::Modified),
                (None, _) => reason(c.modified, visit, self.relist_before),
            };
            let Some(why) = why else {
                states.push((day, None));
                continue;
            };
            let xml = match publisher {
                Publisher::JusticeLaws(client, _) => {
                    self.justice_laws_xml(client, &t.key, day, "fr", &c.xml_url, visit.is_none())
                        .await?
                }
                Publisher::Legilux(..) => match self.legilux_cached(&c.xml_url)? {
                    Some(xml) => xml,
                    None => publisher.xml(&c.xml_url).await?,
                },
                _ => publisher.xml(&c.xml_url).await?,
            };
            let checksum = c
                .token
                .unwrap_or_else(|| xxhash_rust::xxh3::xxh3_64(xml.as_bytes()));
            let articles = publisher.articles(&c.xml_url, &xml)?;
            if articles.is_empty() {
                tracing::warn!(
                    publisher = source,
                    key = t.key,
                    state = c.applies_from,
                    "a state without any article is left out"
                );
                continue;
            }
            if visit.is_none_or(|v| v.content_checksum != checksum) {
                moved += 1;
                if why == Reason::Relist {
                    tally.repaired.push(item.clone());
                }
            }
            states.push((day, Some(articles)));
            read.push((item, checksum));
        }
        tracing::info!(
            publisher = source,
            key = t.key,
            text_uid = t.text_uid,
            states = states.len(),
            read = read.len(),
            moved,
            "followed text listed"
        );
        if let Some(current) = t.current {
            read.push((t.key.clone(), current));
        }
        let compilation = match publisher {
            Publisher::Fedlex(client) if moved > 0 || self.compilation_due(t) => {
                Some(client.compilation_acts(&t.key).await?)
            }
            _ => None,
        };
        let (ro_rows, ro_facts, mut ro_links) = compilation
            .as_deref()
            .map(publications::compilation)
            .transpose()?
            .unwrap_or_default();
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        if moved > 0 {
            let stored = if states.iter().any(|(_, arts)| arts.is_none()) {
                repo.stored_article_versions(&t.text_uid)
                    .await
                    .map_err(|e| anyhow!("stored_article_versions {}: {e}", t.text_uid))?
            } else {
                Vec::new()
            };
            let states = states
                .into_iter()
                .map(|(day, arts)| {
                    let arts = match arts {
                        Some(arts) => arts,
                        None => stored_state(&stored, day),
                    };
                    if arts.is_empty() {
                        bail!(
                            "{} holds no article on {day}, a state its visits say was read",
                            t.text_uid
                        );
                    }
                    Ok((day, arts))
                })
                .collect::<Result<Vec<_>>>()?;
            let articles = article_versions(&states);
            let versions: usize = articles
                .iter()
                .map(|a| a.versions.as_ref().map_or(0, Vec::len))
                .sum();
            tracing::info!(
                publisher = source,
                key = t.key,
                text_uid = t.text_uid,
                first = ?states.first().map(|s| s.0),
                last = ?states.last().map(|s| s.0),
                latest_articles = states.last().map_or(0, |s| s.1.len()),
                articles = articles.len(),
                versions,
                "followed text rebuilt"
            );
            let doc = CorpusDoc {
                text_uid: t.text_uid.clone(),
                source: source.to_string(),
                jurisdiction: self.jurisdiction.to_string(),
                title: t.title.clone(),
                title_key: t.title_key.clone(),
                nature: t.nature.clone(),
                translation: "officiel".to_string(),
                source_url: Some(t.url.clone()),
                source_asof: None,
                source_upstream_url: None,
                date_texte: listing.adopted,
                effect_date: None,
                detect_abrogation: self.detect_abrogation,
                num_prefix_agnostic: false,
                articles,
                links: std::mem::take(&mut ro_links),
            };
            if !self.dry_run {
                load_doc(&repo, &doc, self.read_on, source, &mut tally.publie_targets).await?;
                tally.written += 1;
            }
        }
        if self.dry_run {
            if let Publisher::JusticeLaws(client, index) = publisher {
                self.justice_laws_english(client, index, t, false).await?;
            }
            return Ok(());
        }
        if let Some(acts) = &compilation {
            self.write_compilation(&repo, t, &ro_rows, &ro_facts)
                .await?;
            if moved == 0 {
                write_text_links(&repo, &t.text_uid, &ro_links, &mut tally.publie_targets).await?;
            }
            read.push((
                publications::compilation_item(&t.key),
                xxhash_rust::xxh3::xxh3_64(format!("{acts:?}").as_bytes()),
            ));
        }
        let listed = match (publisher, listing.abbreviation.as_deref()) {
            (Publisher::Lexwork(c), Some(names)) => text_aliases::in_list(c.canton.lang, names),
            _ => Vec::new(),
        };
        let names = [aliases(t), listed].concat();
        let rows: Vec<(&str, &TextAlias)> =
            names.iter().map(|a| (t.text_uid.as_str(), a)).collect();
        repo.put_text_aliases(&[&t.text_uid], &rows)
            .await
            .map_err(|e| anyhow!("put_text_aliases {}: {e}", t.text_uid))?;
        if let Publisher::Fedlex(_) | Publisher::JusticeLaws(..) = publisher {
            let states: String = listing
                .consolidations
                .iter()
                .map(|c| c.applies_from.as_str())
                .collect();
            read.push((t.key.clone(), xxhash_rust::xxh3::xxh3_64(states.as_bytes())));
        }
        repo.put_norm_visits(source, &read)
            .await
            .map_err(|e| anyhow!("put_norm_visits {source}: {e}"))?;
        if let Publisher::JusticeLaws(client, index) = publisher {
            self.justice_laws_english(client, index, t, moved > 0)
                .await?;
        }
        if let Publisher::Fedlex(client) = publisher {
            self.expressions(client, t, moved > 0).await?;
        }
        Ok(())
    }

    /// A Legilux file fetched ahead into the cache under a week ago, read once.
    fn legilux_cached(&self, url: &str) -> Result<Option<String>> {
        let path = legilux::cached(&self.cache, url);
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        let fresh = meta
            .modified()?
            .elapsed()
            .is_ok_and(|age| age < std::time::Duration::from_secs(7 * 86_400));
        let xml = fresh
            .then(|| zstd::decode_all(std::fs::File::open(&path)?))
            .transpose()?;
        if !self.dry_run || !fresh {
            std::fs::remove_file(&path)?;
        }
        xml.map(String::from_utf8).transpose().map_err(Into::into)
    }

    /// A document's current page, or its `archive`th: the one fetched ahead into the cache under
    /// a week ago, read once, else Justel's.
    async fn justel_page(
        &self,
        client: &JustelClient,
        d: &JustelDocument<'_>,
        archive: Option<usize>,
    ) -> Result<String> {
        let path = justel_cached(&self.cache, d.numac, archive);
        if let (None, Ok(meta)) = (d.cn, std::fs::metadata(&path)) {
            let fresh = meta
                .modified()?
                .elapsed()
                .is_ok_and(|age| age < std::time::Duration::from_secs(7 * 86_400));
            let html = fresh
                .then(|| zstd::decode_all(std::fs::File::open(&path)?))
                .transpose()?;
            if !self.dry_run || !fresh {
                std::fs::remove_file(&path)?;
            }
            if let Some(html) = html {
                return Ok(String::from_utf8(html)?);
            }
        }
        Ok(client
            .page(&JustelClient::url(d.numac, d.cn, archive))
            .await?)
    }

    /// A Belgian text: each document's current page, then the versions it archived since its
    /// last visit, or every version of every document when the base holds no history they extend.
    async fn justel_text(
        &self,
        client: &JustelClient,
        t: &Target,
        documents_read: &[JustelDocument<'_>],
        adopted: Option<&str>,
        spaced: bool,
        tally: &mut Tally,
    ) -> Result<()> {
        /// A document's current page, and whether the versions its visits hold continue in it.
        struct Current<'d> {
            document: &'d JustelDocument<'d>,
            archived: usize,
            repealed_on: Option<String>,
            articles: Vec<justel::Article>,
            checksum: u64,
            moved: bool,
            unread: usize,
            extends: bool,
        }
        let source = self.source;
        let archive_item = |cn: &str, k: usize| format!("{cn}@{k:03}");
        let mut documents = Vec::with_capacity(documents_read.len());
        let mut spellings = justel::Spellings::default();
        for d in documents_read {
            let url = JustelClient::url(d.numac, d.cn, None);
            let page = lj_parse::justel_page(&self.justel_page(client, d, None).await?)
                .map_err(|e| anyhow!("{url}: {e}"))?;
            let checksum = xxhash_rust::xxh3::xxh3_64(format!("{page:?}").as_bytes());
            if page.articles.is_empty() {
                match (&page.elsewhere, page.no_text, documents_read) {
                    (Some(holder), _, [_]) => tracing::info!(
                        publisher = source,
                        key = t.key,
                        holder,
                        "justel document sends to another for its text"
                    ),
                    (None, Some(why), [_]) => tracing::info!(
                        publisher = source,
                        key = t.key,
                        ?why,
                        "justel document shows no article"
                    ),
                    _ => bail!("{url} shows no article"),
                }
                if !self.dry_run {
                    let conn = self
                        .pool
                        .get()
                        .await
                        .map_err(|e| anyhow!("pool.get: {e}"))?;
                    DecisionRepository::new(&conn)
                        .put_norm_visits(source, &[(d.key().to_string(), checksum)])
                        .await
                        .map_err(|e| anyhow!("put_norm_visits {source}: {e}"))?;
                }
                return Ok(());
            }
            let moved = self
                .visits
                .get(d.key())
                .is_none_or(|v| v.content_checksum != checksum);
            let unread = (1..=page.archived)
                .find(|&k| !self.visits.contains_key(&archive_item(d.key(), k)))
                .unwrap_or(page.archived + 1);
            let extends = self.visits.contains_key(d.key())
                && (unread..=page.archived)
                    .all(|k| !self.visits.contains_key(&archive_item(d.key(), k)))
                && !self
                    .visits
                    .contains_key(&archive_item(d.key(), page.archived + 1));
            documents.push(Current {
                document: d,
                archived: page.archived,
                repealed_on: page.repealed_on,
                articles: justel::articles(page.articles, spaced, d.book.as_ref(), &mut spellings)?,
                checksum,
                moved,
                unread,
                extends,
            });
        }
        let mut owners: HashMap<&str, &str> = HashMap::new();
        for c in &documents {
            for a in &c.articles {
                if let Some(other) = owners.insert(&a.num, c.document.key()) {
                    bail!(
                        "{}: article {} is in documents {other} and {}",
                        t.text_uid,
                        a.num,
                        c.document.key()
                    );
                }
            }
        }
        if !documents.iter().any(|c| c.moved) {
            tracing::info!(publisher = source, key = t.key, "followed text unchanged");
            return Ok(());
        }
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let stored = if documents.iter().all(|c| c.extends) {
            repo.stored_article_versions(&t.text_uid)
                .await
                .map_err(|e| anyhow!("stored_article_versions {}: {e}", t.text_uid))?
        } else {
            Vec::new()
        };
        let fresh = stored.is_empty();
        let mut histories = justel::Histories::from_stored(&stored, &mut spellings);
        let mut order = Vec::new();
        let mut read: Vec<(String, u64)> = Vec::new();
        for c in documents {
            let (d, archived) = (c.document, c.archived);
            order.extend(c.articles.iter().map(|a| a.num.clone()));
            if !fresh && !c.moved {
                continue;
            }
            let born = d
                .born
                .map(|b| NaiveDate::parse_from_str(b, "%Y-%m-%d"))
                .transpose()?;
            let mut doc = justel::Document::new(&c.articles, born);
            let first = if fresh { 1 } else { c.unread };
            for k in first..=archived {
                let url = JustelClient::url(d.numac, d.cn, Some(k));
                let html = self.justel_page(client, d, Some(k)).await?;
                let page = lj_parse::justel_page(&html).map_err(|e| anyhow!("{url}: {e}"))?;
                let until = match page.until {
                    Some(JustelUntil::Day(day)) => NaiveDate::parse_from_str(&day, "%Y-%m-%d")?,
                    Some(JustelUntil::Undetermined) => {
                        tracing::info!(
                            publisher = source,
                            key = t.key,
                            archive = k,
                            "justel archive ends on an undetermined day, its forms left to the next version"
                        );
                        read.push((
                            archive_item(d.key(), k),
                            xxhash_rust::xxh3::xxh3_64(html.as_bytes()),
                        ));
                        continue;
                    }
                    None => bail!("{url} names no day its version stopped applying"),
                };
                let whole = page.whole;
                let articles =
                    justel::articles(page.articles, spaced, d.book.as_ref(), &mut spellings)?;
                histories.observe(&mut doc, &articles, Some(until), whole);
                read.push((
                    archive_item(d.key(), k),
                    xxhash_rust::xxh3::xxh3_64(html.as_bytes()),
                ));
            }
            histories.observe(&mut doc, &c.articles, None, true);
            if let Some(on) = &c.repealed_on {
                histories.repeal(&doc, NaiveDate::parse_from_str(on, "%Y-%m-%d")?);
            }
            read.push((d.key().to_string(), c.checksum));
            tracing::info!(
                publisher = source,
                key = t.key,
                document = d.key(),
                archived,
                read = archived + 1 - first.min(archived + 1),
                fresh,
                "justel document read"
            );
        }
        let articles = histories.into_articles(&order);
        let versions: usize = articles
            .iter()
            .map(|a| a.versions.as_ref().map_or(0, Vec::len))
            .sum();
        tracing::info!(
            publisher = source,
            key = t.key,
            text_uid = t.text_uid,
            articles = articles.len(),
            versions,
            fresh,
            "followed text rebuilt"
        );
        if self.dry_run {
            return Ok(());
        }
        let doc = CorpusDoc {
            text_uid: t.text_uid.clone(),
            source: source.to_string(),
            jurisdiction: self.jurisdiction.to_string(),
            title: t.title.clone(),
            title_key: t.title_key.clone(),
            nature: t.nature.clone(),
            translation: "officiel".to_string(),
            source_url: Some(t.url.clone()),
            source_asof: None,
            source_upstream_url: None,
            date_texte: adopted.map(str::to_string),
            effect_date: None,
            detect_abrogation: self.detect_abrogation,
            num_prefix_agnostic: false,
            articles,
            links: Vec::new(),
        };
        load_doc(&repo, &doc, self.read_on, source, &mut tally.publie_targets).await?;
        tally.written += 1;
        repo.put_norm_visits(source, &read)
            .await
            .map_err(|e| anyhow!("put_norm_visits {source}: {e}"))?;
        Ok(())
    }
    /// A Flemish act: its states walked back from today, written as one text.
    async fn codex_text(&self, client: &CodexClient, t: &Target, tally: &mut Tally) -> Result<()> {
        let source = self.source;
        let id: u64 = t
            .key
            .parse()
            .map_err(|e| anyhow!("codex key {}: {e}", t.key))?;
        let walked = codex::walk(client, id, &self.cache.join("codex")).await?;
        let (adopted, published, checksum, requests) = (
            walked.adopted.clone(),
            walked.published.clone(),
            walked.checksum,
            walked.requests,
        );
        if let Some(day) = &walked.cut {
            tracing::warn!(
                publisher = source,
                key = t.key,
                requests,
                history_from = day,
                "codex act past its download budget, its history starts at the earliest state read"
            );
        }
        let articles = walked.articles()?;
        let versions: usize = articles
            .iter()
            .map(|a| a.versions.as_ref().map_or(0, Vec::len))
            .sum();
        tracing::info!(
            publisher = source,
            key = t.key,
            requests,
            articles = articles.len(),
            versions,
            "codex act walked"
        );
        if self.dry_run {
            return Ok(());
        }
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        if !articles.is_empty() {
            let doc = CorpusDoc {
                text_uid: t.text_uid.clone(),
                source: source.to_string(),
                jurisdiction: self.jurisdiction.to_string(),
                title: t.title.clone(),
                title_key: t.title_key.clone(),
                nature: t.nature.clone(),
                translation: "officiel".to_string(),
                source_url: Some(t.url.clone()),
                source_asof: None,
                source_upstream_url: None,
                date_texte: adopted.clone(),
                effect_date: None,
                detect_abrogation: self.detect_abrogation,
                num_prefix_agnostic: false,
                articles,
                links: Vec::new(),
            };
            load_doc(&repo, &doc, self.read_on, source, &mut tally.publie_targets).await?;
            tally.written += 1;
        }
        let publication = publications::moniteur(
            &t.text_uid,
            &t.title,
            &t.nature,
            &t.url,
            adopted.as_deref(),
            published.as_deref(),
        )?;
        let visit = moniteur_visit(&t.key, &publication);
        self.write_moniteur(
            &repo,
            vec![publication],
            &[(t.key.clone(), checksum), visit],
        )
        .await
    }

    /// A Walloon act from each version its page lists that no visit read, the others from the base.
    async fn wallex_text(
        &self,
        client: &WallexClient,
        t: &Target,
        tally: &mut Tally,
    ) -> Result<()> {
        let source = self.source;
        let page = wallex::page(&client.page(&t.url).await?, &t.url)?;
        let adopted = page
            .adopted
            .as_deref()
            .map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d"))
            .transpose()?;
        let title = wallex::title(&page.title, adopted);
        let publication = wallex_publication(t, &page)?;
        let mut listed = page.versions.clone();
        listed.sort_by(|a, b| a.from.cmp(&b.from));
        let mut shown = Some(page.articles);
        let mut states: Vec<(NaiveDate, Option<Vec<ConsolidatedArticle>>)> = Vec::new();
        let mut read: Vec<(String, u64)> = Vec::new();
        let mut moved = 0usize;
        for v in &listed {
            let day = NaiveDate::parse_from_str(&v.from, "%Y-%m-%d")?;
            let item = format!("{}@{}", t.key, v.from);
            let visit = self.visits.get(&item);
            let mut articles = match &v.path {
                None => shown
                    .take()
                    .ok_or_else(|| anyhow!("{} shows two versions", t.url))?,
                Some(_) if visit.is_some() => {
                    states.push((day, None));
                    continue;
                }
                Some(path) => {
                    let url = WallexClient::url(path);
                    wallex::page(&client.page(&url).await?, &url)?.articles
                }
            };
            if articles.is_empty() {
                tracing::warn!(
                    publisher = source,
                    key = t.key,
                    state = v.from,
                    "a state without any article is left out"
                );
                continue;
            }
            for a in &mut articles {
                a.texte = lj_core::normalizer::normalize_chars(&a.texte);
            }
            let checksum = xxhash_rust::xxh3::xxh3_64(format!("{articles:?}").as_bytes());
            if visit.is_none_or(|v| v.content_checksum != checksum) {
                moved += 1;
            }
            states.push((day, Some(articles)));
            read.push((item, checksum));
        }
        let ends = listed
            .last()
            .and_then(|v| v.until.as_deref())
            .map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d"))
            .transpose()?
            .and_then(|d| d.succ_opt());
        let act_checksum = xxhash_rust::xxh3::xxh3_64(format!("{listed:?}{read:?}").as_bytes());
        read.push((t.key.clone(), act_checksum));
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        if moved > 0 {
            let stored = if states.iter().any(|(_, arts)| arts.is_none()) {
                repo.stored_article_versions(&t.text_uid)
                    .await
                    .map_err(|e| anyhow!("stored_article_versions {}: {e}", t.text_uid))?
            } else {
                Vec::new()
            };
            let states = states
                .into_iter()
                .map(|(day, arts)| (day, arts.unwrap_or_else(|| stored_state(&stored, day))))
                .filter(|(_, arts)| !arts.is_empty())
                .collect::<Vec<_>>();
            let mut articles = article_versions(&states);
            for v in articles
                .iter_mut()
                .flat_map(|a| a.versions.iter_mut().flatten())
            {
                if wallex::repealed(&v.texte) {
                    v.status = "ABROGE".to_string();
                }
            }
            if let Some(end) = ends {
                for a in &mut articles {
                    if let Some(open) = a
                        .versions
                        .iter_mut()
                        .flatten()
                        .last()
                        .filter(|v| v.date_fin.is_none())
                    {
                        open.date_fin = Some(end.to_string());
                    }
                }
            }
            let versions: usize = articles
                .iter()
                .map(|a| a.versions.as_ref().map_or(0, Vec::len))
                .sum();
            tracing::info!(
                publisher = source,
                key = t.key,
                text_uid = t.text_uid,
                states = states.len(),
                read = read.len() - 1,
                articles = articles.len(),
                versions,
                "followed text rebuilt"
            );
            if self.dry_run {
                return Ok(());
            }
            let doc = CorpusDoc {
                text_uid: t.text_uid.clone(),
                source: source.to_string(),
                jurisdiction: self.jurisdiction.to_string(),
                nature: belgium::nature(&title).to_string(),
                title,
                title_key: None,
                translation: "officiel".to_string(),
                source_url: Some(t.url.clone()),
                source_asof: None,
                source_upstream_url: None,
                date_texte: page.adopted,
                effect_date: None,
                detect_abrogation: self.detect_abrogation,
                num_prefix_agnostic: false,
                articles,
                links: Vec::new(),
            };
            load_doc(&repo, &doc, self.read_on, source, &mut tally.publie_targets).await?;
            tally.written += 1;
        }
        if self.dry_run {
            return Ok(());
        }
        read.push(moniteur_visit(&t.key, &publication));
        self.write_moniteur(&repo, vec![publication], &read).await
    }
}

pub async fn sync_norms(
    country: NormsCountry,
    scan: NormsScan,
    only: Option<String>,
    limit: Option<usize>,
    fresh: bool,
    minutes: Option<u64>,
    dry_run: bool,
) -> Result<()> {
    if matches!(country, NormsCountry::BeDg) {
        if only.is_some() {
            bail!("be-dg reads every DGLex document, --only does not apply");
        }
        return dglex::sync(scan, limit, fresh, minutes, dry_run).await;
    }
    if matches!(country, NormsCountry::BeGallilex) {
        if only.is_some() {
            bail!("be-gallilex loads every cached Gallilex document, --only does not apply");
        }
        return gallilex::sync(limit, fresh, dry_run).await;
    }
    if let NormsCountry::CaQc | NormsCountry::CaNb = country {
        let site = match country {
            NormsCountry::CaQc => &lims::LEGISQUEBEC,
            _ => &lims::NEW_BRUNSWICK,
        };
        return lims::sync(site, only.as_deref(), limit, fresh, dry_run).await;
    }
    let settings = Settings::from_env()?;
    let (jurisdiction, _, detect_abrogation) = country.followed();
    let now = Utc::now();
    let deadline =
        minutes.map(|m| std::time::Instant::now() + std::time::Duration::from_secs(m * 60));

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let lanes = lanes(
        country,
        &settings.egress_proxy,
        only.as_deref(),
        &settings.cache_dir(),
        scan == NormsScan::Relist,
    )
    .await?;
    let Some(source) = lanes.first().map(|l| l.publisher.name()) else {
        bail!("no followed {jurisdiction} text is keyed {only:?}");
    };
    let posts_asked = lanes.iter().any(|l| {
        matches!(&l.publisher, Publisher::Ejo(_, ids) | Publisher::Cnlegis(_, ids) if !ids.is_empty())
    });
    if only.is_some() && !posts_asked && lanes.iter().all(|l| l.targets.is_empty()) {
        bail!("no followed {jurisdiction} text is keyed {only:?}");
    }
    let visits = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
            .await
            .map_err(|e| anyhow!("migrations: {e}"))?;
        if fresh {
            HashMap::new()
        } else {
            DecisionRepository::new(&conn)
                .norm_visits(source)
                .await
                .map_err(|e| anyhow!("norm_visits {source}: {e}"))?
        }
    };
    let run = Run {
        source,
        jurisdiction,
        detect_abrogation,
        relist_before: relist_before(scan, now),
        read_on: now.date_naive(),
        dry_run,
        visits: &visits,
        pool: &pool,
        cache: settings.cache_dir(),
        deadline,
    };
    let tallies =
        futures::future::join_all(lanes.into_iter().map(|lane| run.lane(lane, limit))).await;

    let mut total = Tally::default();
    for t in tallies {
        total.listed += t.listed;
        total.unchanged += t.unchanged;
        total.written += t.written;
        total.repaired.extend(t.repaired);
        total.failed.extend(t.failed);
        total.publie_targets.extend(t.publie_targets);
    }
    if !total.repaired.is_empty() {
        tracing::warn!(
            publisher = source,
            repaired = total.repaired.len(),
            states = total.repaired.join(","),
            "the weekly relist found states the daily scan missed"
        );
    }
    tracing::info!(
        publisher = source,
        ?scan,
        listed = total.listed,
        unchanged = total.unchanged,
        written = total.written,
        failed = total.failed.len(),
        dry_run,
        "sync-norms finished"
    );
    if total.written > 0 {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        run_corpus_finishers(&DecisionRepository::new(&conn), total.publie_targets).await?;
    }
    if !total.failed.is_empty() {
        bail!(
            "{} followed texts failed: {}",
            total.failed.len(),
            total.failed.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(num: &str, texte: &str) -> ConsolidatedArticle {
        ConsolidatedArticle {
            num: num.to_string(),
            title_path: None,
            texte: texte.to_string(),
            nota: None,
        }
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn windows(a: &CorpusArticle) -> Vec<(Option<&str>, Option<&str>, &str)> {
        a.versions
            .as_ref()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v.date_debut.as_deref(),
                    v.date_fin.as_deref(),
                    v.texte.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn an_article_gets_a_version_per_change_and_closes_when_it_leaves() {
        let states = vec![
            (day("2021-01-01"), vec![art("1", "a"), art("2", "x")]),
            (day("2022-01-01"), vec![art("1", "a"), art("2", "y")]),
            (day("2023-07-01"), vec![art("1", "b")]),
        ];
        let arts = article_versions(&states);
        let one = arts.iter().find(|a| a.num == "1").unwrap();
        let two = arts.iter().find(|a| a.num == "2").unwrap();
        assert_eq!(
            windows(one),
            vec![
                (Some("2021-01-01"), Some("2023-07-01"), "a"),
                (Some("2023-07-01"), None, "b"),
            ]
        );
        assert_eq!(
            windows(two),
            vec![
                (Some("2021-01-01"), Some("2022-01-01"), "x"),
                (Some("2022-01-01"), Some("2023-07-01"), "y"),
            ]
        );
    }

    /// A country's texts are written, slugged and placed only under a jurisdiction the
    /// referential knows, with a journal whose rules give it its places.
    #[test]
    fn every_followed_country_is_a_jurisdiction_of_the_referential() {
        use clap::ValueEnum;
        for country in NormsCountry::value_variants() {
            let (jurisdiction, table, _) = country.followed();
            assert_eq!(
                lj_norm::jurisdictions::from_url_segment(&jurisdiction.to_ascii_lowercase()),
                Some(jurisdiction)
            );
            assert!(lj_norm::referential_labels::jurisdiction_label(jurisdiction).is_some());
            assert!(super::super::slugs::country(jurisdiction).is_ok());
            for f in table {
                let journal = lj_norm::applicability::journal_code(f.text_uid, jurisdiction);
                assert!(
                    !lj_norm::journal_rules::rules_of(&journal).is_empty(),
                    "{} {journal}",
                    f.text_uid
                );
            }
        }
        for b in BELGIUM {
            let journal = lj_norm::applicability::journal_code(b.followed.text_uid, "BE");
            assert!(!lj_norm::journal_rules::rules_of(&journal).is_empty());
        }
        let codes = lj_sources::lexwork::CANTONS
            .iter()
            .map(|c| c.code)
            .chain(lj_sources::lexfind::CANTONS.iter().map(|c| c.code));
        for code in codes {
            let journal = lj_norm::applicability::journal_code(&format!("CH-{code}/E-5-10"), "CH");
            assert_eq!(journal, format!("JO-CH-{code}"));
            assert!(!lj_norm::journal_rules::rules_of(&journal).is_empty());
        }
        let journal = lj_norm::applicability::journal_code("CH-VD/173.36", "CH");
        assert_eq!(journal, "JO-CH-VD");
        assert!(!lj_norm::journal_rules::rules_of(&journal).is_empty());
    }

    #[test]
    fn a_cantonal_act_s_nature_is_read_from_its_title() {
        for (title, nature) in [
            ("Constitution du canton de Fribourg", "CONSTITUTION"),
            ("Verfassung des Kantons Aargau", "CONSTITUTION"),
            ("Loi sur le Grand Conseil", "loi"),
            (
                "Einführungsgesetz zum Schweizerischen Zivilgesetzbuch",
                "loi",
            ),
            (
                "Règlement d'exécution de la loi sur les impôts",
                "ordonnance",
            ),
            ("Vollziehungsverordnung zum Bundesgesetz", "ordonnance"),
            ("Kantonsratsbeschluss über die Genehmigung", "DECRET"),
            (
                "Interkantonale Vereinbarung über die Harmonisierung",
                "acte",
            ),
        ] {
            assert_eq!(cantonal_nature(title), nature, "{title}");
        }
    }

    /// A Canadian act is named by its title before the statute reference, without the country.
    #[test]
    fn a_canadian_act_s_slug_is_its_name() {
        let expected = [
            ("C-46", "code-criminel"),
            ("D-3.4", "loi-sur-le-divorce"),
            ("C-5", "loi-sur-la-preuve"),
            ("B-3", "loi-sur-la-faillite-et-l-insolvabilite"),
            (
                "I-2.5",
                "loi-sur-l-immigration-et-la-protection-des-refugies",
            ),
            ("H-6", "loi-sur-les-droits-de-la-personne"),
        ];
        let mut taken = std::collections::HashSet::new();
        for (f, (key, slug)) in CANADA.iter().zip(expected) {
            assert_eq!(f.key, key);
            let src = lj_store::repository::SlugSourceRow {
                text_uid: f.text_uid.to_string(),
                title: f.title.to_string(),
                jurisdiction: "CA".to_string(),
                date_texte: None,
                date_publi: None,
                nor: None,
            };
            let minted = super::super::slugs::pick_slug(&src, &taken).unwrap();
            assert_eq!(minted, slug, "{}", f.title);
            taken.insert(("CA".to_string(), minted));
        }
    }

    fn visited(at: &str, checksum: u64) -> NormVisit {
        NormVisit {
            visited_at: at.parse().unwrap(),
            content_checksum: checksum,
        }
    }

    #[test]
    fn a_state_is_read_when_never_visited_or_modified_since_its_visit() {
        let visit = visited("2026-11-10T03:00:00Z", 7);
        let daily = relist_before(NormsScan::Daily, "2026-11-15T03:00:00Z".parse().unwrap());
        assert_eq!(reason(None, None, daily), Some(Reason::New));
        assert_eq!(
            reason(Some(day("2026-11-12")), None, daily),
            Some(Reason::New)
        );
        assert_eq!(reason(None, Some(&visit), daily), None);
        assert_eq!(reason(Some(day("2026-11-09")), Some(&visit), daily), None);
        assert_eq!(
            reason(Some(day("2026-11-10")), Some(&visit), daily),
            Some(Reason::Modified)
        );
        assert_eq!(
            reason(Some(day("2026-11-14")), Some(&visit), daily),
            Some(Reason::Modified)
        );
    }

    /// The previous relist reached the state an hour later in its run than this one does.
    #[test]
    fn the_relist_reads_every_state_the_previous_relist_read() {
        let now = "2026-11-15T03:00:00Z".parse().unwrap();
        let by_last_relist = visited("2026-11-08T04:00:00Z", 7);
        let by_a_daily_run = visited("2026-11-12T03:00:00Z", 7);
        let relist = relist_before(NormsScan::Relist, now);
        assert_eq!(
            reason(None, Some(&by_last_relist), relist),
            Some(Reason::Relist)
        );
        assert_eq!(reason(None, Some(&by_a_daily_run), relist), None);
        assert_eq!(
            reason(Some(day("2026-11-13")), Some(&by_a_daily_run), relist),
            Some(Reason::Modified)
        );
        let daily = relist_before(NormsScan::Daily, now);
        assert_eq!(reason(None, Some(&by_last_relist), daily), None);
    }

    /// The base as the writer leaves it: a version closed by the next one ends the day before.
    fn as_stored(articles: &[CorpusArticle]) -> Vec<StoredArticleVersion> {
        let mut stored = Vec::new();
        for (position, a) in articles.iter().enumerate() {
            let versions = a.versions.as_ref().unwrap();
            for (k, v) in versions.iter().enumerate() {
                let next = versions.get(k + 1).and_then(|n| n.date_debut.as_deref());
                let date_fin = v.date_fin.as_deref().map(|fin| match Some(fin) == next {
                    true => day(fin).pred_opt().unwrap(),
                    false => day(fin),
                });
                stored.push(StoredArticleVersion {
                    num: a.num.clone(),
                    position: Some(position as i32),
                    title_path: a.title_path.clone(),
                    nota: a.nota.clone(),
                    texte: v.texte.clone(),
                    date_debut: day(v.date_debut.as_deref().unwrap()),
                    date_fin,
                });
            }
        }
        stored.sort_by(|a, b| a.num.cmp(&b.num));
        stored
    }

    /// A text rebuilt from the base for the states not read, around the one read, is the text
    /// read whole: the same articles in the same order, with the same versions.
    #[test]
    fn a_text_rebuilt_from_the_base_is_the_text_read_whole() {
        let states = vec![
            (
                day("2020-01-01"),
                vec![art("1", "a"), art("2", "x"), art("3", "c")],
            ),
            (day("2021-01-01"), vec![art("1", "a"), art("2", "y")]),
            (day("2021-01-02"), vec![art("1", "b"), art("2", "y")]),
            (day("2022-01-01"), vec![art("1", "b"), art("5", "e")]),
            (
                day("2023-01-01"),
                vec![art("1", "b"), art("2", "y"), art("4", "d")],
            ),
            (
                day("2024-01-01"),
                vec![art("4", "d"), art("1", "f"), art("2", "y")],
            ),
        ];
        let whole = article_versions(&states);
        let stored = as_stored(&whole);
        for read in 0..states.len() {
            let rebuilt: Vec<_> = states
                .iter()
                .enumerate()
                .map(|(i, (d, arts))| match i == read {
                    true => (*d, arts.clone()),
                    false => (*d, stored_state(&stored, *d)),
                })
                .collect();
            let again = article_versions(&rebuilt);
            assert_eq!(
                again
                    .iter()
                    .map(|a| (&a.num, windows(a)))
                    .collect::<Vec<_>>(),
                whole
                    .iter()
                    .map(|a| (&a.num, windows(a)))
                    .collect::<Vec<_>>(),
                "state {read} read"
            );
        }
    }

    #[test]
    fn an_article_that_returns_opens_a_new_version() {
        let states = vec![
            (day("2021-01-01"), vec![art("5", "a")]),
            (day("2022-01-01"), vec![]),
            (day("2023-01-01"), vec![art("5", "a")]),
        ];
        assert_eq!(
            windows(&article_versions(&states)[0]),
            vec![
                (Some("2021-01-01"), Some("2022-01-01"), "a"),
                (Some("2023-01-01"), None, "a"),
            ]
        );
    }
}
