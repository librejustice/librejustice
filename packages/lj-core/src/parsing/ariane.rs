//! Parse pur des analyses AJCE ArianeWeb (ADR 0204 ; savoir source ADR 0095).
//!
//! L'HTML plein (déjà décodé latin-1 + `0x19` au bord `lj-sources`) alterne des
//! segments séparés par `<hr>` : en-tête (n° dossier, niveau Lebon, date de
//! lecture), puis paires bloc-rubrique (code PCJA + libellés + titre analytique)
//! / bloc-sommaire (paragraphe doctrinal). Deux époques cohabitent :
//! - 1976 : libellé de rubrique mono-ligne MAJUSCULES (`ALGERIE - CONTENTIEUX`),
//!   sommaire **répété à l'identique** sous chaque rubrique partagée ;
//! - récent : libellé multi-lignes à tiret final (`Procédure-⏎ Jugements-…`),
//!   renvois `(1) Cf. …` en queue du dernier sommaire.
//!
//! Sortie : la structure [`AjceAnalysis`] et la composition du bundle
//! `source_fields` ADR 0204 (`commentaires[]`).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

/// Une analyse AJCE parsée (un document du fond = une entrée `kind:"analyse"`).
#[derive(Debug, PartialEq, Eq)]
pub struct AjceAnalysis {
    /// N° de dossier CE de l'en-tête (`N° 438885`) — recoupement avec le hit.
    pub dossier: Option<String>,
    /// Niveau de publication en toutes lettres (`Mentionné aux tables du
    /// recueil Lebon`).
    pub niveau: Option<String>,
    /// Rubriques du plan de classement, dans l'ordre du document.
    pub rubriques: Vec<AjceRubrique>,
    /// Sommaires doctrinaux **dédupliqués**, ordre de première apparition.
    pub sommaires: Vec<String>,
    /// Renvois de fin (`(1) Cf. CE, …`), dédupliqués.
    pub renvois: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct AjceRubrique {
    /// Code PCJA (`54-06-05-11`).
    pub code: String,
    /// Libellé hiérarchique (`Procédure - Jugements - Frais et dépens - …`).
    pub label: String,
    /// Titre analytique de la rubrique.
    pub titre: String,
    /// Index dans [`AjceAnalysis::sommaires`] du sommaire attaché.
    pub sommaire_idx: Option<usize>,
}

static HR_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<hr\s*/?>").unwrap());
static BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static RUBRIQUE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d{2,3}(?:-\d{2,3})*)\s*:\s*(.*)$").unwrap());
static DOSSIER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"N°\s*([0-9][0-9,\s]*)").unwrap());
static RENVOI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\(\d+\)\s").unwrap());

/// Parse l'HTML plein (décodé) d'une analyse AJCE. Tolérant : un document sans
/// rubrique reconnaissable rend `rubriques`/`sommaires` vides, jamais d'erreur
/// (le pipeline le compte et le skippe).
pub fn parse_ajce_html(html: &str) -> AjceAnalysis {
    let segments: Vec<Vec<Vec<String>>> = HR_SPLIT
        .split(html)
        .map(segment_paragraphs)
        .filter(|paras| !paras.is_empty())
        .collect();

    let mut out = AjceAnalysis {
        dossier: None,
        niveau: None,
        rubriques: Vec::new(),
        sommaires: Vec::new(),
        renvois: Vec::new(),
    };

    for (i, paras) in segments.iter().enumerate() {
        if i == 0 {
            parse_header(paras, &mut out);
            continue;
        }
        if let Some(c) = RUBRIQUE_CODE.captures(&paras[0][0]) {
            // Paragraphe 0 = ligne code (+ éventuel début de libellé) puis
            // lignes de libellé (multi-lignes à tiret final en récent,
            // mono-ligne MAJUSCULES en 1976). Paragraphes suivants = titre.
            let code = c[1].to_string();
            let mut label_parts: Vec<String> = Vec::new();
            let rest = c[2].trim();
            if !rest.is_empty() {
                label_parts.push(rest.trim_end_matches('-').trim().to_string());
            }
            for line in &paras[0][1..] {
                label_parts.push(line.trim_end_matches('-').trim().to_string());
            }
            let titre = paras[1..]
                .iter()
                .map(|p| p.join(" "))
                .collect::<Vec<_>>()
                .join(" ");
            out.rubriques.push(AjceRubrique {
                code,
                label: label_parts.join(" - "),
                titre,
                sommaire_idx: None,
            });
        } else {
            // Bloc sommaire : les paragraphes de renvois `(1) Cf. …` (en queue
            // du dernier sommaire) sont extraits ligne à ligne, le reste forme
            // le paragraphe doctrinal (dédupliqué byte-à-byte, audit ADR 0095).
            let mut body_paras: Vec<String> = Vec::new();
            for p in paras {
                if RENVOI.is_match(&p[0]) {
                    for r in p {
                        if !out.renvois.iter().any(|x| x == r) {
                            out.renvois.push(r.clone());
                        }
                    }
                } else {
                    body_paras.push(p.join("\n"));
                }
            }
            let body = body_paras.join("\n");
            if body.is_empty() {
                continue;
            }
            let idx = match out.sommaires.iter().position(|s| *s == body) {
                Some(idx) => idx,
                None => {
                    out.sommaires.push(body);
                    out.sommaires.len() - 1
                }
            };
            if let Some(r) = out
                .rubriques
                .iter_mut()
                .rev()
                .find(|r| r.sommaire_idx.is_none())
            {
                r.sommaire_idx = Some(idx);
            }
        }
    }
    out
}

/// En-tête : `Conseil d'État` / `N° 438885` / niveau Lebon / `Lecture du …`.
fn parse_header(paras: &[Vec<String>], out: &mut AjceAnalysis) {
    for line in paras.iter().flatten() {
        if let Some(c) = DOSSIER.captures(line) {
            let first = c[1].split([',', ' ']).find(|s| !s.is_empty());
            out.dossier = first.map(str::to_string);
        } else if !line.starts_with("Conseil d") && !line.starts_with("Lecture du") {
            out.niveau.get_or_insert_with(|| line.clone());
        }
    }
}

/// Un segment `<hr>` → paragraphes (groupes de lignes séparés par des lignes
/// vides) : `<br>` = saut de ligne, autres balises retirées, blancs normalisés
/// par ligne. Les frontières de paragraphe (`<br><br>`) portent la structure
/// libellé / titre / renvois.
fn segment_paragraphs(segment: &str) -> Vec<Vec<String>> {
    let with_breaks = BR.replace_all(segment, "\n");
    let stripped = TAG.replace_all(&with_breaks, "");
    let mut paras: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for line in stripped.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(line);
        }
    }
    if !cur.is_empty() {
        paras.push(cur);
    }
    paras
}

/// Corps affichable d'une entrée `kind:"analyse"` : pour chaque sommaire unique
/// (ordre du document), les titres analytiques des rubriques qui y renvoient,
/// puis le paragraphe doctrinal. Certains documents (années 60) n'ont **aucun**
/// paragraphe doctrinal, seulement les titres — le corps se replie dessus.
pub fn analyse_body(a: &AjceAnalysis) -> String {
    if a.sommaires.is_empty() {
        let mut titres: Vec<&str> = Vec::new();
        for r in &a.rubriques {
            if !r.titre.is_empty() && !titres.contains(&r.titre.as_str()) {
                titres.push(&r.titre);
            }
        }
        return titres.join("\n");
    }
    let mut groups: Vec<String> = Vec::new();
    for (idx, sommaire) in a.sommaires.iter().enumerate() {
        let mut titres: Vec<&str> = Vec::new();
        for r in &a.rubriques {
            if r.sommaire_idx == Some(idx)
                && !r.titre.is_empty()
                && !titres.contains(&r.titre.as_str())
            {
                titres.push(&r.titre);
            }
        }
        let mut g = titres.join("\n");
        if !g.is_empty() {
            g.push_str("\n\n");
        }
        g.push_str(sommaire);
        groups.push(g);
    }
    groups.join("\n\n")
}

/// Décision `AW_DCE` parsée (backfill ADR 0219) : en-tête + corps texte.
#[derive(Debug)]
pub struct DceParsed {
    /// N°s de dossier de l'en-tête (`N° 412849, 412895` → vec).
    pub dossiers: Vec<String>,
    /// ECLI verbatim de l'en-tête (`ECLI:FR:CESSR:2002:221186.20020517`).
    pub ecli: Option<String>,
    /// Niveau de publication en toutes lettres (ligne « … recueil Lebon »).
    pub niveau: Option<String>,
    /// Texte intégral (balises retirées, `<br>` = saut de ligne).
    pub body: String,
}

/// Parse l'HTML plein (décodé) d'une décision `AW_DCE`. Seul l'en-tête est
/// interprété (dossiers, ECLI, niveau) ; les champs structurés (solution,
/// formation…) restent à l'extracteur routé CE standard.
pub fn parse_dce_html(html: &str) -> DceParsed {
    // `<div>` (centrage des mentions) ne vaut pas saut de ligne après strip :
    // on le convertit pour ne pas coller deux lignes (« 2002REPUBLIQUE… »).
    let html = html
        .replace("<div", "<br><div")
        .replace("</div>", "</div><br>");
    let paras = segment_paragraphs(&html);
    let mut out = DceParsed {
        dossiers: Vec::new(),
        ecli: None,
        niveau: None,
        body: String::new(),
    };
    for line in paras.iter().flatten() {
        if out.dossiers.is_empty() {
            if let Some(c) = DOSSIER.captures(line) {
                out.dossiers = c[1]
                    .split([',', ' '])
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        }
        if out.ecli.is_none() && line.starts_with("ECLI:") {
            out.ecli = Some(line.clone());
        }
        if out.niveau.is_none() && line.contains("recueil Lebon") {
            out.niveau = Some(line.clone());
        }
    }
    out.body = paras
        .iter()
        .map(|p| p.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");
    out
}

/// Une analyse prête pour le bundle (parse + métadonnées du hit xsearch).
pub struct AjceEntry {
    pub body: String,
    /// Codes PCJA du hit (`SourceCsv3` éclaté).
    pub codes_pcja: Vec<String>,
    /// Niveau de publication A/B/C (`SourceStr8`).
    pub niveau: Option<String>,
    /// Rubriques `code : label` (libellés du plan de classement, pour le rendu).
    pub rubriques: Vec<String>,
    pub renvois: Vec<String>,
    /// Date de lecture ISO de la décision parente.
    pub date: Option<String>,
}

/// `source_fields` de la ligne `decision_sources` ArianeWeb (ADR 0204) : une
/// entrée `commentaires[]` par analyse, plus `{"kind":"conclusions"}` si le
/// graphe de fratrie confirme des conclusions. Les clés de rattachement
/// (dossier, date) restent dans le bundle : le lien CRP est composé au rendu
/// depuis ces champs.
pub fn build_ariane_source_fields(
    dossier: &str,
    date_lecture: &str,
    ecli: Option<&str>,
    analyses: &[AjceEntry],
    has_conclusions: bool,
) -> Value {
    let mut commentaires: Vec<Value> = analyses
        .iter()
        .map(|a| {
            json!({
                "kind": "analyse",
                "author": "Conseil d'État (SRD)",
                "date": a.date,
                "body": a.body,
                "meta": {
                    "codes_pcja": a.codes_pcja,
                    "niveau": a.niveau,
                    "rubriques": a.rubriques,
                    "renvois": a.renvois,
                },
            })
        })
        .collect();
    if has_conclusions {
        commentaires.push(json!({ "kind": "conclusions" }));
    }
    json!({
        "dossier": dossier,
        "date_lecture": date_lecture,
        "ecli": ecli,
        "commentaires": commentaires,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Format récent (2022) : libellés multi-lignes à tiret final, deux
    /// rubriques au sommaire distinct, renvois en queue du dernier sommaire.
    const RECENT: &str = "<html>\r\n<body style=\"font-size: 16px;\"><strong>Conseil d'État</strong><br><br><strong>N° 438885<br></strong>Mentionné aux tables du recueil Lebon<br><br><strong>Lecture du lundi 20 juin 2022<br></strong><br><hr><br>54-06-05-11 :\r\n          Procédure-\r\n\t  Jugements-\r\n\t\t  Frais et dépens-<br><br>Frais mis à la charge d'une société - Préjudice indemnisable (1) (2).<br><br><br><hr><br>Premier sommaire doctrinal.\r\nSuite du premier sommaire.<br><br><br><br><hr><br>60-04-02-01 :\r\n          Responsabilité-\r\n\t  Réparation-<br><br>Illégalité de l'autorisation - Exonération partielle.<br><br><br><hr><br>Second sommaire doctrinal.<br><br><br>(1) Cf. CE, 4 novembre 2020, Société financière Mag, n° 428741, T. pp. 992.\r\n(2) Rappr. CE, 15 octobre 2021, n°s 436725 436746, T. pp. 852.</body>\r\n</html>\r\n";

    /// Format 1976 : libellé mono-ligne MAJUSCULES, sommaire répété à
    /// l'identique sous deux rubriques (dédup attendue).
    const ANCIEN: &str = "<html>\r\n<body style=\"font-size: 16px;\"><strong>Conseil d'État</strong><br><br><strong>N° 96402<br></strong>Mentionné aux tables du recueil Lebon<br><br><strong>Lecture du mercredi 3 mars 1976<br></strong><hr><br>05-03 :\r\n        ALGERIE - CONTENTIEUX<br><br>Responsabilité - Absence - Services de police.<br><br><br><hr><br>Sommaire unique répété.<br><br><hr><br>60-01-02-01 :\r\n        RESPONSABILITE DE LA PUISSANCE PUBLIQUE - FONDEMENT<br><br>Absence de responsabilité sans faute - Algérie.<br><br><br><hr><br>Sommaire unique répété.<br><br></body>\r\n</html>\r\n";

    #[test]
    fn parse_format_recent() {
        let a = parse_ajce_html(RECENT);
        assert_eq!(a.dossier.as_deref(), Some("438885"));
        assert_eq!(
            a.niveau.as_deref(),
            Some("Mentionné aux tables du recueil Lebon")
        );
        assert_eq!(a.rubriques.len(), 2);
        assert_eq!(a.rubriques[0].code, "54-06-05-11");
        assert_eq!(
            a.rubriques[0].label,
            "Procédure - Jugements - Frais et dépens"
        );
        assert_eq!(
            a.rubriques[0].titre,
            "Frais mis à la charge d'une société - Préjudice indemnisable (1) (2)."
        );
        assert_eq!(a.rubriques[0].sommaire_idx, Some(0));
        assert_eq!(a.rubriques[1].sommaire_idx, Some(1));
        assert_eq!(a.sommaires.len(), 2);
        assert_eq!(
            a.sommaires[0],
            "Premier sommaire doctrinal.\nSuite du premier sommaire."
        );
        assert_eq!(a.renvois.len(), 2);
        assert!(a.renvois[0].starts_with("(1) Cf. CE, 4 novembre 2020"));
    }

    #[test]
    fn parse_format_1976_dedup_sommaire() {
        let a = parse_ajce_html(ANCIEN);
        assert_eq!(a.dossier.as_deref(), Some("96402"));
        assert_eq!(a.rubriques.len(), 2);
        assert_eq!(a.rubriques[0].label, "ALGERIE - CONTENTIEUX");
        // Sommaire byte-identique sous les deux rubriques → une seule copie,
        // les deux rubriques pointent dessus.
        assert_eq!(a.sommaires, vec!["Sommaire unique répété."]);
        assert_eq!(a.rubriques[0].sommaire_idx, Some(0));
        assert_eq!(a.rubriques[1].sommaire_idx, Some(0));
        assert!(a.renvois.is_empty());
    }

    /// Format « classement seul » (fréquent 1986-1995) : rubriques code +
    /// libellé sans titre analytique ni sommaire — le classement PCJA est la
    /// seule information de la fiche, le corps reste vide.
    const CLASSEMENT_SEUL: &str = "<html> <body style=\"font-size: 16px;\"><strong>Conseil d'État</strong><br><br><strong>N° 99545<br></strong>Non publié au recueil Lebon<br><br><strong>Lecture du vendredi 22 octobre 1993<br></strong><hr><br>14-06-01 : COMMERCE, INDUSTRIE, INTERVENTION ECONOMIQUE DE LA PUISSANCE PUBLIQUE - ORGANISATION PROFESSIONNELLE DES ACTIVITES ECONOMIQUES - CHAMBRES DE COMMERCE ET D'INDUSTRIE<br><br><br><br><br><hr><br><br><br><hr><br>33-02-06-02 : ETABLISSEMENTS PUBLICS - REGIME JURIDIQUE - PERSONNEL - STATUT<br><br><br><br><br><hr><br><br><br></body> </html>";

    /// En-tête réel de la décision AW_DCE n° 62362 (CE 221186, 17 mai 2002) —
    /// vérifié live 2026-07-12.
    const DCE: &str = "<html> <body style=\"font-size: 16px;\"><strong>Conseil d'État</strong><br><br><strong>N° 221186</strong><br><strong>ECLI:FR:CESSR:2002:221186.20020517</strong><br>Mentionné au tables du recueil Lebon<br><span style=\"float: right\"><strong>Section du Contentieux</strong></span><br>M. Stirn, président<br><br><strong>Lecture du 17 mai 2002</strong><div align=\"center\"><strong>REPUBLIQUE FRANCAISE</strong></div><br>Vu la requête, enregistrée le 18 mai 2000…</body> </html>";

    #[test]
    fn parse_dce_en_tete() {
        let d = parse_dce_html(DCE);
        assert_eq!(d.dossiers, vec!["221186"]);
        assert_eq!(
            d.ecli.as_deref(),
            Some("ECLI:FR:CESSR:2002:221186.20020517")
        );
        assert_eq!(
            d.niveau.as_deref(),
            Some("Mentionné au tables du recueil Lebon")
        );
        assert!(d
            .body
            .contains("Vu la requête, enregistrée le 18 mai 2000…"));
        assert!(d.body.starts_with("Conseil d'État"));
        // Le `<div>` de centrage vaut saut de ligne : pas de collage.
        assert!(!d.body.contains("2002REPUBLIQUE"));
    }

    #[test]
    fn parse_classement_seul() {
        let a = parse_ajce_html(CLASSEMENT_SEUL);
        assert_eq!(a.dossier.as_deref(), Some("99545"));
        assert_eq!(a.rubriques.len(), 2);
        assert_eq!(a.rubriques[0].code, "14-06-01");
        assert!(a.rubriques[0].label.starts_with("COMMERCE, INDUSTRIE"));
        assert!(a.rubriques[0].titre.is_empty());
        assert!(a.sommaires.is_empty());
        assert_eq!(analyse_body(&a), "");
    }

    #[test]
    fn body_groupe_titres_puis_sommaire() {
        let a = parse_ajce_html(ANCIEN);
        assert_eq!(
            analyse_body(&a),
            "Responsabilité - Absence - Services de police.\nAbsence de responsabilité sans faute - Algérie.\n\nSommaire unique répété."
        );
    }

    #[test]
    fn bundle_adr_0204() {
        let a = parse_ajce_html(ANCIEN);
        let entry = AjceEntry {
            body: analyse_body(&a),
            codes_pcja: vec!["05-03".into(), "60-01-02-01".into()],
            niveau: Some("B".into()),
            rubriques: a
                .rubriques
                .iter()
                .map(|r| format!("{} : {}", r.code, r.label))
                .collect(),
            renvois: a.renvois.clone(),
            date: Some("1976-03-03".into()),
        };
        let sf = build_ariane_source_fields("96402", "1976-03-03", None, &[entry], true);
        let commentaires = sf["commentaires"].as_array().unwrap();
        assert_eq!(commentaires.len(), 2);
        assert_eq!(commentaires[0]["kind"], "analyse");
        assert_eq!(commentaires[0]["meta"]["codes_pcja"][0], "05-03");
        // Conclusions = existence seule : aucun autre champ (ADR 0204).
        assert_eq!(commentaires[1], json!({ "kind": "conclusions" }));
        assert_eq!(sf["dossier"], "96402");
    }
}
