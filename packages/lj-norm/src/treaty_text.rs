//! A treaty as the JO prints it in the decree that publishes it, cut into the
//! treaty's own articles.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::article_key::identity_key;

/// One article of the publication decree, in reading order.
pub struct Entry<'a> {
    pub id: &'a str,
    pub text: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreatyArticle {
    /// The entry the article starts in.
    pub source: String,
    pub num: String,
    pub num_key: String,
    pub title_path: Option<String>,
    pub text: String,
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^article\s+(premier|1\s*er|ier|unique|\d+(?:\s*(?:bis|ter|quater|quinquies|sexies|septies|octies|[a-z]))?|[ivxlc]+(?:\s*(?:bis|ter))?)\s*\.?$",
    )
    .unwrap()
});

static PIECE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(ACCORD|CONVENTION|PROTOCOLE|AVENANT|ANNEXE|APPENDICE|[ÉE]CHANGE DE (?:LETTRES|NOTES)|LETTRE|NOTE|D[ÉE]CLARATION|M[ÉE]MORANDUM|ARRANGEMENT|ACTE FINAL|TRAIT[ÉE]|STATUTS?|R[ÈE]GLEMENT|D[ÉE]CISION|AMENDEMENTS?|R[ÉE]SOLUTION|CHARTE|PACTE|PROC[ÈE]S-VERBAL)(?:\s+(?:N[°O]\s*)?([0-9]+|[IVXLC]+|[A-Z])|([IVXLC]+))?(?:[\s.,:;)]|$)",
    )
    .unwrap()
});

static SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:titre|chapitre|section|sous-section|partie)\s+(?:premier|1\s*er|ier|unique|\d+|[ivxlc]+|[a-z])\b",
    )
    .unwrap()
});

static TITLE_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").unwrap());

static SPACED_CAPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\p{Lu} ){2,}\p{Lu}\b").unwrap());

static DECREE_ARTICLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:art\.|article)\s*(?:\d+|1\s*er|premier)(?:er)?\s*\.?\s*[-\x{2013}]")
        .unwrap()
});

static DECREE_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)pr[ée]sent d[ée]cret|ser(?:a|ont) publi[ée]e?s?").unwrap());

static NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\(\d+\)[^\n]*(?:en vigueur|pris effet|applicables? à compter)[^\n]*$")
        .unwrap()
});

static DECRETE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^d[ée]cr[eè]te\s*:?$").unwrap());

/// An unnumbered article of a publication decree that is the decree's own and
/// not the treaty's: « Décrète : », a note `(1)` standing alone, the article
/// ordering the publication or its execution, a notice that the text is not
/// reproduced. A note printed before the treaty stays in its preamble.
pub fn is_decree_paragraph(text: &str) -> bool {
    let t = text.trim();
    DECRETE.is_match(t)
        || t.chars().count() < 300 && NOTE.is_match(t)
        || t.chars().count() < 1500 && DECREE_ARTICLE.is_match(t) && DECREE_WORDS.is_match(t)
        || t.chars().count() < 600 && t.contains("Vous pouvez consulter")
}

/// The lines of a section title the cut reads: a chapter's title may carry the
/// heading of its first article, `Titre Ier : X  Article 1er  Y`.
pub fn title_lines(title: &str) -> Vec<&str> {
    let lines: Vec<&str> = TITLE_BREAK.split(title.trim()).collect();
    match lines
        .iter()
        .position(|l| SECTION.is_match(l) || heading(l).is_some())
    {
        Some(i) => lines[i..].to_vec(),
        None => Vec::new(),
    }
}

/// `A C C O R D SOUS FORME` reads `ACCORD SOUS FORME`.
fn unspaced(line: &str) -> String {
    match SPACED_CAPS.find(line) {
        Some(m) => format!("{}{}", m.as_str().replace(' ', ""), &line[m.end()..]),
        None => line.to_string(),
    }
}

fn is_capitals(line: &str) -> bool {
    let letters: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() >= 4
        && letters.iter().filter(|c| c.is_uppercase()).count() * 10 >= letters.len() * 9
}

struct Piece {
    num: String,
    key: String,
}

/// The piece a line in capitals opens: `PROTOCOLE N° 1 RELATIF …` gives
/// `Protocole 1`, keyed `protocole-1`.
fn piece(line: &str) -> Option<Piece> {
    let line = unspaced(line);
    if !is_capitals(&line) {
        return None;
    }
    let caps = PIECE.captures(&line)?;
    let kind = caps.get(1)?.as_str();
    let mut num = format!(
        "{}{}",
        kind.chars().next()?,
        kind.chars().skip(1).collect::<String>().to_lowercase()
    );
    if let Some(n) = caps.get(2).or(caps.get(3)) {
        num.push(' ');
        num.push_str(n.as_str());
    }
    let key = lj_fold::slug(&num, '-');
    Some(Piece { num, key })
}

fn heading(line: &str) -> Option<String> {
    let num = HEADING.captures(line)?.get(1)?.as_str();
    Some(if num.eq_ignore_ascii_case("ier") {
        "1er".to_string()
    } else {
        num.split_whitespace().collect::<Vec<_>>().join(" ")
    })
}

struct Draft {
    source: String,
    num: String,
    piece: Option<String>,
    title_path: Option<String>,
    lines: Vec<String>,
}

/// Section lines a long line follows were text: they go back to the article.
fn spill(current: &mut Option<Draft>, section: &mut Vec<String>, source: &str) {
    match current.as_mut() {
        Some(d) => d.lines.append(section),
        None if section.is_empty() => {}
        None => {
            *current = Some(Draft {
                source: source.to_string(),
                num: String::new(),
                piece: None,
                title_path: None,
                lines: std::mem::take(section),
            })
        }
    }
}

/// Cuts a treaty at the article headings that stand alone on their line; a
/// heading opened by a guillemet stays in the article that quotes it.
pub fn cut(entries: &[Entry]) -> Vec<TreatyArticle> {
    let mut drafts: Vec<Draft> = Vec::new();
    let mut current: Option<Draft> = None;
    let mut piece_key: Option<String> = None;
    let mut seen_heading = false;
    let mut section: Vec<String> = Vec::new();
    let mut in_section = false;

    for entry in entries {
        for raw in entry.text.lines() {
            let line = raw.trim();
            if let Some(num) = heading(line) {
                drafts.extend(current.take());
                let title_path = (!section.is_empty()).then(|| section.join(" "));
                in_section = false;
                current = Some(Draft {
                    source: entry.id.to_string(),
                    num,
                    piece: piece_key.clone(),
                    title_path,
                    lines: Vec::new(),
                });
                seen_heading = true;
                continue;
            }
            if seen_heading {
                if let Some(p) = piece(line) {
                    drafts.extend(current.take());
                    section.clear();
                    in_section = false;
                    piece_key = Some(p.key.clone());
                    current = Some(Draft {
                        source: entry.id.to_string(),
                        num: p.num,
                        piece: None,
                        title_path: None,
                        lines: vec![line.to_string()],
                    });
                    continue;
                }
            }
            if SECTION.is_match(line) && line.chars().count() < 200 {
                section = vec![line.to_string()];
                in_section = true;
                continue;
            }
            if in_section {
                if line.is_empty() {
                    continue;
                }
                if line.chars().count() < 200 && section.len() < 3 {
                    section.push(line.to_string());
                    continue;
                }
                spill(&mut current, &mut section, entry.id);
                in_section = false;
            }
            match current.as_mut() {
                Some(d) => d.lines.push(raw.trim_end().to_string()),
                None if line.is_empty() => {}
                None => {
                    current = Some(Draft {
                        source: entry.id.to_string(),
                        num: String::new(),
                        piece: None,
                        title_path: None,
                        lines: vec![raw.trim_end().to_string()],
                    })
                }
            }
        }
    }
    if in_section {
        let source = entries.last().map_or("", |e| e.id);
        spill(&mut current, &mut section, source);
    }
    drafts.extend(current);

    let headed = drafts.iter().any(|d| !d.num.is_empty());
    let mut used: HashSet<String> = HashSet::new();
    drafts
        .into_iter()
        .filter_map(|d| {
            let text = d.lines.join("\n").trim().to_string();
            if text.is_empty() {
                return None;
            }
            let num = match d.num.as_str() {
                "" if headed => "Préambule".to_string(),
                "" => "unique".to_string(),
                n => n.to_string(),
            };
            let bare = identity_key(&num);
            let mut key = match &d.piece {
                Some(p) => format!("{p}-{bare}"),
                None => bare,
            };
            let base = key.clone();
            let mut n = 2;
            while !used.insert(key.clone()) {
                key = format!("{base}-{n}");
                n += 1;
            }
            Some(TreatyArticle {
                source: d.source,
                num,
                num_key: key,
                title_path: d.title_path,
                text,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nums(articles: &[TreatyArticle]) -> Vec<(&str, &str)> {
        articles
            .iter()
            .map(|a| (a.num.as_str(), a.num_key.as_str()))
            .collect()
    }

    #[test]
    fn an_amendment_keeps_the_articles_it_quotes_inside_its_own() {
        let text = "AVENANT\nÀ LA CONVENTION DU 21 JUILLET 1959\n\nLe Gouvernement de la République française\nsont convenus des dispositions suivantes :\nArticle Ier\nL'article 7 est remplacé :\n« Article 7\n(1) Les revenus…\nArticle II\nL'article 14 est remplacé :\n« Article 14\n(2) Les pensions…\nArticle XIX\nFait à Berlin, le 31 mars 2015.";
        let cut = cut(&[Entry {
            id: "JORFARTI1",
            text,
        }]);
        assert_eq!(
            nums(&cut),
            [
                ("Préambule", "preambule"),
                ("1er", "1"),
                ("II", "ii"),
                ("XIX", "xix")
            ]
        );
        assert!(cut[2].text.contains("« Article 14\n(2) Les pensions"));
        assert!(cut[0].text.starts_with("AVENANT"));
    }

    #[test]
    fn one_entry_per_article_keeps_each_article_s_source() {
        let entries = [
            Entry {
                id: "A",
                text: "A C C O R D DE PARTENARIAT",
            },
            Entry {
                id: "B",
                text: "Article 1er\nUn partenariat est établi.",
            },
            Entry {
                id: "C",
                text: "Article 2\nLe respect de la démocratie.",
            },
        ];
        let cut = cut(&entries);
        assert_eq!(
            nums(&cut),
            [("Préambule", "preambule"), ("1er", "1"), ("2", "2")]
        );
        assert_eq!(
            cut.iter().map(|a| a.source.as_str()).collect::<Vec<_>>(),
            ["A", "B", "C"]
        );
        assert_eq!(cut[2].text, "Le respect de la démocratie.");
    }

    #[test]
    fn a_protocol_after_the_treaty_keys_its_articles_by_itself() {
        let text = "CONVENTION\nArticle 1\nObjet.\nArticle 2\nFait à Paris.\nPROTOCOLE N° 1 RELATIF AUX PENSIONS\nLes parties sont convenues :\nArticle 1\nLes pensions.\nÉCHANGE DE LETTRES\nMonsieur le Ministre,";
        let cut = cut(&[Entry { id: "X", text }]);
        assert_eq!(
            nums(&cut),
            [
                ("Préambule", "preambule"),
                ("1", "1"),
                ("2", "2"),
                ("Protocole 1", "protocole-1"),
                ("1", "protocole-1-1"),
                ("Échange de lettres", "echange-de-lettres"),
            ]
        );
        assert!(cut[3].text.starts_with("PROTOCOLE N° 1 RELATIF"));
    }

    #[test]
    fn a_section_heading_titles_the_next_article_and_leaves_the_previous_one() {
        let text =
            "Article 1\nObjet.\nTITRE II\nDispositions finales\nArticle 2\nEntrée en vigueur.";
        let cut = cut(&[Entry { id: "X", text }]);
        assert_eq!(cut[0].text, "Objet.");
        assert_eq!(
            cut[1].title_path.as_deref(),
            Some("TITRE II Dispositions finales")
        );
    }

    #[test]
    fn a_chapter_title_carries_the_heading_of_its_first_article() {
        assert_eq!(
            title_lines("Titre Ier : DÉFINITIONS  Article 1er  Définitions"),
            ["Titre Ier : DÉFINITIONS", "Article 1er", "Définitions"]
        );
        assert!(title_lines("Annexe").is_empty());
        let entries = [
            Entry {
                id: "A",
                text: "ACCORD SUR LA SÉCURITÉ SOCIALE\nTitre Ier : DÉFINITIONS",
            },
            Entry {
                id: "B",
                text: "Article 1er\nDéfinitions\n1. Aux fins du présent Accord…",
            },
        ];
        let cut = cut(&entries);
        assert_eq!(cut[0].text, "ACCORD SUR LA SÉCURITÉ SOCIALE");
        assert_eq!(
            cut[1].title_path.as_deref(),
            Some("Titre Ier : DÉFINITIONS")
        );
        assert_eq!(cut[1].source, "B");
    }

    #[test]
    fn a_piece_s_title_is_numbered_only_by_a_number() {
        assert_eq!(
            piece("CONVENTION D'ENTRAIDE JUDICIAIRE").map(|p| p.key),
            Some("convention".into())
        );
        assert_eq!(
            piece("ANNEXE III").map(|p| p.key),
            Some("annexe-iii".into())
        );
        assert_eq!(piece("ANNEXE B").map(|p| p.key), Some("annexe-b".into()));
        assert_eq!(
            piece("A N N E X E V I I TEXTE").map(|p| p.key),
            Some("annexe-vii".into())
        );
        assert_eq!(piece("Annexe III").map(|p| p.key), None);
    }

    #[test]
    fn letters_with_no_heading_are_one_article() {
        let cut = cut(&[Entry {
            id: "X",
            text: "Monsieur le Ministre,\nJ'ai l'honneur…",
        }]);
        assert_eq!(nums(&cut), [("unique", "unique")]);
    }

    #[test]
    fn the_decree_s_own_paragraphs_are_not_the_treaty() {
        for t in [
            "Décrète :",
            "(1) Le présent accord est entré en vigueur le 19 septembre 1995.",
            "Art. 1er. - L'accord sous forme d'échange de notes, signé à Paris, sera publié au Journal officiel de la République française.",
            "Art. 2. - Le Premier ministre et le ministre des affaires étrangères sont chargés de l'exécution du présent décret.",
            "Vous pouvez consulter l'image dans le fac-similé du JO nº 0123.",
        ] {
            assert!(is_decree_paragraph(t), "{t}");
        }
        for t in [
            "Article 1er\nUn partenariat est établi.",
            "A C C O R D SOUS FORME D'ECHANGE DE NOTES",
            "Art. 3. - Les parties contractantes s'engagent à coopérer.",
            "(1) Les deux Etats reconnaissent l'importance de la coopération transfrontalière.",
            "(1) Le présent accord est entré en vigueur le 9 mai 1995.\n\nACCORD\nENTRE LA FRANCE ET LA BULGARIE",
        ] {
            assert!(!is_decree_paragraph(t), "{t}");
        }
    }
}
