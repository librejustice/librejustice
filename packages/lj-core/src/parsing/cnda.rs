//! Sous-module de `parsing` (#26, découpe ADR 0066). Aucune logique changée :
//! déplacement depuis `parsing.rs`, accès aux helpers partagés via `super`.

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// OCR markdown → texte propre. La CNDA passe par l'OCR Mistral (`/v1/ocr`) : un
// markdown par page (titres `#`, gras `**`, listes, parfois images `![…](…)` /
// tables `|…|`) où **chaque paragraphe logique tient sur une ligne** (≠ les wraps
// visuels de l'extraction PDF native, qui rendaient chaque ligne coupée comme un
// faux paragraphe). On retire le balisage en **préservant `ligne = paragraphe`**
// puis on applique `clean_texte` (le normaliseur de l'opendata) → texte rendu à
// l'identique des autres sources (`decision_paragraphs` = une ligne par paragraphe).
// ─────────────────────────────────────────────────────────────────────────────

/// Image markdown `![alt](src)` — retirée (placeholder inutile, `include_image_base64=false`).
static MD_IMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
/// Lien markdown `[texte](url)` → `texte`.
static MD_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
/// Marqueur de titre en début de ligne (`#`..`######` + espace) — retiré, le texte
/// du titre devient un paragraphe.
static MD_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]{0,3}#{1,6}[ \t]+").unwrap());
/// Marqueur de liste à puces en début de ligne (`-`/`*`/`+` + espace) — retiré,
/// l'item devient un paragraphe nu (rendu opendata des items « Vu »/moyens). NE
/// touche PAS la numérotation `1.`/`2.` des considérants (numérotation sémantique
/// de la Cour, conservée). Une règle horizontale `---` n'a pas d'espace après le
/// marqueur ⇒ non matchée ici, retirée par [`is_md_noise_line`].
static MD_LIST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]{0,3}[-*+][ \t]+").unwrap());

/// Vrai si la ligne (trimée) est du bruit markdown structurel à retirer : séparateur
/// de table (`|---|:--:|`) ou règle horizontale (`---`/`***`/`___`).
fn is_md_noise_line(trimmed: &str) -> bool {
    if trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') {
        return true; // règle horizontale
    }
    trimmed.contains('|') && trimmed.chars().all(|c| matches!(c, '|' | ':' | '-' | ' '))
}

/// Nettoie le markdown OCR en texte (strip balisage, `ligne = paragraphe`) puis
/// `clean_texte`. Voir le bloc ci-dessus.
pub fn clean_ocr_markdown(md: &str) -> String {
    let no_img = MD_IMAGE_RE.replace_all(md, "");
    let no_link = MD_LINK_RE.replace_all(&no_img, "$1");
    let no_head = MD_HEADING_RE.replace_all(&no_link, "");
    let no_list = MD_LIST_RE.replace_all(&no_head, "");
    let stripped = no_list
        .lines()
        .filter(|l| !is_md_noise_line(l.trim()))
        // Gras `**`/`__` retirés ; barres de table → espace (le reste de la ligne
        // de données d'une table est conservé comme texte).
        .map(|l| l.replace("**", "").replace("__", "").replace('|', " "))
        .collect::<Vec<_>>()
        .join("\n");
    clean_texte(&stripped)
}

// ─────────────────────────────────────────────────────────────────────────────
// PDF natif-texte → recollage déterministe (ADR 0124). `pdftotext` (poppler, bord
// lj-sources) rend un texte fidèle mais conserve les **retours de ligne visuels**
// (chaque ligne du PDF = une ligne, une phrase est coupée en plusieurs lignes) et
// répète les en-têtes/pieds de page. On reconstruit les paragraphes logiques par
// règles — un nouveau paragraphe à chaque marqueur structurel CNDA (gabarit
// administratif type CE) ou après une fin de clause (`;`/`.`/`:`) — pour rendre le
// texte à l'identique du chemin OCR (`ligne = paragraphe`), sans dépendance ni
// non-déterminisme. Mesuré à 99,4 % de similarité token médiane vs OCR.
// ─────────────────────────────────────────────────────────────────────────────

/// Début d'un nouveau paragraphe logique CNDA : marqueurs du gabarit (visas,
/// considérants, dispositif, articles, signataires) + numérotation. En tête de
/// ligne (le texte est déjà découpé par ligne).
static CNDA_PARA_START_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(Vu\b|Considérant\b|Sur\b|Après\b|D\s*[ÉE]\s*C\s*I\s*D\s*E\b|DÉCIDE\b|DECIDE\b|Article\s|ARTICLE\s|Art\.\s|M\.\s|Mme\b|Mlle\b|MM\.\s|N°\s*\d|n°\s*\d|Lu\b|Délibéré\b|En conséquence\b|PAR CES MOTIFS\b|La présente\b|La République\b|\d+\.\s|\d+\)\s|[a-z]\)\s)",
    )
    .unwrap()
});

/// Ligne d'en-tête/pied de page répétée (numéro de page, `n° NNNN` seul, filets,
/// bandeau juridiction, **liste N° multi-requêtes** `N° 17010844 – 18044574,
/// 17010847 – …` rappelée en tête de chaque page) — retirée du flux. N'est
/// supprimée que si répétée ≥ 3× (cf. [`reflow_cnda_pdf_text`]) : le cartouche de
/// titre, qui éclate ces mêmes N° sur des lignes distinctes (freq = 1), survit
/// au gate de fréquence, alors que la liste compacte rappelée à chaque page tombe.
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
/// `DECIDE :`) → normalisé en `DECIDE :` pour la détection de section (un seul
/// gabarit en aval, `CNDA_SECTION_PATTERNS`).
static CNDA_DECIDE_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*D\s*[ÉE]\s*C\s*I\s*D\s*E\s*:?\s*$").unwrap());

/// Recolle le texte `pdftotext` d'une décision CNDA en paragraphes logiques
/// (ADR 0124). Déterministe, pur. Voir le bloc ci-dessus.
///
/// Algorithme : form-feeds → sauts de ligne ; suppression des lignes d'en-tête/
/// pied (motif pur, ou répétées ≥ 3× = bandeau de page) ; rejointure ligne→ligne
/// sauf nouveau paragraphe sur marqueur ([`CNDA_PARA_START_RE`]) ou après une fin
/// de clause (`;`/`.`/`:`) ; dispositif normalisé `DECIDE :` ; recollage des
/// césures de fin de ligne (`mot-` + minuscule → `mot`).
pub fn reflow_cnda_pdf_text(raw: &str) -> String {
    let normalized = raw.replace('\u{c}', "\n");
    let lines: Vec<&str> = normalized.lines().map(str::trim).collect();

    // Comptage pour repérer les lignes d'en-tête/pied répétées à chaque page.
    let mut freq: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for l in &lines {
        if !l.is_empty() {
            *freq.entry(*l).or_insert(0) += 1;
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<String>| {
        if !buf.is_empty() {
            out.push(std::mem::take(buf));
        }
    };

    for line in lines {
        if line.is_empty() {
            // Une ligne vide ne **ferme pas** un paragraphe en cours : `pdftotext`
            // en insère une à chaque frontière de page, en plein milieu d'une
            // phrase à cheval sur deux pages. On continue d'accumuler ; la fin de
            // paragraphe réelle est portée par la ponctuation finale (`;`/`.`/`:`)
            // ou un marqueur (gérés ci-dessous), pas par le blanc. (Mesuré : flush
            // sur blanc coupait 2× plus de phrases — ADR 0124.)
            continue;
        }
        // Bruit d'en-tête/pied : motif pur (toujours), ou bandeau répété (≥ 3×).
        if CNDA_HEADER_PURE_RE.is_match(line)
            || (CNDA_HEADER_NOISE_RE.is_match(line) && freq.get(line).copied().unwrap_or(0) >= 3)
        {
            continue;
        }
        if CNDA_DECIDE_LINE_RE.is_match(line) {
            flush(&mut buf, &mut out);
            out.push("DECIDE :".to_string());
            continue;
        }
        let new_para = CNDA_PARA_START_RE.is_match(line)
            || buf.ends_with(';')
            || buf.ends_with('.')
            || buf.ends_with(':');
        if new_para {
            flush(&mut buf, &mut out);
            buf.push_str(line);
        } else if buf.is_empty() {
            buf.push_str(line);
        } else if buf.ends_with('-') && line.starts_with(|c: char| c.is_lowercase()) {
            // Césure de fin de ligne : recolle sans espace, retire le tiret.
            buf.pop();
            buf.push_str(line);
        } else {
            buf.push(' ');
            buf.push_str(line);
        }
    }
    flush(&mut buf, &mut out);
    out.join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────
// CNDA (Cour nationale du droit d'asile, asile/protection subsidiaire) — ADR 0096.
//
// Parser PUR : reçoit le texte issu de l'OCR Mistral déjà nettoyé au bord
// (`lj-core::clean_ocr_markdown`, appliqué par `lj-ingest`) et les métadonnées de
// la fiche HTML éditoriale désérialisées (`fiche` : titre éditorial, content_type,
// editorial_abstract, fiche_url, pdf_url, date de publication). Émet un
// [`CndaParsed`] : la `Decision` (modèle 0085), les `source_fields` (métadonnées
// hors texte) et la `solution_uid` best-effort. Aucune I/O, aucune dép native — le
// texte est extrait en amont.
//
// La CNDA est absente de Judilibre/opendata (audit `cnda.md`) : juridiction neuve
// `juridiction_type = "CNDA"`, ECLI **fabriqué** `ECLI:FR:CNDA:{année}:{numero}`
// (la Cour n'en émet aucun), date indexée = date de **lecture** du PDF (jamais la
// date de mise en ligne de la fiche, piège audit §43/85).
// ─────────────────────────────────────────────────────────────────────────────

/// Les 8 marqueurs de section d'une décision CNDA (audit `cnda.md` §66-70 ;
/// gabarit administratif type CE, ordre préservé). `kind`, regex. IGNORECASE ;
/// le dispositif marqué `DECIDE` est ancré en tête de ligne (MULTILINE) — le
/// recollage `D E C I D E :` → `DECIDE :` a déjà eu lieu au bord (`lj-sources`).
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
            // Tolère l'accent (`DÉCIDE`, rendu par pdftotext) comme l'absence
            // (`DECIDE`, OCR), et le lettrinage `D É C I D E` (les deux bords).
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

/// `(code, libellé)` du classement nomenclature `095-…` (équivalent du plan de
/// classement CE/JADE). Capture le code hiérarchique puis le libellé qui suit.
static CNDA_CLASSEMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(095(?:-\d{2})+)\s+(.+)").unwrap());

/// Importance jurisprudentielle CNDA (`C` / `C+` / `R`), en tête de ligne.
static CNDA_IMPORTANCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(C\+|C|R)\s*$").unwrap());

/// Formation entre parenthèses (`(6ème section, 2ème chambre)`), audit §62.
static CNDA_FORMATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([^()]*\bchambre\b[^()]*)\)").unwrap());

/// Date de lecture (date indexée, ADR 0096), capturée APRÈS un marqueur de
/// prononcé sous toutes ses formes selon l'époque/OCR : `Lu en audience publique
/// le …` (CNDA moderne), `Lu en séance publique le …` (CRR ancien), `Lecture du
/// …` / `Lecture …` (en-tête, OCR sans « du »). Le groupe 1 = la date FR
/// (`8 septembre 2022`) ; **exiger** une vraie date jour-mois-année après le
/// marqueur écarte les faux positifs (`Lecture du jugement …`, audit 09007100).
static CNDA_LECTURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:Lu en (?:audience|séance) publique le|Lecture(?:\s+du)?)\s+((?:1er|\d{1,2})\s+[a-zàâäéèêëîïôöùûüç]+\s+\d{4})",
    )
    .unwrap()
});

/// Date d'audience (`Audience du 30 mars 2026`, `séance publique du …` CRR),
/// conservée en `source_fields`. Groupe 1 = date FR.
static CNDA_AUDIENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:Audience du|(?:séance|audience) publique du)\s+((?:1er|\d{1,2})\s+[a-zàâäéèêëîïôöùûüç]+\s+\d{4})",
    )
    .unwrap()
});

/// Date FR (`6 mai 2015`) lue dans un slug éditorial CNDA/CRR
/// (`cnda-6-mai-2015-…`, `crr-sr-17-fevrier-2006-…`, `cnda-ord.-7-janvier-2015-…`).
/// Le slug porte la **date de décision** (= lecture), pas la date de mise en ligne
/// (piège ADR 0096 §43/85). Le mois est validé par [`cnda_fr_month`] pour ne pas
/// capter un faux triplet chiffre-mot-chiffre.
static CNDA_SLUG_DATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d{1,2})-([a-zàâäéèêëîïôöùûüç]+)-(\d{4})\b").unwrap());

/// Année (4 chiffres) d'une date FR libre (`12 mai 2026` → `2026`), pour l'ECLI.
static CNDA_YEAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{4})\b").unwrap());

/// Date FR libre `jour mois année` (`12 mai 2026`, `1er août 2025`) dans une chaîne
/// `Lecture du …`. Sert à produire la date indexée ISO.
static CNDA_FR_DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d{1,2}|1er)\s+([a-zàâäéèêëîïôöùûüç]+)\s+(\d{4})\b").unwrap()
});

/// Mois français → numéro (1–12). Casse/accents tolérés. `None` si inconnu.
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

/// Date de lecture FR libre (`12 mai 2026`) → ISO `2026-05-12`, pour la colonne
/// `date_lecture` **indexée** : la frontière store ne parse que `%Y-%m-%d` (#12) et
/// jette toute autre forme en NULL — une décision sans date indexée casse le tri /
/// filtre / law-at-date. La forme FR brute reste en `source_fields.lecture_date`
/// (audit). `None` si jour/mois/année illisibles (date validée par `jiff`).
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

/// Date FR libre (`6 mai 2015`) lue dans un slug éditorial. `None` si le slug ne
/// porte pas de triplet `jour-mois-année` à mois français valide.
fn cnda_date_from_slug(slug: &str) -> Option<String> {
    let c = CNDA_SLUG_DATE_RE.captures(slug)?;
    cnda_fr_month(&c[2])?; // valide le mois (sinon ce n'est pas une date)
    Some(format!("{} {} {}", &c[1], &c[2], &c[3]))
}

/// Date de lecture FR libre, par ordre de fiabilité décroissante :
/// 1. marqueur de prononcé dans le texte OCR ([`CNDA_LECTURE_RE`]) ;
/// 2. `lecture_date` de la fiche (rarement présent) ;
/// 3. date du **slug** éditorial (`pdf_url` puis `fiche_url`) — date de décision,
///    pas de mise en ligne (ADR 0096 §43/85).
///
/// `None` si aucune source ne donne de date (ECLI infabricable → skip amont).
fn cnda_resolve_lecture(pdf_text: &str, fiche: &Value) -> Option<String> {
    CNDA_LECTURE_RE
        .captures(pdf_text)
        .map(|c| c[1].trim().to_string())
        .or_else(|| fiche_str(fiche, "lecture_date").map(str::to_string))
        .or_else(|| fiche_str(fiche, "pdf_url").and_then(cnda_date_from_slug))
        .or_else(|| fiche_str(fiche, "fiche_url").and_then(cnda_date_from_slug))
}

/// Découpe le texte PDF CNDA en sections via les 8 marqueurs réguliers
/// (audit §66-70). Même logique d'offsets/dédup que [`extract_sections_xml`] :
/// premier match par `kind`, tri par position, préambule = en-tête avant le
/// premier marqueur. Offsets en codepoints.
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

/// Année de la décision depuis la date de lecture libre (`12 mai 2026` → `2026`),
/// pour l'ECLI fabriqué. `None` si aucun millésime à 4 chiffres.
fn cnda_year_from_lecture(lecture: &str) -> Option<String> {
    CNDA_YEAR_RE.captures(lecture).map(|c| c[1].to_string())
}

/// Mapping best-effort du dispositif CNDA → uid `solution:*` (ADR 0096
/// décision #10 ; vocabulaire référentiel émis directement depuis v12, ADR
/// 0148). Reconnaissance qualité réfugié / protection subsidiaire / annulation
/// OFPRA ⇒ le recours OFPRA aboutit (`solution:SATISFACTION_TOTALE`) ; rejet ⇒
/// `solution:REJET`. Exclusion / révision ⇒ `None` (signal ambigu) : pas de
/// repli.
fn cnda_solution_uid(dispositif: Option<&str>) -> Option<String> {
    let raw = dispositif?.to_lowercase();
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

/// Lit une chaîne non-vide d'une clé de la fiche (`Value` objet).
fn fiche_str<'a>(fiche: &'a Value, key: &str) -> Option<&'a str> {
    fiche
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Résultat du parse CNDA (ADR 0096) — tout ce que le pipeline upserte pour une
/// décision : la `Decision` (modèle 0085), les `source_fields` (métadonnées hors
/// texte, rendues via ADR 0090) et la solution best-effort. `source_fields`
/// et `solution_uid` débordent de `Decision` (#11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CndaParsed {
    pub decision: Decision,
    pub source_fields: Value,
    /// Uid `solution:*` (`solution:SATISFACTION_TOTALE`…) ou `None`.
    pub solution_uid: Option<String>,
}

/// Construit les `source_fields` CNDA (ADR 0085/0090, hors texte) : abstract
/// éditorial, classement nomenclature (code + libellé), importance, formation,
/// dates audience/lecture, URLs fiche/PDF, content_type, titre éditorial. Les
/// clés absentes sont omises (objet compact).
#[allow(clippy::too_many_arguments)]
fn build_source_fields_cnda(
    fiche: &Value,
    classement_code: Option<&str>,
    classement_libelle: Option<&str>,
    importance: Option<&str>,
    formation: Option<&str>,
    audience_date: Option<&str>,
    lecture_date: &str,
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
    out.insert(
        "lecture_date".to_string(),
        Value::String(lecture_date.to_string()),
    );
    Value::Object(out)
}

/// `true` si `source_uid` provient de la CNDA (ADR 0096) : préfixe `cnda/`, posé
/// par [`parse_cnda`]. Discriminant de famille du dispatch
/// [`Decision::from_source_fields`] — sûr car stable en DB et jamais porté par
/// les autres fonds.
pub(crate) fn source_uid_is_cnda(source_uid: &str) -> bool {
    source_uid.starts_with("cnda/")
}

/// Reconstruit la `fiche` (clés d'origine lues par [`parse_cnda`]) depuis les
/// `source_fields` CNDA (clés renommées par [`build_source_fields_cnda`] :
/// `titre_editorial`←`titre`, `fiche_publication_date`←`publication_date`). Les
/// autres clés (`editorial_abstract`, `content_type`, `fiche_url`, `pdf_url`,
/// `lecture_date`, `audience_date`) sont conservées telles quelles.
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

impl Decision {
    /// Branche CNDA de [`Decision::from_source_fields`] (ADR 0096/0085) :
    /// reconstruit une `Decision` **identique** à [`parse_cnda`] depuis
    /// `(full_text, source_fields)`. Pendant exact de [`build_source_fields_cnda`].
    ///
    /// Parité **structurelle** : [`parse_cnda`] pose `metadata_header = visa_trim
    /// = ""`, donc le chunk ne dépend que de `texte_integral_clean` (= `full_text`,
    /// stocké verbatim). On réinvoque le parser dédié — `numero` est dérivé du
    /// `source_uid` (`cnda/<numero>`), la `fiche` reconstruite des `source_fields`,
    /// le `pdf_text` est `full_text` si celui-ci porte la structure CNDA (8
    /// marqueurs détectés) sinon `""` (décision fiche-only, dont `full_text` =
    /// `clean_texte(abstract)` que [`parse_cnda`] recalcule depuis la fiche).
    ///
    /// **Hors périmètre du re-extract** : `solution_uid` (best-effort sur le
    /// dispositif) déborde de `Decision` et est posé à l'ingest via
    /// `prebuilt_extracted`. Il n'est PAS reproductible par
    /// `lj_extract::extract` (CNDA → famille générique, qui rend
    /// `None`). La parité re-embed (chunk) reste prouvée indépendamment : elle ne
    /// dépend que des trois champs du chunker, tous reproduits ici à l'identique.
    ///
    /// Panique si le parser échoue : `(full_text, source_fields)` provient d'une
    /// décision déjà ingérée (donc valide) — un échec signale une corruption DB ou
    /// un mauvais dispatch, pas une donnée externe (#12).
    pub(crate) fn from_source_fields_cnda(
        full_text: &str,
        source_fields: &Value,
        source_uid: &str,
    ) -> Decision {
        let numero = source_uid
            .strip_prefix("cnda/")
            .unwrap_or_else(|| panic!("from_source_fields_cnda: source_uid non CNDA {source_uid}"));
        let fiche = cnda_fiche_from_source_fields(source_fields);
        // PDF si `full_text` porte la structure CNDA (8 marqueurs) ; sinon
        // fiche-only (`full_text` = `clean_texte(abstract)`, recalculé par le
        // parser depuis `fiche["editorial_abstract"]`).
        let pdf_text = if extract_sections_cnda(full_text).is_empty() {
            ""
        } else {
            full_text
        };
        parse_cnda(pdf_text, &fiche, numero)
            .unwrap_or_else(|e| panic!("from_source_fields CNDA {source_uid}: {e}"))
            .decision
    }
}

/// Parse une décision CNDA (ADR 0096) en [`CndaParsed`].
///
/// `pdf_text` = texte issu de l'OCR Mistral déjà nettoyé au bord
/// (`clean_ocr_markdown`) ; vide ⇒ décision fiche-only (`full_text` =
/// abstract éditorial de la fiche, `payload_format` `html` côté ingest). `fiche`
/// = métadonnées de la fiche HTML désérialisées. `numero` = numéro de décision
/// (clé robuste, triple source : slug PDF / en-tête / `/Title`). PUR.
///
/// `juridiction_type` = `"CNDA"` (posé explicitement). `ecli` =
/// `ECLI:FR:CNDA:{année}:{numero}` (année dérivée de la date de **lecture**, la
/// Cour n'émet aucun ECLI). `date_lecture` résolue par [`cnda_resolve_lecture`]
/// (marqueur de prononcé du texte OCR, sinon date du slug éditorial).
/// `numero_dossiers` = `[numero]`. `solution` = dispositif verbatim.
///
/// Frontière de validation **unique** (AGENTS.md #12, garde anti-fragilité ADR
/// 0096) : erreur franche [`CoreError::Xml`] si le numéro est vide, ou — sur une
/// décision avec PDF — si aucune des 8 sections n'est détectée ou si la date de
/// lecture est introuvable. Un parse qui « réussit » sur une page vidée est pire
/// qu'un échec.
///
/// ## Contrat de stabilité du chunk (anti-re-embed)
///
/// Le texte envoyé à l'embedder est dérivé de trois champs de [`Decision`] :
/// `texte_integral_clean` (corps), `metadata_header` et `visa_trim`. Ce parser
/// pose **`metadata_header = ""` et `visa_trim = ""`** pour la CNDA : le chunk se
/// réduit donc à `texte_integral_clean` (= le `pdf_text` OCR verbatim avec
/// PDF, ou `clean_texte(abstract)` en fiche-only). **Aucune métadonnée n'entre
/// dans le chunk.**
///
/// Conséquence : dates, ECLI, classement, importance, formation, URLs vivent
/// **uniquement** dans `source_fields` et les colonnes de [`Decision`] — hors
/// chunk, donc modifiables sans re-embed. `date_lecture`/`date_audience` sont
/// normalisées ISO `YYYY-MM-DD` (la frontière store ne parse que `%Y-%m-%d`) via
/// [`cnda_lecture_to_iso`] ; la forme FR libre (`12 mai 2026`) reste dans
/// `source_fields.lecture_date`/`audience_date` pour l'audit.
pub fn parse_cnda(pdf_text: &str, fiche: &Value, numero: &str) -> crate::error::Result<CndaParsed> {
    use crate::error::CoreError;

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
        );

        let decision = Decision {
            source_uid: format!("cnda/{numero}"),
            member_name: numero.to_string(),
            ecli: Some(format!("ECLI:FR:CNDA:{year}:{numero}")),
            juridiction_code: None,
            juridiction_nom: Some("Cour nationale du droit d'asile".to_string()),
            juridiction_type: Some("CNDA".to_string()),
            juridiction_location: None,
            numero_dossier: Some(numero.to_string()),
            numero_dossiers: Some(vec![numero.to_string()]),
            numero_role: None,
            // ISO pour la colonne indexée (la forme FR brute reste en source_fields).
            date_lecture: cnda_lecture_to_iso(&lecture_date),
            date_audience: audience_date.as_deref().and_then(cnda_lecture_to_iso),
            date_mise_jour: None,
            formation,
            type_decision: None,
            type_recours: None,
            solution: dispositif,
            publication_codes: importance.map(|i| vec![i]).unwrap_or_default(),
            avocat_requerant: None,
            texte_integral_raw: pdf_text.to_string(),
            texte_integral_clean: pdf_text.to_string(),
            sections,
            metadata_header: String::new(),
            visa_trim: String::new(),
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
        // Le PDF (texte intégral + dates de lecture/audience) manque : l'abstract
        // de la fiche est la seule substance. La date de lecture est introuvable
        // → ECLI infabricable → erreur franche (#12, pas de ligne vide).
        let abstract_text = fiche_str(fiche, "editorial_abstract").ok_or_else(|| {
            CoreError::Xml(format!(
                "CNDA {numero}: ni PDF ni abstract éditorial (fiche vidée)"
            ))
        })?;
        // Pas de PDF ⇒ la date vient de la fiche puis, en dernier recours, du slug
        // éditorial (`pdf_url`/`fiche_url`, `cnda-6-mai-2015-…`). En prod la fiche
        // ne porte quasi jamais `lecture_date` : le slug est la source réelle.
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
        );

        let decision = Decision {
            source_uid: format!("cnda/{numero}"),
            member_name: numero.to_string(),
            ecli: Some(format!("ECLI:FR:CNDA:{year}:{numero}")),
            juridiction_code: None,
            juridiction_nom: Some("Cour nationale du droit d'asile".to_string()),
            juridiction_type: Some("CNDA".to_string()),
            juridiction_location: None,
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
            texte_integral_raw: abstract_text.to_string(),
            texte_integral_clean: full_text,
            sections: Vec::new(),
            metadata_header: String::new(),
            visa_trim: String::new(),
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

    // ── CNDA (ADR 0096) ─────────────────────────────────────────────────────

    #[test]
    fn reflow_cnda_pdf_text_rejoins_wrapped_lines_into_logical_paragraphs() {
        // pdftotext rend les retours de ligne visuels : une clause "Considérant"
        // coupée sur 3 lignes, un en-tête de page répété, le dispositif lettriné,
        // une césure de fin de ligne.
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
        // En-tête "n° NNNN" répété supprimé (motif pur).
        assert!(!out.contains("n° 24001234"), "{out:?}");
        // Clause "Par un recours … attaquée ;" recollée en UNE ligne, césure
        // "déci-\nsion" recollée en "décision".
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Par un recours") && l.contains("décision attaquée ;")),
            "{out:?}"
        );
        assert!(!out.contains("déci-"), "{out:?}");
        // Marqueurs de section sur leur propre ligne (nouveau paragraphe).
        assert!(lines.contains(&"Vu la procédure suivante :"), "{out:?}");
        assert!(lines.contains(&"Considérant ce qui suit :"), "{out:?}");
        // Dispositif lettriné/accentué normalisé en "DECIDE :" et détecté par le
        // pattern de section.
        assert!(lines.contains(&"DECIDE :"), "{out:?}");
        assert!(
            CNDA_SECTION_PATTERNS
                .iter()
                .find(|(k, _)| *k == "dispositif")
                .map(|(_, re)| re.is_match(&out))
                .unwrap_or(false),
            "dispositif non détecté dans {out:?}"
        );
        // Article = nouveau paragraphe.
        assert!(
            lines.iter().any(|l| l.starts_with("Article 1er")),
            "{out:?}"
        );
    }

    /// Arrêt multi-requêtes : la liste N° compacte (`N° … – …, … – …`) est
    /// rappelée en tête de **chaque page** (freq ≥ 3) → strippée par le gate de
    /// fréquence. Le cartouche de titre éclate ces mêmes N° sur des lignes
    /// distinctes (freq = 1) → préservé. (ADR 0124 — trou découvert sur 3/462
    /// décisions : numéro de page nu sur sa propre ligne, liste séparée par
    /// virgules/tirets, pas par espaces comme je l'avais d'abord supposé.)
    #[test]
    fn reflow_cnda_pdf_text_strips_repeated_multi_requete_banner() {
        // Liste compacte rappelée 3× (running header), cartouche de titre éclaté 1×.
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
        // Running header compact (freq 3) supprimé.
        assert!(
            !out.contains("19041414 – 18044574, 19034967"),
            "running header compact non strippé: {out:?}"
        );
        // Cartouche de titre éclaté (freq 1 chacun) préservé.
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
        // Balisage retiré.
        assert!(
            !out.contains('#') && !out.contains('*') && !out.contains("!["),
            "{out:?}"
        );
        // Image et séparateur de table supprimés.
        assert!(!out.contains("img-0") && !out.contains("---"), "{out:?}");
        // Nom propre préservé séparé (le bug recollage `ElFasher` ne doit PAS exister).
        assert!(
            out.contains("El Fasher") && !out.contains("ElFasher"),
            "{out:?}"
        );
        // Lien réduit à son texte.
        assert!(
            out.contains("le texte") && !out.contains("https://x"),
            "{out:?}"
        );
        // Titre conservé comme paragraphe (sur sa propre ligne).
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines.contains(&"COUR NATIONALE DU DROIT D'ASILE"),
            "{out:?}"
        );
        // Paragraphe = une ligne : la longue phrase OFPRA n'est pas coupée.
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Par un recours") && l.contains("Darfour.")),
            "{out:?}"
        );
        // Données de table conservées comme texte (barres → espaces).
        assert!(
            out.contains("Numéro") && out.contains("25043827"),
            "{out:?}"
        );
    }

    #[test]
    fn clean_ocr_markdown_strips_bullets_keeps_numbering() {
        // Items « Vu »/moyens en puces `-`/`*` : marqueur retiré, item = paragraphe
        // nu (rendu opendata). Numérotation `1.`/`2.` des considérants : conservée.
        let md = "Vu les pièces suivantes :\n\
                  - le recours enregistré le 4 juillet 2024 ;\n\
                  * la décision contestée de la Cour ;\n\
                  + les autres pièces du dossier.\n\
                  1. Aux termes de l'article L. 511-9 du CESEDA : la Cour statue.\n\
                  2. Il résulte de l'instruction que le recours est recevable.";
        let out = clean_ocr_markdown(md);
        let lines: Vec<&str> = out.lines().collect();
        // Puces retirées : l'item commence directement par son texte.
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
        // Aucune ligne ne commence par un marqueur de puce résiduel.
        assert!(
            !lines
                .iter()
                .any(|l| l.starts_with("- ") || l.starts_with("* ") || l.starts_with("+ ")),
            "{lines:?}"
        );
        // Numérotation sémantique conservée intacte.
        assert!(
            lines.contains(&"1. Aux termes de l'article L. 511-9 du CESEDA : la Cour statue."),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"2. Il résulte de l'instruction que le recours est recevable."),
            "{lines:?}"
        );
    }

    /// Texte PDF CNDA recollé (bord lj-sources) : en-tête + les 8 sections
    /// régulières (audit §66-70). `D E C I D E :` déjà recollé en `DECIDE :`.
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

    /// Métadonnées de la fiche HTML éditoriale désérialisées.
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
        assert_eq!(d.juridiction_type.as_deref(), Some("CNDA"));
        assert_eq!(
            d.juridiction_nom.as_deref(),
            Some("Cour nationale du droit d'asile")
        );
        // ECLI fabriqué : année dérivée de la date de LECTURE (2026), pas de la
        // date de publication de la fiche (1 juin 2026 — piège, mais même année ici).
        assert_eq!(d.ecli.as_deref(), Some("ECLI:FR:CNDA:2026:26006334"));
        assert_eq!(d.numero_dossier.as_deref(), Some("26006334"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["26006334".to_string()][..])
        );
        // Date indexée = lecture ; audience conservée distincte.
        // date indexée normalisée ISO (la forme FR brute reste en source_fields).
        assert_eq!(d.date_lecture.as_deref(), Some("2026-05-12"));
        assert_eq!(d.date_audience.as_deref(), Some("2026-03-30"));
        assert_eq!(d.formation.as_deref(), Some("6ème section, 2ème chambre"));
        // full_text = pdf_text recollé verbatim (indexé BM25).
        assert_eq!(d.texte_integral_clean, cnda_pdf_text());
        assert_eq!(d.texte_integral_raw, cnda_pdf_text());

        // Les 8 marqueurs détectés (+ préambule en tête).
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
        // Sections triées par offset, contiguës et couvrant la fin du texte.
        let n = d.sections.len();
        assert_eq!(
            d.sections[n - 1].end_char,
            d.texte_integral_clean.chars().count()
        );

        // solution = dispositif verbatim ; solution_uid best-effort.
        let solution = d.solution.as_deref().expect("dispositif");
        assert!(solution.starts_with("DECIDE"));
        assert!(solution.contains("La qualité de réfugié est reconnue"));
        assert_eq!(
            out.solution_uid.as_deref(),
            Some("solution:SATISFACTION_TOTALE")
        );
    }

    #[test]
    fn cnda_source_fields_carry_classement_importance_and_urls() {
        let out = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334").expect("parse");
        let sf = out.source_fields.as_object().expect("objet");

        // Classement nomenclature = code + libellé (séparés).
        assert_eq!(sf.get("classement_code").unwrap(), "095-03-01-02-03-03");
        assert_eq!(
            sf.get("classement_libelle").unwrap(),
            "Appartenance à une minorité nationale ou ethnique"
        );
        assert_eq!(sf.get("importance").unwrap(), "C+");
        assert_eq!(sf.get("formation").unwrap(), "6ème section, 2ème chambre");
        assert_eq!(sf.get("audience_date").unwrap(), "30 mars 2026");
        assert_eq!(sf.get("lecture_date").unwrap(), "12 mai 2026");
        // Métadonnées de la fiche éditoriale (rendues via ADR 0090).
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
        // Importance reportée aussi en publication_codes (facette grands arrêts).
        assert_eq!(out.decision.publication_codes, vec!["C+".to_string()]);
    }

    #[test]
    fn cnda_fiche_only_uses_abstract_as_full_text() {
        // Aucun PDF → full_text = clean_texte(abstract) ; date de lecture portée
        // par la fiche (résolue côté ingest). Sections vides, ECLI tout de même
        // fabriqué depuis l'année de lecture.
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
        // tolère le texte autour (la regex isole `jour mois année`).
        assert_eq!(
            cnda_lecture_to_iso("Lu en audience publique le 3 avril 2025.").as_deref(),
            Some("2025-04-03")
        );
        // date invalide / illisible → None (pas de date indexée fabriquée).
        assert_eq!(cnda_lecture_to_iso("seulement 2025").as_deref(), None);
        assert_eq!(cnda_lecture_to_iso("32 mai 2026").as_deref(), None);
    }

    #[test]
    fn cnda_errors_on_empty_numero_missing_sections_or_lecture() {
        // Numéro vide → erreur franche.
        assert!(parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "  ").is_err());
        // PDF sans aucun des 8 marqueurs → erreur franche (page vidée).
        assert!(parse_cnda("texte quelconque sans marqueur", &cnda_fiche(), "1").is_err());
        // PDF structuré sans date de lecture ET fiche sans slug daté → erreur
        // franche (ni texte OCR ni slug ne donnent de date).
        let no_lecture = "Vu la procédure suivante : x.\nConsidérant ce qui suit : y.\nDECIDE :\nArticle 1er : rejet.";
        let fiche_sans_date = json!({
            "editorial_abstract": "abstract",
            "fiche_url": "https://www.cnda.fr/decisions-de-justice/jurisprudence/fiche-sans-date",
            "pdf_url": "https://www.cnda.fr/Media/mediatheque-cnda/documents/sans-date-n-26006334-c",
        });
        assert!(parse_cnda(no_lecture, &fiche_sans_date, "1").is_err());
        // …mais le même PDF AVEC un slug daté (`cnda-12-mai-2026-…`) réussit : la
        // date de lecture est récupérée du slug, pas seulement du texte OCR.
        assert!(parse_cnda(no_lecture, &cnda_fiche(), "26006334").is_ok());
    }

    #[test]
    fn cnda_lecture_marker_variants_by_era() {
        // Le marqueur de prononcé varie selon l'époque/OCR : on capte la date dans
        // chaque forme (groupe 1 = date FR), sans faux positif.
        let cases = [
            (
                "Lu en audience publique le 8 septembre 2022.",
                "8 septembre 2022",
            ),
            ("Lu en séance publique le 18 avril 2005", "18 avril 2005"), // CRR
            ("Lecture 8 septembre 2022", "8 septembre 2022"),            // sans « du »
            ("Lecture du 12 mai 2026", "12 mai 2026"),
        ];
        for (text, want) in cases {
            let c = CNDA_LECTURE_RE
                .captures(text)
                .unwrap_or_else(|| panic!("{text:?}"));
            assert_eq!(&c[1], want, "{text:?}");
        }
        // Faux positif écarté : « Lecture du jugement … » (pas de date) ne matche pas.
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
        // Slug descriptif sans date → None (le numéro seul ne forme pas une date).
        assert_eq!(
            cnda_date_from_slug("la-cnda-evalue-la-credibilite-n-25013796"),
            None
        );
    }

    /// Contrat anti-re-embed (ÉTAPE 2, ADR 0096) : les trois champs qui
    /// composent le chunk (`texte_integral_clean`, `metadata_header`,
    /// `visa_trim`) sont posés tels que le chunk = `texte_integral_clean` seul.
    /// `metadata_header`/`visa_trim` sont vides ⇒ aucune métadonnée (date, ECLI,
    /// classement…) n'entre dans le chunk. Vaut PDF **et** fiche-only.
    #[test]
    fn cnda_metadata_never_enters_chunk() {
        // Décision avec PDF.
        let out = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334").expect("parse PDF");
        assert_eq!(out.decision.metadata_header, "");
        assert_eq!(out.decision.visa_trim, "");
        assert_eq!(out.decision.texte_integral_clean, cnda_pdf_text());
        // La date de lecture est portée hors chunk (colonne ISO + source_fields FR).
        assert_eq!(out.decision.date_lecture.as_deref(), Some("2026-05-12"));
        assert!(!out.decision.texte_integral_clean.is_empty());

        // Fiche-only.
        let fiche = json!({
            "editorial_abstract": "Analyse éditoriale de la décision.",
            "fiche_url": "https://www.cnda.fr/fiche-sans-pdf",
            "lecture_date": "5 avril 2025",
        });
        let out = parse_cnda("", &fiche, "25001234").expect("parse fiche-only");
        assert_eq!(out.decision.metadata_header, "");
        assert_eq!(out.decision.visa_trim, "");
        assert_eq!(out.decision.date_lecture.as_deref(), Some("2025-04-05"));
    }

    /// Stabilité de reconstruction (ÉTAPE 4, ADR 0096) : les trois champs qui
    /// déterminent le chunk sont une fonction **déterministe** de l'entrée —
    /// re-parser le même `(pdf_text recollé, fiche, numero)` rend des champs
    /// identiques. Garantit qu'un re-extract/re-chunk futur ne bouge pas les
    /// chunks (donc pas de re-embed). La parité « re-embed depuis source_fields »
    /// via `Decision::from_source_fields` (branche CNDA) est éprouvée par
    /// `cnda_round_trips_via_source_fields` (PDF + fiche-only) + l'oracle
    /// `bench extract-fields-parity`.
    #[test]
    fn cnda_chunk_fields_are_deterministic() {
        let run = || {
            let d = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334")
                .expect("parse")
                .decision;
            (d.texte_integral_clean, d.metadata_header, d.visa_trim)
        };
        assert_eq!(run(), run());
    }

    // ── Round-trip `from_source_fields` ⟷ parse direct (ADR 0085/0096, #37) ──
    //
    // Spec gate : reconstruire une `Decision` depuis `(full_text, source_fields)`
    // reproduit EXACTEMENT le parse direct. Parité structurelle (clean=raw=texte,
    // header/visa vides) — le chunk ne dépend que de `full_text`. Vaut PDF (texte
    // intégral) ET fiche-only (`full_text` = `clean_texte(abstract)` recalculé).

    #[test]
    fn cnda_round_trips_via_source_fields() {
        // Décision avec PDF : `full_text` = pdf_text recollé, sections détectées.
        let orig = parse_cnda(&cnda_pdf_text(), &cnda_fiche(), "26006334").expect("parse PDF");
        let rebuilt = Decision::from_source_fields(
            &orig.decision.texte_integral_clean,
            &orig.source_fields,
            &orig.decision.source_uid,
        );
        assert_eq!(orig.decision, rebuilt);

        // Fiche-only : pas de PDF, `full_text` = `clean_texte(abstract)`.
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
        // Exclusion / révision = signal ambigu → None (pas de variante neuve).
        assert_eq!(mk("L'intéressé est exclu du statut de réfugié"), None);
        assert_eq!(cnda_solution_uid(None), None);
    }
}
