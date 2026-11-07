use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// OCR markdown → texte propre. La CNDA passe par l'OCR Mistral : un markdown par
// page (titres `#`, gras, listes, images, tables) où chaque paragraphe logique
// tient sur une ligne. On retire le balisage en préservant `ligne = paragraphe`,
// puis on applique `clean_texte`.
// ─────────────────────────────────────────────────────────────────────────────

static MD_IMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
static MD_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
static MD_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]{0,3}#{1,6}[ \t]+").unwrap());
/// Ne touche PAS la numérotation `1.`/`2.` des considérants, qui est sémantique.
static MD_LIST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]{0,3}[-*+][ \t]+").unwrap());

fn is_md_noise_line(trimmed: &str) -> bool {
    if trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') {
        return true;
    }
    trimmed.contains('|') && trimmed.chars().all(|c| matches!(c, '|' | ':' | '-' | ' '))
}

pub fn clean_ocr_markdown(md: &str) -> String {
    let no_img = MD_IMAGE_RE.replace_all(md, "");
    let no_link = MD_LINK_RE.replace_all(&no_img, "$1");
    let no_head = MD_HEADING_RE.replace_all(&no_link, "");
    let no_list = MD_LIST_RE.replace_all(&no_head, "");
    let stripped = no_list
        .lines()
        .filter(|l| !is_md_noise_line(l.trim()))
        .map(|l| l.replace("**", "").replace("__", "").replace('|', " "))
        .collect::<Vec<_>>()
        .join("\n");
    clean_texte(&stripped)
}

/// Ligne d'en-tête/pied de page répétée (numéro de page, `n° NNNN` seul, filets,
/// bandeau juridiction, liste N° multi-requêtes `N° 17010844 – 18044574, …`
/// rappelée en tête de chaque page) — retirée du flux. N'est supprimée que si
/// répétée ≥ 3× : le cartouche de titre, qui éclate ces mêmes N° sur des lignes
/// distinctes, survit au gate de fréquence.
static CNDA_HEADER_NOISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(N°\s*\d+\s*$|N°\s*\d{4,}(?:[\s,–-]+(?:et\s+)?\d{4,})+\s*$|page\s+\d+|–\s*\d+\s*–|-\s*\d+\s*-|\d+\s*/\s*\d+\s*$|RÉPUBLIQUE\s+FRAN[ÇC]AISE|COUR\s+NATIONALE\s+DU\s+DROIT|République française\s*$)",
    )
    .unwrap()
});

/// Motif d'en-tête/pied **toujours** supprimé même sans répétition (purement
/// structurel : numéro nu, numéro de page, filet).
static CNDA_HEADER_PURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(N°\s*\d+|page\s+\d+|\d+\s*/\s*\d+|–\s*\d+\s*–|-\s*\d+\s*-)\s*$").unwrap()
});

/// Dispositif lettriné/accentué sur sa propre ligne (`D É C I D E`, `DÉCIDE`,
/// `DECIDE :`) → normalisé en `DECIDE :` pour la détection de section.
static CNDA_DECIDE_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*D\s*[ÉE]\s*C\s*I\s*D\s*E\s*:?\s*$").unwrap());

/// The CNDA's page headers dropped and its `D É C I D E` written `DECIDE :` on a line of its own,
/// where the dispositif is found, then [`reflow_page_text`](crate::reflow_page_text).
pub fn reflow_cnda_pdf_text(raw: &str) -> String {
    let normalized = raw.replace('\u{c}', "\n");
    let lines: Vec<&str> = normalized.lines().map(str::trim).collect();
    let mut freq: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for l in lines.iter().filter(|l| !l.is_empty()) {
        *freq.entry(l).or_default() += 1;
    }
    let kept: Vec<&str> = lines
        .iter()
        .filter(|l| {
            !(CNDA_HEADER_PURE_RE.is_match(l) || CNDA_HEADER_NOISE_RE.is_match(l) && freq[*l] >= 3)
        })
        .map(|l| {
            if CNDA_DECIDE_LINE_RE.is_match(l) {
                "\nDECIDE :\n"
            } else {
                l
            }
        })
        .collect();
    crate::reflow_page_text(&kept.join("\n"))
}

// ─────────────────────────────────────────────────────────────────────────────
// CNDA — Cour nationale du droit d'asile (asile, protection subsidiaire).
//
// Parser PUR : reçoit le texte issu de l'OCR Mistral déjà nettoyé au bord
// (`clean_ocr_markdown`) et les métadonnées de la fiche HTML éditoriale
// désérialisées (`fiche` : titre éditorial, content_type, editorial_abstract,
// fiche_url, pdf_url, date de publication). Émet un [`CndaParsed`] : la
// `Decision`, les `source_fields` (métadonnées hors texte) et la `solution_uid`
// best-effort. Aucune I/O, aucune dép native.
//
// La CNDA est absente de Judilibre et de l'opendata : juridiction
// `jurisdiction_type = CNDA`, ECLI fabriqué `ECLI:FR:CNDA:{année}:{numero}` (la
// Cour n'en émet aucun), date indexée = date de lecture du PDF, jamais la date de
// mise en ligne de la fiche.
// ─────────────────────────────────────────────────────────────────────────────

/// Les 8 marqueurs de section d'une décision CNDA (gabarit administratif type CE,
/// ordre préservé). IGNORECASE ; le dispositif marqué `DECIDE` est ancré en tête
/// de ligne (MULTILINE), le recollage `D E C I D E :` → `DECIDE :` ayant déjà eu
/// lieu au bord.
static CNDA_SECTION_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        (
            "procedure",
            Regex::new(r"(?i)\bVu la procédure suivante\s*:").unwrap(),
        ),
        ("visas", Regex::new(r"(?i)\bVu la convention\b").unwrap()),
        (
            "audience",
            Regex::new(r"(?i)\b(?:Après avoir entendu|Sont intervenus à l'audience publique)\b")
                .unwrap(),
        ),
        (
            "motifs",
            Regex::new(r"(?i)\bConsidérant ce qui suit\b\s*:?\s*").unwrap(),
        ),
        (
            "dispositif",
            Regex::new(r"(?mi)^\s*D\s*[ÉE]\s*C\s*I\s*D\s*E\s*:?\s*$").unwrap(),
        ),
        (
            "lecture_signature",
            Regex::new(r"(?i)\bLu en audience publique\b").unwrap(),
        ),
        (
            "execution",
            Regex::new(r"(?i)\bLa République mande\b").unwrap(),
        ),
        (
            "voies_recours",
            Regex::new(r"(?i)\bLa présente décision sera notifiée\b").unwrap(),
        ),
    ]
});

/// `(code, libellé)` du classement nomenclature `095-…`.
static CNDA_CLASSEMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(095(?:-\d{2})+)\s+(.+)").unwrap());

/// Importance jurisprudentielle CNDA (`C` / `C+` / `R`), en tête de ligne.
static CNDA_IMPORTANCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(C\+|C|R)\s*$").unwrap());

static CNDA_FORMATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([^()]*\bchambre\b[^()]*)\)").unwrap());

/// Date de lecture (date indexée), capturée APRÈS un marqueur de prononcé sous
/// toutes ses formes : `Lu en audience publique le …`, `Lu en séance publique le
/// …`, `Lecture du …`, `Lecture …`. Le groupe 1 = la date FR
/// (`8 septembre 2022`) ; une vraie date jour-mois-année est exigée après le
/// marqueur.
static CNDA_LECTURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:Lu en (?:audience|séance) publique le|Lecture(?:\s+du)?)\s+((?:1er|\d{1,2})\s+[a-zàâäéèêëîïôöùûüç]+\s+\d{4})",
    )
    .unwrap()
});

/// Date d'audience (`Audience du 30 mars 2026`, `séance publique du …`).
/// Groupe 1 = date FR.
static CNDA_AUDIENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:Audience du|(?:séance|audience) publique du)\s+((?:1er|\d{1,2})\s+[a-zàâäéèêëîïôöùûüç]+\s+\d{4})",
    )
    .unwrap()
});

/// Date FR lue dans un slug éditorial CNDA/CRR (`cnda-6-mai-2015-…`,
/// `crr-sr-17-fevrier-2006-…`). Le slug porte la date de décision (= lecture),
/// pas la date de mise en ligne.
static CNDA_SLUG_DATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d{1,2})-([a-zàâäéèêëîïôöùûüç]+)-(\d{4})\b").unwrap());

static CNDA_YEAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{4})\b").unwrap());

static CNDA_FR_DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d{1,2}|1er)\s+([a-zàâäéèêëîïôöùûüç]+)\s+(\d{4})\b").unwrap()
});

fn cnda_fr_month(name: &str) -> Option<i8> {
    Some(match name.to_lowercase().as_str() {
        "janvier" => 1,
        "février" | "fevrier" => 2,
        "mars" => 3,
        "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" => 7,
        "août" | "aout" => 8,
        "septembre" => 9,
        "octobre" => 10,
        "novembre" => 11,
        "décembre" | "decembre" => 12,
        _ => return None,
    })
}

/// Date de lecture FR libre (`12 mai 2026`) → ISO `2026-05-12` : la frontière
/// store ne parse que `%Y-%m-%d` et jette toute autre forme en NULL. La forme FR
/// brute reste en `source_fields.lecture_date`.
fn cnda_lecture_to_iso(lecture: &str) -> Option<String> {
    let c = CNDA_FR_DATE_RE.captures(lecture)?;
    let day: i8 = if c[1].eq_ignore_ascii_case("1er") {
        1
    } else {
        c[1].parse().ok()?
    };
    let month = cnda_fr_month(&c[2])?;
    let year: i16 = c[3].parse().ok()?;
    jiff::civil::Date::new(year, month, day)
        .ok()
        .map(|d| d.strftime("%Y-%m-%d").to_string())
}

fn cnda_date_from_slug(slug: &str) -> Option<String> {
    let c = CNDA_SLUG_DATE_RE.captures(slug)?;
    cnda_fr_month(&c[2])?; // valide le mois
    Some(format!("{} {} {}", &c[1], &c[2], &c[3]))
}

/// Date de lecture FR libre, par ordre de fiabilité décroissante :
/// 1. marqueur de prononcé dans le texte OCR ([`CNDA_LECTURE_RE`]) ;
/// 2. `lecture_date` de la fiche (rarement présent) ;
/// 3. date du slug éditorial (`pdf_url` puis `fiche_url`).
///
/// `None` si aucune source ne donne de date (ECLI infabricable).
fn cnda_resolve_lecture(pdf_text: &str, fiche: &Value) -> Option<String> {
    CNDA_LECTURE_RE
        .captures(pdf_text)
        .map(|c| c[1].trim().to_string())
        .or_else(|| fiche_str(fiche, "lecture_date").map(str::to_string))
        .or_else(|| fiche_str(fiche, "pdf_url").and_then(cnda_date_from_slug))
        .or_else(|| fiche_str(fiche, "fiche_url").and_then(cnda_date_from_slug))
}

/// Découpe le texte PDF CNDA en sections via les 8 marqueurs. Premier match par
/// `kind`, tri par position, préambule = en-tête avant le premier marqueur.
/// Offsets en codepoints.
fn extract_sections_cnda(text: &str) -> Vec<DecisionSection> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut markers: Vec<(usize, &'static str, String)> = Vec::new();
    for (kind, re) in CNDA_SECTION_PATTERNS.iter() {
        if let Some(m) = re.find(text) {
            let start_char = byte_to_char_index(text, m.start());
            markers.push((start_char, kind, m.as_str().trim().to_string()));
        }
    }
    if markers.is_empty() {
        return Vec::new();
    }
    markers.sort_by_key(|m| m.0);

    let mut deduped: Vec<(usize, &'static str, String)> = Vec::new();
    let mut seen_kinds: Vec<&'static str> = Vec::new();
    let mut last_start: Option<usize> = None;
    for (start, kind, label) in markers {
        if seen_kinds.contains(&kind) || Some(start) == last_start {
            continue;
        }
        deduped.push((start, kind, label));
        seen_kinds.push(kind);
        last_start = Some(start);
    }

    let total = char_len(text);
    let mut sections: Vec<DecisionSection> = Vec::new();

    if deduped[0].0 > 0 {
        let intro = char_slice(text, 0, deduped[0].0);
        let intro = intro.trim();
        if !intro.is_empty() {
            sections.push(DecisionSection {
                label: "Préambule".to_string(),
                kind: "preamble".to_string(),
                start_char: 0,
                end_char: deduped[0].0,
                text: intro.to_string(),
            });
        }
    }

    for (idx, (start, kind, label)) in deduped.iter().enumerate() {
        let end = if idx + 1 < deduped.len() {
            deduped[idx + 1].0
        } else {
            total
        };
        let section_text = char_slice(text, *start, end);
        let section_text = section_text.trim();
        if section_text.is_empty() {
            continue;
        }
        sections.push(DecisionSection {
            label: label.clone(),
            kind: (*kind).to_string(),
            start_char: *start,
            end_char: end,
            text: section_text.to_string(),
        });
    }
    sections
}

fn cnda_year_from_lecture(lecture: &str) -> Option<String> {
    CNDA_YEAR_RE.captures(lecture).map(|c| c[1].to_string())
}

/// Mapping best-effort du dispositif CNDA → uid `solution:*`. Qualité de réfugié,
/// protection subsidiaire ou annulation OFPRA ⇒ le recours aboutit
/// (`solution:SATISFACTION_TOTALE`) ; rejet ⇒ `solution:REJET`. Exclusion ou
/// révision ⇒ `None`, signal ambigu.
fn cnda_solution_uid(dispositif: Option<&str>) -> Option<String> {
    let raw = dispositif?.to_lowercase();
    // Révision : la Cour défait sa propre décision antérieure (« déclarée nulle et
    // non avenue ») puis restatue sur le recours initial. Le dispositif porte alors
    // les deux sens — le recours en révision est admis, le recours initial rejeté.
    if raw.contains("non avenue") {
        return None;
    }
    let key = if raw.contains("reconnaît la qualité de réfugié")
        || raw.contains("reconnait la qualité de réfugié")
        || raw.contains("qualité de réfugié")
        || raw.contains("protection subsidiaire")
        || raw.contains("annule la décision")
        || raw.contains("annulation")
    {
        "SATISFACTION_TOTALE"
    } else if raw.contains("rejet") {
        "REJET"
    } else {
        return None;
    };
    Some(format!("solution:{key}"))
}

fn fiche_str<'a>(fiche: &'a Value, key: &str) -> Option<&'a str> {
    fiche
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Résultat du parse CNDA : la `Decision`, les `source_fields` (métadonnées hors
/// texte) et la solution best-effort — ces deux-là débordent de `Decision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CndaParsed {
    pub decision: Decision,
    pub source_fields: Value,
    pub solution_uid: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn build_source_fields_cnda(
    fiche: &Value,
    classement_code: Option<&str>,
    classement_libelle: Option<&str>,
    importance: Option<&str>,
    formation: Option<&str>,
    audience_date: Option<&str>,
    lecture_date: &str,
    solution_uid: Option<&str>,
) -> Value {
    let mut out = serde_json::Map::new();
    let mut put = |k: &str, v: Option<&str>| {
        if let Some(v) = v {
            out.insert(k.to_string(), Value::String(v.to_string()));
        }
    };
    put("editorial_abstract", fiche_str(fiche, "editorial_abstract"));
    put("titre_editorial", fiche_str(fiche, "titre"));
    put("content_type", fiche_str(fiche, "content_type"));
    put("fiche_url", fiche_str(fiche, "fiche_url"));
    put("pdf_url", fiche_str(fiche, "pdf_url"));
    put(
        "fiche_publication_date",
        fiche_str(fiche, "publication_date"),
    );
    put("classement_code", classement_code);
    put("classement_libelle", classement_libelle);
    put("importance", importance);
    put("formation", formation);
    put("audience_date", audience_date);
    // Le sens du dispositif n'a pas de colonne dans `Decision` : il ne survit à la
    // décomposition `source_payload ⟷ (full_text, source_fields)` que par cette clé.
    put("solution_uid", solution_uid);
    out.insert(
        "lecture_date".to_string(),
        Value::String(lecture_date.to_string()),
    );
    Value::Object(out)
}

/// `true` si `source_uid` provient de la CNDA : préfixe `cnda/`. Discriminant de
/// famille du dispatch [`Decision::from_source_fields`].
pub(crate) fn source_uid_is_cnda(source_uid: &str) -> bool {
    source_uid.starts_with("cnda/")
}

/// Reconstruit la `fiche` (clés d'origine lues par [`parse_cnda`]) depuis les
/// `source_fields` CNDA, dont les clés renommées par [`build_source_fields_cnda`]
/// (`titre_editorial`←`titre`, `fiche_publication_date`←`publication_date`).
fn cnda_fiche_from_source_fields(source_fields: &Value) -> Value {
    let mut fiche = serde_json::Map::new();
    let mut copy = |fiche_key: &str, sf_key: &str| {
        if let Some(v) = fiche_str(source_fields, sf_key) {
            fiche.insert(fiche_key.to_string(), Value::String(v.to_string()));
        }
    };
    copy("titre", "titre_editorial");
    copy("publication_date", "fiche_publication_date");
    copy("editorial_abstract", "editorial_abstract");
    copy("content_type", "content_type");
    copy("fiche_url", "fiche_url");
    copy("pdf_url", "pdf_url");
    copy("lecture_date", "lecture_date");
    copy("audience_date", "audience_date");
    Value::Object(fiche)
}

/// CNDA branch of [`Decision::from_source_fields`]: rebuilds a `Decision`
/// identical to [`parse_cnda`] from `(full_text, source_fields)`. Exact
/// counterpart of [`build_source_fields_cnda`].
///
/// [`parse_cnda`] leaves `metadata_header` empty, so the chunk depends on
/// `texte_integral_clean` alone (= `full_text`, stored verbatim).
/// `solution_uid` does not fit in `Decision` and does not pass through here: it
/// lives in `source_fields`.
///
/// Panics if the parser fails: the input comes from a decision already
/// ingested, hence valid.
pub(crate) fn from_source_fields_cnda(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    reparse_cnda(full_text, source_fields, source_uid).decision
}

fn reparse_cnda(full_text: &str, source_fields: &Value, source_uid: &str) -> CndaParsed {
    let numero = source_uid
        .strip_prefix("cnda/")
        .unwrap_or_else(|| panic!("reparse_cnda: source_uid non CNDA {source_uid}"));
    let fiche = cnda_fiche_from_source_fields(source_fields);
    let pdf_text = if extract_sections_cnda(full_text).is_empty() {
        ""
    } else {
        full_text
    };
    parse_cnda(pdf_text, &fiche, numero)
        .unwrap_or_else(|e| panic!("reparse CNDA {source_uid}: {e}"))
}

pub fn cnda_solution_uid_from_source_fields(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Option<String> {
    reparse_cnda(full_text, source_fields, source_uid).solution_uid
}

/// Parse une décision CNDA en [`CndaParsed`].
///
/// `pdf_text` vide ⇒ décision fiche-only (`full_text` = abstract éditorial).
/// L'`ecli` est fabriqué (`ECLI:FR:CNDA:{année}:{numero}`, année dérivée de la
/// date de lecture) : la Cour n'en émet aucun.
///
/// Erreur franche si le numéro est vide, ou — avec PDF — si aucune des 8
/// sections n'est détectée ou si la date de lecture est introuvable.
///
/// `metadata_header` is left empty, so no metadata enters the chunk: dates, ECLI,
/// classification, formation and URLs change without a re-embed.
pub fn parse_cnda(
    pdf_text: &str,
    fiche: &Value,
    numero: &str,
) -> lj_core::error::Result<CndaParsed> {
    use lj_core::error::CoreError;

    let numero = numero.trim();
    if numero.is_empty() {
        return Err(CoreError::Xml("CNDA: numéro de décision vide".to_string()));
    }

    let has_pdf = !pdf_text.trim().is_empty();

    if has_pdf {
        // ── Décision avec PDF : texte intégral + métadonnées extraites. ──
        let sections = extract_sections_cnda(pdf_text);
        if sections.is_empty() {
            return Err(CoreError::Xml(format!(
                "CNDA {numero}: aucune des 8 sections détectée (gabarit PDF inattendu)"
            )));
        }

        let lecture_date = cnda_resolve_lecture(pdf_text, fiche).ok_or_else(|| {
            CoreError::Xml(format!(
                "CNDA {numero}: date de lecture introuvable (texte OCR ni slug)"
            ))
        })?;
        let year = cnda_year_from_lecture(&lecture_date).ok_or_else(|| {
            CoreError::Xml(format!(
                "CNDA {numero}: année illisible dans la date de lecture « {lecture_date} »"
            ))
        })?;

        let audience_date = CNDA_AUDIENCE_RE
            .captures(pdf_text)
            .map(|c| c[1].trim().to_string());
        let formation = CNDA_FORMATION_RE
            .captures(pdf_text)
            .map(|c| c[1].trim().to_string());
        let importance = CNDA_IMPORTANCE_RE
            .captures(pdf_text)
            .map(|c| c[1].trim().to_string());
        let classement = CNDA_CLASSEMENT_RE.captures(pdf_text);
        let classement_code = classement.as_ref().map(|c| c[1].trim().to_string());
        let classement_libelle = classement.as_ref().map(|c| c[2].trim().to_string());

        let dispositif = sections
            .iter()
            .find(|s| s.kind == "dispositif")
            .map(|s| s.text.clone());
        let solution_uid = cnda_solution_uid(dispositif.as_deref());

        let source_fields = build_source_fields_cnda(
            fiche,
            classement_code.as_deref(),
            classement_libelle.as_deref(),
            importance.as_deref(),
            formation.as_deref(),
            audience_date.as_deref(),
            &lecture_date,
            solution_uid.as_deref(),
        );

        let decision = Decision {
            source_uid: format!("cnda/{numero}"),
            member_name: numero.to_string(),
            ecli: Some(format!("ECLI:FR:CNDA:{year}:{numero}")),
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name: Some("Cour nationale du droit d'asile".to_string()),
            jurisdiction_type: Some("CNDA".to_string()),
            jurisdiction_location: None,
            numero_dossier: Some(numero.to_string()),
            numero_dossiers: Some(vec![numero.to_string()]),
            numero_role: None,
            date_lecture: cnda_lecture_to_iso(&lecture_date),
            date_audience: audience_date.as_deref().and_then(cnda_lecture_to_iso),
            date_mise_jour: None,
            formation,
            type_decision: None,
            type_recours: None,
            solution: dispositif,
            publication_codes: importance.map(|i| vec![i]).unwrap_or_default(),
            avocat_requerant: None,
            listed_counsel: Vec::new(),
            texte_integral_raw: pdf_text.to_string(),
            texte_integral_clean: pdf_text.to_string(),
            sections,
            metadata_header: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: Vec::new(),
        };

        Ok(CndaParsed {
            decision,
            source_fields,
            solution_uid,
        })
    } else {
        // ── Fiche-only (aucun PDF accessible) : full_text = abstract éditorial. ──
        let abstract_text = fiche_str(fiche, "editorial_abstract").ok_or_else(|| {
            CoreError::Xml(format!(
                "CNDA {numero}: ni PDF ni abstract éditorial (fiche vidée)"
            ))
        })?;
        // Pas de PDF ⇒ la date vient de la fiche puis, en dernier recours, du slug
        // éditorial (`pdf_url`/`fiche_url`, `cnda-6-mai-2015-…`).
        let lecture_date = cnda_resolve_lecture("", fiche).ok_or_else(|| {
            CoreError::Xml(format!(
                "CNDA {numero}: fiche-only sans date de lecture (ECLI infabricable)"
            ))
        })?;
        let year = cnda_year_from_lecture(&lecture_date).ok_or_else(|| {
            CoreError::Xml(format!(
                "CNDA {numero}: année illisible dans la date de lecture « {lecture_date} »"
            ))
        })?;

        let full_text = clean_texte(abstract_text);
        let source_fields = build_source_fields_cnda(
            fiche,
            None,
            None,
            None,
            None,
            fiche_str(fiche, "audience_date"),
            &lecture_date,
            None,
        );

        let decision = Decision {
            source_uid: format!("cnda/{numero}"),
            member_name: numero.to_string(),
            ecli: Some(format!("ECLI:FR:CNDA:{year}:{numero}")),
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name: Some("Cour nationale du droit d'asile".to_string()),
            jurisdiction_type: Some("CNDA".to_string()),
            jurisdiction_location: None,
            numero_dossier: Some(numero.to_string()),
            numero_dossiers: Some(vec![numero.to_string()]),
            numero_role: None,
            date_lecture: cnda_lecture_to_iso(&lecture_date),
            date_audience: fiche_str(fiche, "audience_date").and_then(cnda_lecture_to_iso),
            date_mise_jour: None,
            formation: None,
            type_decision: None,
            type_recours: None,
            solution: None,
            publication_codes: Vec::new(),
            avocat_requerant: None,
            listed_counsel: Vec::new(),
            texte_integral_raw: abstract_text.to_string(),
            texte_integral_clean: full_text,
            sections: Vec::new(),
            metadata_header: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: vec!["cnda:fiche_only_no_pdf".to_string()],
        };

        Ok(CndaParsed {
            decision,
            source_fields,
            solution_uid: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── CNDA ───────────────────────────────────────────────────────────────

    #[test]
    fn reflow_cnda_pdf_text_rejoins_wrapped_lines_into_logical_paragraphs() {
        let raw = "n° 24001234\n\
                   Vu la procédure suivante :\n\
                   Par un recours enregistré le 4 juillet 2024,\n\
                   l'OFPRA demande l'annulation de la déci-\n\
                   sion attaquée ;\n\
                   n° 24001234\n\
                   Considérant ce qui suit :\n\
                   le requérant soutient qu'il craint des\n\
                   persécutions ;\n\
                   \n\
                   D É C I D E :\n\
                   Article 1er : Le recours est rejeté.";
        let out = reflow_cnda_pdf_text(raw);
        let lines: Vec<&str> = out.lines().collect();
        assert!(!out.contains("n° 24001234"), "{out:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Par un recours") && l.contains("décision attaquée ;")),
            "{out:?}"
        );
        assert!(!out.contains("déci-"), "{out:?}");
        assert!(lines.contains(&"Vu la procédure suivante :"), "{out:?}");
        assert!(lines.contains(&"Considérant ce qui suit :"), "{out:?}");
        assert!(lines.contains(&"DECIDE :"), "{out:?}");
        assert!(
            CNDA_SECTION_PATTERNS
                .iter()
                .find(|(k, _)| *k == "dispositif")
                .map(|(_, re)| re.is_match(&out))
                .unwrap_or(false),
            "dispositif non détecté dans {out:?}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("Article 1er")),
            "{out:?}"
        );
    }

    /// Arrêt multi-requêtes : la liste N° compacte (`N° … – …, … – …`) est rappelée en
    /// tête de chaque page (freq ≥ 3) → strippée par le gate de fréquence. Le
    /// cartouche de titre éclate ces mêmes N° sur des lignes distinctes (freq = 1) →
    /// préservé.
    #[test]
    fn reflow_cnda_pdf_text_strips_repeated_multi_requete_banner() {
        let raw = "COUR NATIONALE DU DROIT D'ASILE\n\
                   N° 19041414 – 18044574\n\
                   N° 19034967 – 18044573\n\
                   Vu la procédure suivante :\n\
                   le requérant soutient qu'il craint des persécutions ;\n\
                   N° 19041414 – 18044574, 19034967 – 18044573\n\
                   Considérant ce qui suit :\n\
                   le recours est fondé ;\n\
                   N° 19041414 – 18044574, 19034967 – 18044573\n\
                   Sur le fond :\n\
                   la demande est accueillie ;\n\
                   N° 19041414 – 18044574, 19034967 – 18044573\n\
                   D É C I D E :\n\
                   Article 1er : Le recours est rejeté.";
        let out = reflow_cnda_pdf_text(raw);
        assert!(
            !out.contains("19041414 – 18044574, 19034967"),
            "running header compact non strippé: {out:?}"
        );
        assert!(out.contains("N° 19041414 – 18044574"), "{out:?}");
        assert!(out.contains("N° 19034967 – 18044573"), "{out:?}");
    }

    #[test]
    fn clean_ocr_markdown_strips_syntax_keeps_paragraph_per_line() {
        let md = "# COUR NATIONALE DU DROIT D'ASILE\n\
                  ![img-0.jpeg](img-0.jpeg)\n\
                  \n\
                  **Vu la procédure suivante :**\n\
                  Par un recours enregistré le 4 juillet 2024, l'OFPRA demande à la Cour d'annuler la décision concernant El Fasher au Darfour.\n\
                  \n\
                  | Champ | Valeur |\n\
                  | --- | --- |\n\
                  | Numéro | 25043827 |\n\
                  \n\
                  DECIDE:\n\
                  Article 1er : La décision est annulée. Voir [le texte](https://x).";
        let out = clean_ocr_markdown(md);
        assert!(
            !out.contains('#') && !out.contains('*') && !out.contains("!["),
            "{out:?}"
        );
        assert!(!out.contains("img-0") && !out.contains("---"), "{out:?}");
        assert!(
            out.contains("El Fasher") && !out.contains("ElFasher"),
            "{out:?}"
        );
        assert!(
            out.contains("le texte") && !out.contains("https://x"),
            "{out:?}"
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines.contains(&"COUR NATIONALE DU DROIT D'ASILE"),
            "{out:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Par un recours") && l.contains("Darfour.")),
            "{out:?}"
        );
        assert!(
            out.contains("Numéro") && out.contains("25043827"),
            "{out:?}"
        );
    }

    #[test]
    fn clean_ocr_markdown_strips_bullets_keeps_numbering() {
        let md = "Vu les pièces suivantes :\n\
                  - le recours enregistré le 4 juillet 2024 ;\n\
                  * la décision contestée de la Cour ;\n\
                  + les autres pièces du dossier.\n\
                  1. Aux termes de l'article L. 511-9 du CESEDA : la Cour statue.\n\
                  2. Il résulte de l'instruction que le recours est recevable.";
        let out = clean_ocr_markdown(md);
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines.contains(&"le recours enregistré le 4 juillet 2024 ;"),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"la décision contestée de la Cour ;"),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"les autres pièces du dossier."),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.starts_with("- ") || l.starts_with("* ") || l.starts_with("+ ")),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"1. Aux termes de l'article L. 511-9 du CESEDA : la Cour statue."),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"2. Il résulte de l'instruction que le recours est recevable."),
            "{lines:?}"
        );
    }

    fn cnda_pdf_text() -> String {
        [
            "COUR NATIONALE DU DROIT D'ASILE",
            "N° 26006334",
            "M. Boidé Président",
            "(6ème section, 2ème chambre)",
            "095-03-01-02-03-03 Appartenance à une minorité nationale ou ethnique",
            "C+",
            "Audience du 30 mars 2026",
            "Lecture du 12 mai 2026",
            "",
            "Vu la procédure suivante : la requête de M. Y.",
            "Vu la convention de Genève du 28 juillet 1951 et le CESEDA ;",
            "Après avoir entendu le rapporteur public à l'audience publique.",
            "Considérant ce qui suit : le requérant établit la réalité des craintes.",
            "DECIDE :",
            "Article 1er : La qualité de réfugié est reconnue à M. Y.",
            "Lu en audience publique le 12 mai 2026.",
            "La République mande et ordonne au ministre de l'intérieur.",
            "La présente décision sera notifiée à M. Y. et au directeur de l'OFPRA.",
        ]
        .join("\n")
    }

    fn cnda_fiche() -> Value {
        json!({
            "titre": "La Cour reconnaît la qualité de réfugié à un ressortissant soudanais…",
            "content_type": "Jurisprudence",
            "editorial_abstract": "En premier lieu… En second lieu… analyse rédigée par les juristes.",
            "fiche_url": "https://www.cnda.fr/decisions-de-justice/jurisprudence/fiche",
            "pdf_url": "https://www.cnda.fr/Media/mediatheque-cnda/images/2026/cnda-12-mai-2026-m.y.-n-26006334-c",
            "publication_date": "1 juin 2026",
        })
    }

    #[test]
    fn cnda_maps_eight_sections_metadata_and_fabricated_ecli() {
        let out = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334").expect("parse CNDA");
        let d = &out.decision;

        assert_eq!(d.source_uid, "cnda/26006334");
        assert_eq!(d.member_name, "26006334");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CNDA"));
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Cour nationale du droit d'asile")
        );
        assert_eq!(d.ecli.as_deref(), Some("ECLI:FR:CNDA:2026:26006334"));
        assert_eq!(d.numero_dossier.as_deref(), Some("26006334"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["26006334".to_string()][..])
        );
        assert_eq!(d.date_lecture.as_deref(), Some("2026-05-12"));
        assert_eq!(d.date_audience.as_deref(), Some("2026-03-30"));
        assert_eq!(d.formation.as_deref(), Some("6ème section, 2ème chambre"));
        assert_eq!(d.texte_integral_clean, cnda_pdf_text());
        assert_eq!(d.texte_integral_raw, cnda_pdf_text());

        let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
        for k in [
            "preamble",
            "procedure",
            "visas",
            "audience",
            "motifs",
            "dispositif",
            "lecture_signature",
            "execution",
            "voies_recours",
        ] {
            assert!(kinds.contains(&k), "section {k} manquante : {kinds:?}");
        }
        let n = d.sections.len();
        assert_eq!(
            d.sections[n - 1].end_char,
            d.texte_integral_clean.chars().count()
        );

        let solution = d.solution.as_deref().expect("dispositif");
        assert!(solution.starts_with("DECIDE"));
        assert!(solution.contains("La qualité de réfugié est reconnue"));
        assert_eq!(
            out.solution_uid.as_deref(),
            Some("solution:SATISFACTION_TOTALE")
        );
        // Spec : `solution_uid` n'a pas de colonne dans `Decision`, il doit donc vivre en
        // `source_fields` — sinon la ré-extraction depuis `(full_text, source_fields)`
        // rendrait autre chose que l'ingest.
        assert_eq!(
            out.source_fields
                .get("solution_uid")
                .and_then(Value::as_str),
            out.solution_uid.as_deref()
        );
    }

    #[test]
    fn cnda_source_fields_carry_classement_importance_and_urls() {
        let out = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334").expect("parse");
        let sf = out.source_fields.as_object().expect("objet");

        assert_eq!(sf.get("classement_code").unwrap(), "095-03-01-02-03-03");
        assert_eq!(
            sf.get("classement_libelle").unwrap(),
            "Appartenance à une minorité nationale ou ethnique"
        );
        assert_eq!(sf.get("importance").unwrap(), "C+");
        assert_eq!(sf.get("formation").unwrap(), "6ème section, 2ème chambre");
        assert_eq!(sf.get("audience_date").unwrap(), "30 mars 2026");
        assert_eq!(sf.get("lecture_date").unwrap(), "12 mai 2026");
        assert_eq!(sf.get("content_type").unwrap(), "Jurisprudence");
        assert!(sf
            .get("editorial_abstract")
            .unwrap()
            .as_str()
            .unwrap()
            .starts_with("En premier lieu"));
        assert!(sf
            .get("pdf_url")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("26006334"));
        assert_eq!(out.decision.publication_codes, vec!["C+".to_string()]);
    }

    #[test]
    fn cnda_fiche_only_uses_abstract_as_full_text() {
        let fiche = json!({
            "titre": "Décision sans PDF accessible",
            "content_type": "Décision de justice",
            "editorial_abstract": "Analyse éditoriale de la décision, en l'absence de PDF.",
            "fiche_url": "https://www.cnda.fr/fiche-sans-pdf",
            "lecture_date": "5 avril 2025",
        });
        let out = parse_cnda("", &fiche, "25001234").expect("parse fiche-only");
        let d = &out.decision;
        assert_eq!(d.ecli.as_deref(), Some("ECLI:FR:CNDA:2025:25001234"));
        assert_eq!(d.date_lecture.as_deref(), Some("2025-04-05"));
        assert_eq!(
            d.texte_integral_clean,
            clean_texte("Analyse éditoriale de la décision, en l'absence de PDF.")
        );
        assert!(d.sections.is_empty());
        assert_eq!(d.solution, None);
        assert_eq!(out.solution_uid, None);
        assert!(d
            .parse_warnings
            .contains(&"cnda:fiche_only_no_pdf".to_string()));
    }

    #[test]
    fn cnda_lecture_to_iso_normalizes_french_dates() {
        assert_eq!(
            cnda_lecture_to_iso("12 mai 2026").as_deref(),
            Some("2026-05-12")
        );
        assert_eq!(
            cnda_lecture_to_iso("1er août 2025").as_deref(),
            Some("2025-08-01")
        );
        assert_eq!(
            cnda_lecture_to_iso("19 décembre 2025").as_deref(),
            Some("2025-12-19")
        );
        assert_eq!(
            cnda_lecture_to_iso("Lu en audience publique le 3 avril 2025.").as_deref(),
            Some("2025-04-03")
        );
        assert_eq!(cnda_lecture_to_iso("seulement 2025").as_deref(), None);
        assert_eq!(cnda_lecture_to_iso("32 mai 2026").as_deref(), None);
    }

    #[test]
    fn cnda_errors_on_empty_numero_missing_sections_or_lecture() {
        assert!(parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "  ").is_err());
        assert!(parse_cnda("texte quelconque sans marqueur", &cnda_fiche(), "1").is_err());
        let no_lecture = "Vu la procédure suivante : x.\nConsidérant ce qui suit : y.\nDECIDE :\nArticle 1er : rejet.";
        let fiche_sans_date = json!({
            "editorial_abstract": "abstract",
            "fiche_url": "https://www.cnda.fr/decisions-de-justice/jurisprudence/fiche-sans-date",
            "pdf_url": "https://www.cnda.fr/Media/mediatheque-cnda/documents/sans-date-n-26006334-c",
        });
        assert!(parse_cnda(no_lecture, &fiche_sans_date, "1").is_err());
        assert!(parse_cnda(no_lecture, &cnda_fiche(), "26006334").is_ok());
    }

    #[test]
    fn cnda_lecture_marker_variants_by_era() {
        let cases = [
            (
                "Lu en audience publique le 8 septembre 2022.",
                "8 septembre 2022",
            ),
            ("Lu en séance publique le 18 avril 2005", "18 avril 2005"),
            ("Lecture 8 septembre 2022", "8 septembre 2022"),
            ("Lecture du 12 mai 2026", "12 mai 2026"),
        ];
        for (text, want) in cases {
            let c = CNDA_LECTURE_RE
                .captures(text)
                .unwrap_or_else(|| panic!("{text:?}"));
            assert_eq!(&c[1], want, "{text:?}");
        }
        assert!(CNDA_LECTURE_RE
            .captures("apprécié dans la lecture du jugement que la cour a rendu")
            .is_none());
    }

    #[test]
    fn cnda_date_from_slug_variants() {
        for (slug, want) in [
            ("cnda-12-mai-2026-m.y.-n-26006334-c", "12 mai 2026"),
            ("crr-sr-17-fevrier-2006-m.-o.-n-406325-r", "17 fevrier 2006"),
            (
                "cnda-ord.-7-janvier-2015-m.-a.-n-14027236-c",
                "7 janvier 2015",
            ),
            (
                "cnda-gf-31-janvier-2014-mme-h.-n-12013217-r",
                "31 janvier 2014",
            ),
        ] {
            assert_eq!(cnda_date_from_slug(slug).as_deref(), Some(want), "{slug}");
        }
        assert_eq!(
            cnda_date_from_slug("la-cnda-evalue-la-credibilite-n-25013796"),
            None
        );
    }

    /// `metadata_header` is empty, so the chunk is `texte_integral_clean` alone and
    /// no metadata enters it, with a PDF as without.
    #[test]
    fn cnda_metadata_never_enters_chunk() {
        let out = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334").expect("parse PDF");
        assert_eq!(out.decision.metadata_header, "");
        assert_eq!(out.decision.texte_integral_clean, cnda_pdf_text());
        assert_eq!(out.decision.date_lecture.as_deref(), Some("2026-05-12"));
        assert!(!out.decision.texte_integral_clean.is_empty());

        let fiche = json!({
            "editorial_abstract": "Analyse éditoriale de la décision.",
            "fiche_url": "https://www.cnda.fr/fiche-sans-pdf",
            "lecture_date": "5 avril 2025",
        });
        let out = parse_cnda("", &fiche, "25001234").expect("parse fiche-only");
        assert_eq!(out.decision.metadata_header, "");
        assert_eq!(out.decision.date_lecture.as_deref(), Some("2025-04-05"));
    }

    /// Re-parsing the same `(pdf_text, fiche, numero)` gives the same fields the
    /// chunk is cut from.
    #[test]
    fn cnda_chunk_fields_are_deterministic() {
        let run = || {
            let d = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334")
                .expect("parse")
                .decision;
            (d.texte_integral_clean, d.metadata_header)
        };
        assert_eq!(run(), run());
    }

    // ── Round-trip `from_source_fields` ⟷ parse direct ──
    //
    // Spec : reconstruire une `Decision` depuis `(full_text, source_fields)` reproduit
    // exactement le parse direct. Vaut PDF (texte intégral) et fiche-only
    // (`full_text` = `clean_texte(abstract)` recalculé).

    #[test]
    fn cnda_round_trips_via_source_fields() {
        let orig = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334").expect("parse PDF");
        let rebuilt = Decision::from_source_fields(
            &orig.decision.texte_integral_clean,
            &orig.source_fields,
            &orig.decision.source_uid,
        );
        assert_eq!(orig.decision, rebuilt);

        let fiche = json!({
            "titre": "Décision sans PDF accessible",
            "content_type": "Décision de justice",
            "editorial_abstract": "Analyse éditoriale de la décision, en l'absence de PDF.",
            "fiche_url": "https://www.cnda.fr/fiche-sans-pdf",
            "lecture_date": "5 avril 2025",
        });
        let orig = parse_cnda("", &fiche, "25001234").expect("parse fiche-only");
        let rebuilt = Decision::from_source_fields(
            &orig.decision.texte_integral_clean,
            &orig.source_fields,
            &orig.decision.source_uid,
        );
        assert_eq!(orig.decision, rebuilt);
    }

    #[test]
    fn cnda_solution_mapping_variants() {
        let mk = |dispo: &str| -> Option<String> { cnda_solution_uid(Some(dispo)) };
        // Spec : une révision porte les marqueurs d'accueil de la décision qu'elle
        // défait.
        assert_eq!(
            mk(
                "DECIDE: Article 1er : Le recours en révision du directeur général de l'OFPRA \
                est admis. Article 2 : La décision de la Cour reconnaissant à M. T. la qualité \
                de réfugié est déclarée nulle et non avenue. Article 3 : Le recours de M. T. \
                est rejeté."
            ),
            None
        );
        assert_eq!(
            mk(
                "DECIDE: Article 1er : Les décisions de la Cour reconnaissant la qualité de \
                réfugié sont déclarées nulles et non avenues."
            ),
            None
        );
        assert_eq!(
            mk("La qualité de réfugié est reconnue").as_deref(),
            Some("solution:SATISFACTION_TOTALE")
        );
        assert_eq!(
            mk("Le bénéfice de la protection subsidiaire est accordé").as_deref(),
            Some("solution:SATISFACTION_TOTALE")
        );
        assert_eq!(
            mk("Annule la décision de l'OFPRA").as_deref(),
            Some("solution:SATISFACTION_TOTALE")
        );
        assert_eq!(
            mk("Le recours est rejeté").as_deref(),
            Some("solution:REJET")
        );
        assert_eq!(mk("L'intéressé est exclu du statut de réfugié"), None);
        assert_eq!(cnda_solution_uid(None), None);
    }
}
