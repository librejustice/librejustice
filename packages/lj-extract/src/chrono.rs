//! Liens de chronologie entre décisions (ADR 0161) : depuis une décision
//! d'appel / de cassation / de renvoi, la référence canonique de la décision
//! ATTAQUÉE. Deux sources : la métadonnée Judilibre `contested`
//! ([`Decision::attacked`]) et le texte (« Par un jugement n° X du D, le
//! tribunal administratif de V… », « Décision déférée à la Cour : … »).
//!
//! La cible sort en **clé pendante** dans la grammaire `canonical_ref` exacte
//! de [`crate::identity`] (mêmes formes par juridiction, même normalisation) :
//! elle se résout côté base dès qu'une décision unique et active la porte.

use std::collections::HashMap;
use std::sync::LazyLock;

use lj_core::decision::Decision;
use regex::Regex;

use crate::extract::common::parse_french_date;
use crate::identity::{
    looks_like_caa_rg, looks_like_pourvoi, normalize_component, normalize_pourvoi,
};

/// Nature du lien, du point de vue de la décision SOURCE (celle qui attaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    AppelDe,
    PourvoiContre,
    RenvoiApresCassation,
}

impl LinkType {
    /// Valeur `decision_links.link_type` (CHECK de la migration 0110).
    pub fn as_str(self) -> &'static str {
        match self {
            LinkType::AppelDe => "APPEL_DE",
            LinkType::PourvoiContre => "POURVOI_CONTRE",
            LinkType::RenvoiApresCassation => "RENVOI_APRES_CASSATION",
        }
    }
}

/// Lien extrait : type + `canonical_ref` de la décision attaquée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorRef {
    pub link_type: LinkType,
    pub target_ref: String,
}

/// Mapping (type de juridiction, ville pliée) → code de localisation
/// Judilibre, dérivé du référentiel `jurisdiction` en base au début du run
/// (comme le `LinkSnapshot` citations). Les clés `canonical_ref` CA/TJ/TCOM
/// exigent ce code ; une ville ambiguë (deux codes pour le même couple) ne
/// mappe pas (règle #12 : pas de clé bancale).
pub struct ChronoSnapshot {
    by_type_city: HashMap<(String, String), Option<String>>,
}

impl ChronoSnapshot {
    /// `rows` = lignes `(code, jurisdiction_type, city)` du référentiel
    /// `jurisdiction`. Le code TCOM référentiel est préfixé (`tcom0603`) alors
    /// que la localisation Judilibre est nue (`0603`) : on déprefixe. TA/CAA
    /// servent la chaîne du fond administratif (ADR 0165) ; CPH est vide
    /// aujourd'hui et se remplit à l'ingest des premières décisions
    /// prud'homales (référentiel ouvert, nourri par la donnée).
    pub fn new(rows: impl IntoIterator<Item = (String, String, String)>) -> Self {
        let mut by_type_city: HashMap<(String, String), Option<String>> = HashMap::new();
        for (code, jt, city) in rows {
            if !matches!(jt.as_str(), "CA" | "TJ" | "TCOM" | "TA" | "CAA" | "CPH") {
                continue;
            }
            let location = match jt.as_str() {
                "TCOM" => code.strip_prefix("tcom").unwrap_or(&code).to_string(),
                _ => code,
            };
            for city_key in city_keys(&normalize_component(&city)) {
                let key = (jt.clone(), city_key);
                match by_type_city.get(&key) {
                    Some(Some(prev)) if *prev != location => {
                        by_type_city.insert(key, None);
                    }
                    Some(_) => {}
                    None => {
                        by_type_city.insert(key, Some(location.clone()));
                    }
                }
            }
        }
        Self { by_type_city }
    }

    pub fn empty() -> Self {
        Self {
            by_type_city: HashMap::new(),
        }
    }

    /// Consommé aussi par le lexer de citations RG (`compiled::lex_case`,
    /// ADR 0165) — même mapping ville → code que les liens de chronologie.
    pub(crate) fn location(&self, jt: &str, city_folded: &str) -> Option<&str> {
        self.by_type_city
            .get(&(jt.to_string(), city_folded.to_string()))?
            .as_deref()
    }
}

/// Liens de chronologie d'une décision — vide quand rien d'exploitable
/// (première instance, discriminants manquants, juridiction cible hors
/// nomenclature type CPH).
pub fn prior_decision_refs(d: &Decision, snap: &ChronoSnapshot) -> Vec<PriorRef> {
    match d.jurisdiction_type.as_deref() {
        Some("CC") | Some("CA") => from_attacked_meta(d, snap)
            .or_else(|| from_deferee_text(d, snap))
            .into_iter()
            .collect(),
        Some("CAA") | Some("CE") => from_admin_text(d).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Métadonnée Judilibre `contested` → lien. CC = pourvoi ; CA = appel, sauf
/// cible Cour de cassation = arrêt sur renvoi.
fn from_attacked_meta(d: &Decision, snap: &ChronoSnapshot) -> Option<PriorRef> {
    let a = d.attacked.as_ref()?;
    let label = normalize_component(a.jurisdiction.as_deref()?);
    let number = a.number.as_deref()?;
    let date = a.date.as_deref()?.trim();
    if date.len() != 10 {
        return None;
    }
    let source_jt = d.jurisdiction_type.as_deref()?;

    if label == "cour de cassation" || label.starts_with("cour de cassation ") {
        // Arrêt attaqué rendu par la Cassation : c'est un renvoi.
        let pourvoi = normalize_pourvoi(number);
        if !looks_like_pourvoi(&pourvoi) {
            return None;
        }
        return Some(PriorRef {
            link_type: LinkType::RenvoiApresCassation,
            target_ref: format!("cc|{pourvoi}|{date}"),
        });
    }

    let (target_jt, city) = jurisdiction_from_label(&label)?;
    let location = normalize_component(snap.location(target_jt, &city)?);
    let rg = normalize_component(number);
    if rg.is_empty() || location.is_empty() {
        return None;
    }
    let link_type = match source_jt {
        "CC" => LinkType::PourvoiContre,
        _ => LinkType::AppelDe,
    };
    Some(PriorRef {
        link_type,
        target_ref: format!("{}|{location}|{rg}|{date}", target_jt.to_lowercase()),
    })
}

/// Clés de ville d'une ligne référentielle : la forme pliée, plus la forme
/// sans article de tête (« le havre » → aussi « havre ») — les citations
/// contractent l'article (« du Havre », « des Sables-d'Olonne ») quand la
/// ville référentielle le porte en toutes lettres.
fn city_keys(folded: &str) -> Vec<String> {
    let mut keys = vec![folded.to_string()];
    for article in ["la ", "le ", "les "] {
        if let Some(rest) = folded.strip_prefix(article) {
            if !rest.is_empty() {
                keys.push(rest.to_string());
            }
            break;
        }
    }
    keys
}

/// Libellé plié (`cour d appel d aix en provence`) → (type, ville pliée).
/// Le juge de l'exécution et l'ex-TGI/TI sont des formations du TJ de la même
/// ville. CPH, tribunaux paritaires, cours d'assises : hors nomenclature.
fn jurisdiction_from_label(folded: &str) -> Option<(&'static str, String)> {
    const FORMS: &[(&str, &str)] = &[
        ("cour d appel ", "CA"),
        ("tribunal judiciaire ", "TJ"),
        ("tribunal de grande instance ", "TJ"),
        ("tribunal d instance ", "TJ"),
        ("juge de l execution ", "TJ"),
        ("tribunal de commerce ", "TCOM"),
        ("tribunal mixte de commerce ", "TCOM"),
    ];
    for (prefix, jt) in FORMS {
        if let Some(rest) = folded.strip_prefix(prefix) {
            let city = rest
                .strip_prefix("de la ")
                .or_else(|| rest.strip_prefix("de "))
                .or_else(|| rest.strip_prefix("d "))
                .or_else(|| rest.strip_prefix("du "))
                .or_else(|| rest.strip_prefix("des "))
                .unwrap_or(rest)
                .trim();
            if city.is_empty() {
                return None;
            }
            return Some((jt, city.to_string()));
        }
    }
    None
}

/// Spans inline de la décision ATTAQUÉE (métadonnée Judilibre `contested`) :
/// les surfaces standard du corps qui la répètent — « l'arrêt attaqué (Paris,
/// 22 août 2024) », « l'arrêt rendu le 22 août 2024 par la cour d'appel de
/// Paris » — deviennent des citations de jurisprudence (`case_citation`)
/// portant le MÊME `target_ref` que le lien de chronologie, résolues côté
/// base par le pont depuis `decision_links` (même décision, même clé). Ces
/// mentions n'ont pas de docket en texte : seule la métadonnée identifie la
/// cible, et ville ET date doivent la matcher — zéro mislink. Offsets en
/// CHARS (grain `case_citation`).
pub fn attacked_text_spans(d: &Decision, snap: &ChronoSnapshot) -> Vec<(usize, usize, String)> {
    let Some(r) = from_attacked_meta(d, snap) else {
        return Vec::new();
    };
    let a = d.attacked.as_ref().unwrap();
    let label = normalize_component(a.jurisdiction.as_deref().unwrap_or(""));
    // Cible Cour de cassation (renvoi) : pas de ville, formes inline autres.
    let Some((_, city)) = jurisdiction_from_label(&label) else {
        return Vec::new();
    };
    let date = a.date.as_deref().unwrap_or("").trim();
    let text = &d.texte_integral_clean;
    let mut spans: Vec<(usize, usize)> = Vec::new();

    for cap in RE_ATTACKED_PAREN.captures_iter(text) {
        let inner = cap.get(1).unwrap();
        let Some(c) = RE_CITY_DATE.captures(inner.as_str()) else {
            continue;
        };
        if parse_french_date(&c[2], &c[3], &c[4]).as_deref() != Some(date)
            || normalize_component(&c[1]) != city
        {
            continue;
        }
        spans.push((inner.start(), inner.end()));
    }
    for cap in RE_ATTACKED_RENDU.captures_iter(text) {
        if parse_french_date(&cap[1], &cap[2], &cap[3]).as_deref() != Some(date) {
            continue;
        }
        // Le captage ville peut sur-courir de purs mots (« Douai a violé ») :
        // rogné mot à mot par la droite jusqu'à égaler la ville attendue.
        let cm = cap.get(4).unwrap();
        let raw = cm.as_str();
        let mut end = raw.len();
        loop {
            if normalize_component(&raw[..end]) == city {
                spans.push((
                    cap.get(0).unwrap().start(),
                    cm.start() + raw[..end].trim_end().len(),
                ));
                break;
            }
            match raw[..end].trim_end().rfind(char::is_whitespace) {
                Some(i) => end = i,
                None => break,
            }
        }
    }

    spans.sort_unstable();
    spans.dedup();
    spans
        .into_iter()
        .map(|(bs, be)| {
            (
                text[..bs].chars().count(),
                text[..be].chars().count(),
                r.target_ref.clone(),
            )
        })
        .collect()
}

/// « l'arrêt attaqué (Paris, 22 août 2024) » — la parenthèse standard des
/// moyens de cassation. Groupe 1 = contenu, validé par [`RE_CITY_DATE`].
static RE_ATTACKED_PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:arr[êe]ts?|jugements?|ordonnances?|d[ée]cisions?)\s+attaqu[ée]e?s?\s*\(\s*([^)]{2,80}?)\s*\)",
    )
    .unwrap()
});
/// Contenu de la parenthèse attaquée : « <Ville>, <date> ».
static RE_CITY_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^([\p{L}'’ \-]{2,40}?)\s*,\s*(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})$").unwrap()
});
/// « l'arrêt rendu le 22 août 2024 par la cour d'appel de Paris » — l'en-tête
/// de pourvoi. Le span court du nom de décision à la ville (rognée en aval).
static RE_ATTACKED_RENDU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:arr[êe]ts?|jugements?|ordonnances?)\s+rendue?s?\s+le\s+(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})\s+par\s+l[ea]\s+(?:cour\s+d[’']appel|tribunal\s+judiciaire|tribunal\s+de\s+grande\s+instance|tribunal\s+d[’']instance|juge\s+de\s+l[’']ex[ée]cution|tribunal\s+(?:mixte\s+)?de\s+commerce)\s+(?:de\s+la\s+|de\s+|d[’']|du\s+)((?:[\p{L}'’\-]+)(?: [\p{L}'’\-]+){0,3})",
    )
    .unwrap()
});

/// « Par un jugement n° 2107366 du 13 septembre 2021, la magistrate désignée
/// du tribunal administratif de Marseille a rejeté sa demande. » — zone
/// « Procédure contentieuse antérieure » des CAA/CE. Groupes : n°, jour, mois,
/// année, libellé juridiction verbatim (article inclus, pour la forme nom de
/// la clé TA).
static RE_PRIOR_ADMIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bpar\s+(?:un|une|deux|trois|quatre)?\s*(?:jugements?|ordonnances?|arr[êe]ts?)\s+n[°ºo]?\s*s?\s*([0-9][0-9A-Za-z]{2,14})(?:\s*(?:,|et)\s*n[°ºo]?\s*s?\s*[0-9][0-9A-Za-z]{2,14})*\s+(?:du|en\s+date\s+du)\s+(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})\s*,\s*l[ea]\s+(?:[\p{L}'’ \-]{0,60}?\b(?:du|de\s+la)\s+)?((?:tribunal\s+administratif|cour\s+administrative\s+d[’']appel)\s+(?:de\s+la\s+|de\s+|d[’'])[\p{L}][\p{L}'’ \-]{1,40}?)\s+(?:a\b|ont\b|n[’']a|s[’']est|statuant)",
    )
    .unwrap()
});

/// Réfs admin : toutes les décisions antérieures citées en en-tête ; la
/// décision ATTAQUÉE est la plus récente (un pourvoi CE cite le jugement TA
/// puis l'arrêt CAA — c'est l'arrêt qu'il attaque).
fn from_admin_text(d: &Decision) -> Option<PriorRef> {
    let text = &d.texte_integral_clean;
    let header_end = text.char_indices().nth(8000).map_or(text.len(), |(i, _)| i);
    let header = &text[..header_end];

    let mut best: Option<(String, PriorRef)> = None; // (date ISO, lien) — max date
    for cap in RE_PRIOR_ADMIN.captures_iter(header) {
        let date = match parse_french_date(&cap[2], &cap[3], &cap[4]) {
            Some(iso) => iso,
            None => continue,
        };
        let rg = normalize_component(&cap[1]);
        let jur = normalize_component(&cap[5]);
        if rg.is_empty() || jur.is_empty() {
            continue;
        }
        // Grammaire identity : CAA par RG auto-porteur, TA/repli par le nom.
        let target_ref = if jur.starts_with("cour administrative d appel") && looks_like_caa_rg(&rg)
        {
            format!("caa|{rg}|{date}")
        } else {
            format!("{jur}|{rg}|{date}")
        };
        let link_type = match d.jurisdiction_type.as_deref() {
            Some("CE") if header_has_pourvoi(header) => LinkType::PourvoiContre,
            _ => LinkType::AppelDe,
        };
        let candidate = (
            date.clone(),
            PriorRef {
                link_type,
                target_ref,
            },
        );
        if best.as_ref().is_none_or(|(b, _)| *b <= date) {
            best = Some(candidate);
        }
    }
    best.map(|(_, r)| r)
}

fn header_has_pourvoi(header: &str) -> bool {
    crate::compiled::fold_stable(header).contains("pourvoi")
}

/// « Décision déférée à la Cour : jugement rendu le 07 Septembre 2017 par le
/// Tribunal judiciaire de PARIS - RG n° 15/02585 » et les variantes « jugement
/// du <date> - Juge de l'exécution de SAINT-ETIENNE … RG : 18/00064 » et
/// « jugement du <date> rendu par le tribunal judiciaire de PARIS - RG
/// n° 21/00000 » — en-têtes CA Judilibre quand `contested` manque
/// (97 % des CA).
static RE_DEFEREE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)d[ée]cision\s+d[ée]f[ée]r[ée]e[^:]{0,60}:\s*(?:jugements?|ordonnances?|arr[êe]ts?)[^:;]{0,80}?(?:rendue?\s+le|du)\s+(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})\s*(?:(?:rendue?\s+)?par\s+l[ea]\s*|[-–]\s*)?((?:tribunal\s+judiciaire|tribunal\s+de\s+grande\s+instance|tribunal\s+d[’']instance|juge\s+de\s+l[’']ex[ée]cution|tribunal\s+de\s+commerce|tribunal\s+mixte\s+de\s+commerce)\s+(?:de\s+la\s+|de\s+|d[’']|du\s+)?[\p{L}][\p{L}'’ \-]{1,40}?)\s*[-–,(].{0,120}?RG\s*(?:n[°ºo]\s*)?:?\s*([0-9][0-9A-Za-z/.\-]{3,15})",
    )
    .unwrap()
});

fn from_deferee_text(d: &Decision, snap: &ChronoSnapshot) -> Option<PriorRef> {
    if d.jurisdiction_type.as_deref() != Some("CA") {
        return None;
    }
    let text = &d.texte_integral_clean;
    let header_end = text.char_indices().nth(4000).map_or(text.len(), |(i, _)| i);
    let cap = RE_DEFEREE.captures(&text[..header_end])?;
    let date = parse_french_date(&cap[1], &cap[2], &cap[3])?;
    let (target_jt, city) = jurisdiction_from_label(&normalize_component(&cap[4]))?;
    let location = normalize_component(snap.location(target_jt, &city)?);
    let rg = normalize_component(cap[5].trim());
    if rg.is_empty() {
        return None;
    }
    Some(PriorRef {
        link_type: LinkType::AppelDe,
        target_ref: format!("{}|{location}|{rg}|{date}", target_jt.to_lowercase()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> ChronoSnapshot {
        ChronoSnapshot::new(vec![
            ("ca_nancy".into(), "CA".into(), "Nancy".into()),
            (
                "ca_aix_provence".into(),
                "CA".into(),
                "Aix-en-Provence".into(),
            ),
            ("tj75056".into(), "TJ".into(), "Paris".into()),
            ("tj42218".into(), "TJ".into(), "Saint-Étienne".into()),
            ("tcom7501".into(), "TCOM".into(), "Paris".into()),
        ])
    }

    fn decision(jt: &str, text: &str) -> Decision {
        Decision {
            source_uid: "test".to_string(),
            member_name: String::new(),
            ecli: None,
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name: None,
            jurisdiction_type: Some(jt.to_string()),
            jurisdiction_location: None,
            numero_dossier: None,
            numero_dossiers: None,
            numero_role: None,
            date_lecture: None,
            date_audience: None,
            date_mise_jour: None,
            formation: None,
            type_decision: None,
            type_recours: None,
            solution: None,
            publication_codes: vec![],
            avocat_requerant: None,
            texte_integral_raw: text.to_string(),
            texte_integral_clean: text.to_string(),
            sections: vec![],
            metadata_header: String::new(),
            visa_trim: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: vec![],
        }
    }

    #[test]
    fn cc_contested_ca_gives_pourvoi_contre() {
        let mut d = decision("CC", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Cour d'appel de Nancy".into()),
            number: Some("19/00207".into()),
            date: Some("2020-12-08".into()),
        });
        let refs = prior_decision_refs(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::PourvoiContre);
        assert_eq!(refs[0].target_ref, "ca|ca nancy|19 00207|2020-12-08");
    }

    /// Villes à article : le référentiel porte « Le Havre » / « La Rochelle »
    /// quand la citation contracte (« du Havre ») ou strippe (« de La
    /// Rochelle » → « rochelle ») l'article — les deux formes doivent mapper.
    #[test]
    fn article_city_resolves_contracted_citation() {
        let snap = ChronoSnapshot::new(vec![
            ("tj76351".into(), "TJ".into(), "Le Havre".into()),
            ("tj17300".into(), "TJ".into(), "La Rochelle".into()),
        ]);
        let mut d = decision("CC", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Tribunal judiciaire du Havre".into()),
            number: Some("21/00123".into()),
            date: Some("2023-01-10".into()),
        });
        let refs = prior_decision_refs(&d, &snap);
        assert_eq!(refs[0].target_ref, "tj|tj76351|21 00123|2023-01-10");

        let mut d = decision("CC", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Tribunal judiciaire de La Rochelle".into()),
            number: Some("22/00456".into()),
            date: Some("2023-06-15".into()),
        });
        let refs = prior_decision_refs(&d, &snap);
        assert_eq!(refs[0].target_ref, "tj|tj17300|22 00456|2023-06-15");
    }

    #[test]
    fn ca_contested_cc_is_renvoi() {
        let mut d = decision("CA", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Cour de cassation".into()),
            number: Some("20-22.085".into()),
            date: Some("2022-03-24".into()),
        });
        let refs = prior_decision_refs(&d, &snap());
        assert_eq!(refs[0].link_type, LinkType::RenvoiApresCassation);
        assert_eq!(refs[0].target_ref, "cc|20-22.085|2022-03-24");
    }

    #[test]
    fn caa_header_links_ta_judgment() {
        let d = decision(
            "CAA",
            "Vu la procédure suivante : Procédure contentieuse antérieure : M. B A a demandé \
             au tribunal administratif de Marseille d'annuler l'arrêté du 19 juillet 2021. \
             Par un jugement n° 2107366 du 13 septembre 2021, la magistrate désignée du \
             tribunal administratif de Marseille a rejeté sa demande. Procédure devant la \
             Cour : Par une requête enregistrée le 13 octobre 2021, M. A demande à la Cour \
             d'annuler ce jugement.",
        );
        let refs = prior_decision_refs(&d, &ChronoSnapshot::empty());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::AppelDe);
        assert_eq!(
            refs[0].target_ref,
            "tribunal administratif de marseille|2107366|2021-09-13"
        );
    }

    #[test]
    fn ce_pourvoi_targets_most_recent_caa_arret() {
        let d = decision(
            "CE",
            "Vu la procédure suivante : La société Fryan a demandé au tribunal administratif \
             de Lyon d'annuler l'arrêté du 9 janvier 2017. Par un jugement n° 1721444 du \
             14 novembre 2019, le tribunal administratif de Lyon a fait droit à cette \
             demande. Par un arrêt n° 20LY00096 du 14 décembre 2021, la cour administrative \
             d'appel de Lyon a, sur l'appel de la commune, annulé ce jugement. Par un \
             pourvoi, enregistré le 14 février 2022, la société Fryan demande au Conseil \
             d'Etat d'annuler cet arrêt.",
        );
        let refs = prior_decision_refs(&d, &ChronoSnapshot::empty());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::PourvoiContre);
        assert_eq!(refs[0].target_ref, "caa|20ly00096|2021-12-14");
    }

    #[test]
    fn caa_multi_judgments_takes_first_number() {
        let d = decision(
            "CAA",
            "Par deux jugements n° 2202850 et n° 2202851 du 16 mars 2023, le tribunal \
             administratif de Poitiers a rejeté leurs demandes. Par une requête, \
             enregistrée le 11 avril 2023, M. C demande à la cour d'annuler ce jugement.",
        );
        let refs = prior_decision_refs(&d, &ChronoSnapshot::empty());
        assert_eq!(
            refs[0].target_ref,
            "tribunal administratif de poitiers|2202850|2023-03-16"
        );
    }

    #[test]
    fn ca_deferee_jex_maps_to_tj() {
        let d = decision(
            "CA",
            "COUR D'APPEL DE PARIS Pôle 4 - Chambre 8 ARRÊT DU 27 NOVEMBRE 2014 Numéro \
             d'inscription au répertoire général : 13/24338 Décision déférée à la Cour : \
             Jugement du 19 Novembre 2013 - Juge de l'exécution de PARIS - RG n° 12/81319 \
             APPELANTE Madame X",
        );
        let refs = prior_decision_refs(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::AppelDe);
        assert_eq!(refs[0].target_ref, "tj|tj75056|12 81319|2013-11-19");
    }

    #[test]
    fn ca_deferee_du_date_rendu_par_le_tj() {
        let d = decision(
            "CA",
            "COUR D'APPEL DE PARIS Pôle 3 - Chambre 5 ARRET DU 23 JUIN 2026 Numéro \
             d'inscription au répertoire général : N° RG 25/00001 - N° Portalis X \
             Décision déférée à la Cour : Jugement du 23 janvier 2025 rendu par le \
             tribunal judiciaire de PARIS - RG n° 21/00002 APPELANT LE MINISTERE PUBLIC",
        );
        let refs = prior_decision_refs(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::AppelDe);
        assert_eq!(refs[0].target_ref, "tj|tj75056|21 00002|2025-01-23");
    }

    #[test]
    fn attacked_text_spans_require_city_and_date_match() {
        let text = "ont formé le pourvoi contre l'arrêt rendu le 8 décembre 2020 par la \
                    cour d'appel de Nancy (pôle 4, chambre 10), dans le litige. Sur le \
                    moyen : l'arrêt attaqué (Nancy, 8 décembre 2020) a violé la loi. \
                    L'arrêt attaqué (Paris, 3 mai 2019) relève d'un autre pourvoi.";
        let mut d = decision("CC", text);
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Cour d'appel de Nancy".into()),
            number: Some("19/00207".into()),
            date: Some("2020-12-08".into()),
        });
        let spans = attacked_text_spans(&d, &snap());
        let surfaces: Vec<String> = spans
            .iter()
            .map(|(s, e, r)| {
                assert_eq!(r, "ca|ca nancy|19 00207|2020-12-08");
                text.chars().skip(*s).take(e - s).collect()
            })
            .collect();
        assert_eq!(
            surfaces,
            vec![
                "arrêt rendu le 8 décembre 2020 par la cour d'appel de Nancy".to_string(),
                "Nancy, 8 décembre 2020".to_string(),
            ]
        );
    }

    #[test]
    fn unsupported_target_yields_nothing() {
        let mut d = decision("CA", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Conseil de Prud'hommes de PARIS".into()),
            number: Some("F15/02585".into()),
            date: Some("2017-09-07".into()),
        });
        assert!(prior_decision_refs(&d, &snap()).is_empty());
    }

    #[test]
    fn ambiguous_city_does_not_map() {
        let s = ChronoSnapshot::new(vec![
            ("tj00001".into(), "TJ".into(), "Doublon".into()),
            ("tj00002".into(), "TJ".into(), "Doublon".into()),
        ]);
        assert!(s.location("TJ", "doublon").is_none());
    }
}
