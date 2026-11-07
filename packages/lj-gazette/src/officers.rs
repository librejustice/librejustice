//! Officers named in a BODACC `administration` text: `Président : X, Directeur général : Y`.

use std::sync::LazyLock;

use regex::Regex;

const KEYWORD: &str = r"(?:co-?)?(?:g[ée]rante?|pr[ée]sidente?|directeur|directrice|associ[ée]e?|commissaire|liquidateur|liquidatrice|administrateur|administratrice|membre|vice-pr[ée]sidente?|repr[ée]sentant|contr[ôo]leur|dirigeant|mandataire|secr[ée]taire|tr[ée]sori[eè]re?|personne ayant le pouvoir)";
const ACTION: &str = r"(?:(?:nomination|modification|confirmation|renouvellement|changement|d[ée]mission|d[ée]part)\s+(?:du|de la|de l'|de l\x{2019}|des|d'un|d'une)\s*)?";

static SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*:\s+").unwrap());
static TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?i)(?:^|[,;]\s*|\s)({ACTION}{KEYWORD}\b[^:]*)$")).unwrap()
});
static USAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[,;]?\s*nom d['\x{2019}]usage\s*$").unwrap());
static ACTION_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:nomination|modification|confirmation|renouvellement|changement|demission|depart)(?: (?:du|de la|de l|des|d un|d une))?\b|\bpartante?s?\b").unwrap()
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Officer {
    pub role: String,
    pub name: String,
}

/// The role folded and without the action naming it: `Nomination du Gérante` gives `gerant`.
pub fn canonical_role(role: &str) -> String {
    let folded = lj_fold::slug(&role.replace("(e)", ""), ' ');
    ACTION_WORDS
        .replace_all(&folded, " ")
        .split_whitespace()
        .map(|w| match w {
            "gerante" => "gerant",
            "presidente" => "president",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || c == ',' || c == ';' || c == '.')
}

/// Each name takes its role from the tail of the segment before it; a segment without one ends the list.
pub fn parse(administration: &str) -> Vec<Officer> {
    let parts: Vec<&str> = SPLIT.split(administration).collect();
    let mut out: Vec<Officer> = Vec::new();
    if parts.len() < 2 {
        return out;
    }
    let mut role: Option<String> = Some(parts[0].to_string());
    let last = parts.len() - 1;
    for (i, p) in parts.iter().enumerate().skip(1) {
        let (name, next) = if i < last {
            if USAGE.is_match(p) {
                (
                    USAGE.replace(p, "").into_owned(),
                    Some("@usage".to_string()),
                )
            } else if let Some(c) = TAIL.captures(p) {
                let m = c.get(1).expect("group 1");
                (p[..m.start()].to_string(), Some(m.as_str().to_string()))
            } else {
                (p.to_string(), None)
            }
        } else {
            (p.to_string(), None)
        };
        let name: String = trim(&name).chars().take(300).collect();
        match role.as_deref() {
            Some("@usage") if !out.is_empty() => {
                let prev = out.last_mut().expect("non-empty");
                prev.name = format!("{} ({name})", prev.name);
            }
            Some(r) => out.push(Officer {
                role: canonical_role(trim(r)),
                name,
            }),
            None => return out,
        }
        if next.is_none() && i < last {
            return out;
        }
        role = next;
    }
    out
}
