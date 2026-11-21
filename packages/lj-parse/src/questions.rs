//! Parliamentary questions: the Assemblée's open data, its JO issues as DILA serves them, the Sénat's base.

use std::sync::LazyLock;

use jiff::civil::Date;
use regex::Regex;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chamber {
    Assemblee,
    Senat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub chamber: Chamber,
    /// `QANR5L17QE5275`, `SEQ780402185`.
    pub uid: String,
    pub legislature: u16,
    /// As printed: `5275`, at the Sénat with its kind's letter, `1276G`.
    pub number: String,
    /// `QE`, `QG`, `QOSD`, and at the Sénat `QOAD`, `QOAE`, `QC`.
    pub kind: String,
    pub author: Author,
    /// The ministry the question was addressed to.
    pub ministry: Option<String>,
    pub rubric: Option<String>,
    /// The Assemblée's analysis, the Sénat's title.
    pub title: Option<String>,
    /// The question as printed, then each re-publication.
    pub asked: Vec<Printing>,
    pub answers: Vec<Printing>,
    /// How the chamber closed it: « Réponse publiée », « Caduque ».
    pub closure: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Author {
    /// « M. Christophe Bex ».
    pub name: Option<String>,
    /// The Assemblée's actor reference, `PA793772`, the Sénat's matricule.
    pub actor: Option<String>,
    pub group: Option<String>,
    pub constituency: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Printing {
    pub date: Option<Date>,
    pub page: Option<u32>,
    /// The ministry that answered.
    pub ministry: Option<String>,
    pub text: String,
    pub erratum: Option<String>,
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static BREAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<br\s*/?>|</p>|</div>|</li>|</tr>|</center>|</table>|</h\d>").unwrap()
});
static CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</t[dh]>").unwrap());
static LETTERHEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:M\.|Mme|Mlle) [^.;:]{2,80}? (?:attire|interroge|appelle|alerte|interpelle|souhaite|rappelle|demande|expose|signale|s'inquiète|s'interroge)\b").unwrap()
});
static VERB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" \S+$").unwrap());
static SPEAKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<strong>\s*((?:M\.|Mme) [^<]+?)\s*\.?\s*</strong>").unwrap());

/// An HTML fragment as text, one line per paragraph, break or table row.
fn plain(html: &str) -> String {
    let text = BREAK.replace_all(html, "\n");
    let text = CELL.replace_all(&text, " ");
    let text = TAG.replace_all(&text, "");
    lj_core::normalizer::normalize_chars(&html_escape::decode_html_entities(&text))
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(v, |v, k| v.get(k))?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// A repeated node of the referential: an object when it is one, an array when several.
fn one_or_many(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(v) => vec![v],
    }
}

/// `yyyy-mm-dd`, else `dd/mm/yyyy` as the 15th legislature writes it, a time after either ignored.
fn day(s: &str) -> Option<Date> {
    let s = s.get(..10)?;
    s.parse().ok().or_else(|| {
        let (d, rest) = s.split_once('/')?;
        let (m, y) = rest.split_once('/')?;
        Date::new(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?).ok()
    })
}

fn printing(node: &Value, ministry: Option<&str>) -> Printing {
    let jo = node.get("infoJO");
    Printing {
        date: jo.and_then(|j| str_at(j, &["dateJO"])).and_then(day),
        page: jo
            .and_then(|j| str_at(j, &["pageJO"]))
            .and_then(|p| p.parse().ok()),
        ministry: ministry.map(str::to_string),
        text: str_at(node, &["texte"]).map(plain).unwrap_or_default(),
        erratum: str_at(node, &["erratum"]).map(plain),
    }
}

/// The asker named at the head of a written question: « M. Christophe Bex attire l'attention… ».
fn letterhead_name(text: &str) -> Option<String> {
    let head = LETTERHEAD.find(text)?.as_str();
    Some(VERB.replace(head, "").into_owned())
}

/// The first speaker of an exchange in the chamber who is not its chair.
fn first_speaker(html: &str) -> Option<String> {
    SPEAKER
        .captures_iter(html)
        .map(|c| c[1].trim_end_matches('.').trim().to_string())
        .find(|name| !name.contains("président"))
}

/// One record of the Assemblée's open data, the object under `question`.
pub fn an_question(record: &Value) -> Result<Question, String> {
    let uid = str_at(record, &["uid"]).ok_or("question without uid")?;
    let number = str_at(record, &["identifiant", "numero"])
        .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("{uid}: no number"))?
        .to_string();
    let legislature = str_at(record, &["identifiant", "legislature"])
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("{uid}: no legislature"))?;
    let kind = str_at(record, &["type"]).ok_or_else(|| format!("{uid}: no type"))?;
    let index = record.get("indexationAN");
    let analyses = index.and_then(|i| {
        i.get("analyses")
            .and_then(|a| a.get("analyse"))
            .or_else(|| i.get("ANALYSE").and_then(|a| a.get("ANA")))
    });
    let title: Vec<&str> = index
        .and_then(|i| str_at(i, &["teteAnalyse"]))
        .into_iter()
        .chain(
            one_or_many(analyses)
                .into_iter()
                .filter_map(|a| a.as_str().map(str::trim)),
        )
        .filter(|s| !s.is_empty())
        .collect();
    let answering = one_or_many(record.get("minAttribs").and_then(|m| m.get("minAttrib")))
        .last()
        .and_then(|m| str_at(m, &["denomination", "developpe"]));
    let asked: Vec<Printing> = one_or_many(
        record
            .get("textesQuestion")
            .and_then(|t| t.get("texteQuestion")),
    )
    .into_iter()
    .map(|n| printing(n, None))
    .collect();
    let answer_nodes = one_or_many(
        record
            .get("textesReponse")
            .and_then(|t| t.get("texteReponse")),
    );
    let name = asked
        .first()
        .and_then(|p| letterhead_name(&p.text))
        .or_else(|| {
            answer_nodes
                .first()
                .and_then(|n| str_at(n, &["texte"]))
                .and_then(first_speaker)
        });
    Ok(Question {
        chamber: Chamber::Assemblee,
        uid: uid.to_string(),
        legislature,
        number,
        kind: kind.to_string(),
        author: Author {
            name,
            actor: str_at(record, &["auteur", "identite", "acteurRef"]).map(str::to_string),
            group: str_at(record, &["auteur", "groupe", "abrege"]).map(str::to_string),
            constituency: None,
        },
        ministry: str_at(record, &["minInt", "developpe"]).map(str::to_string),
        rubric: index
            .and_then(|i| str_at(i, &["rubrique"]))
            .map(str::to_string),
        title: (!title.is_empty()).then(|| title.join(", ")),
        asked,
        answers: answer_nodes
            .into_iter()
            .map(|n| printing(n, answering))
            .collect(),
        closure: str_at(record, &["cloture", "libelleCloture"]).map(str::to_string),
    })
}

/// One issue of the Assemblée's questions edition of the JO, `XML1JO_AN<number>.xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoIssue {
    /// `20130003`.
    pub number: String,
    pub published: Date,
    /// The questions the issue prints: `(oral, number, page)`.
    pub questions: Vec<(bool, u32, u32)>,
    pub answers: Vec<IssueAnswer>,
}

/// An answer as the issue prints it, to one question or to several at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueAnswer {
    pub questions: Vec<u32>,
    pub page: u32,
    pub text: String,
}

static ISSUE_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<numero_jo>(\d+)</numero_jo>\s*<date_publication>([\d/]+)</date_publication>")
        .unwrap()
});
static PRINTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<question(Ecrite|Orale)>\s*<numero>(\d+)</numero>\s*<page_jo>(\d+)</page_jo>")
        .unwrap()
});
static ANSWER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<questionReponse>(.*?)<reponse>\s*<page_jo>(\d+)</page_jo>\s*<texteReponse>(.*?)</texteReponse>").unwrap()
});
static ASKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<question>\s*<numero>(\d+)</numero>").unwrap());

/// An issue's bytes, Latin-1 as its prolog declares, else UTF-8.
pub fn jo_issue(bytes: &[u8]) -> Result<JoIssue, String> {
    let prolog = &bytes[..bytes.len().min(100)];
    let xml: String = if prolog
        .windows(10)
        .any(|w| w.eq_ignore_ascii_case(b"ISO-8859-1"))
    {
        bytes.iter().map(|&b| b as char).collect()
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?
    };
    let head = ISSUE_HEAD
        .captures(&xml)
        .ok_or("issue without number or day")?;
    let published = day(&head[2]).ok_or_else(|| format!("issue day {}", &head[2]))?;
    let num = |s: &str| s.parse::<u32>().map_err(|e| e.to_string());
    let questions = PRINTED
        .captures_iter(&xml)
        .map(|c| Ok((&c[1] == "Orale", num(&c[2])?, num(&c[3])?)))
        .collect::<Result<_, String>>()?;
    let answers = ANSWER
        .captures_iter(&xml)
        .map(|c| {
            Ok(IssueAnswer {
                questions: ASKED
                    .captures_iter(&c[1])
                    .map(|q| num(&q[1]))
                    .collect::<Result<_, _>>()?,
                page: num(&c[2])?,
                text: plain(&c[3]),
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(JoIssue {
        number: head[1].to_string(),
        published,
        questions,
        answers,
    })
}

/// The Assemblée's legislature on a day it sits, from the 13th.
pub fn an_legislature(on: Date) -> Option<u16> {
    [
        (17, (2024, 7, 18)),
        (16, (2022, 6, 22)),
        (15, (2017, 6, 21)),
        (14, (2012, 6, 20)),
        (13, (2007, 6, 20)),
    ]
    .into_iter()
    .find(|&(_, (y, m, d))| on >= jiff::civil::date(y, m, d))
    .map(|(l, _)| l)
}

/// A row of a pg_dump's `COPY … FROM stdin` block, its values by column.
pub struct CopyRow<'a> {
    pub columns: &'a [String],
    pub values: Vec<Option<String>>,
}

impl CopyRow<'_> {
    pub fn get(&self, column: &str) -> Option<&str> {
        let i = self.columns.iter().position(|c| c == column)?;
        self.values[i]
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

/// `COPY public.tam_questions (id, numero, …) FROM stdin;` as the table and its columns.
pub fn copy_header(line: &str) -> Option<(String, Vec<String>)> {
    let rest = line.strip_prefix("COPY ")?;
    let (table, rest) = rest.split_once(" (")?;
    let (columns, _) = rest.split_once(") FROM stdin;")?;
    let table = table.rsplit('.').next().unwrap_or(table);
    Some((
        table.to_string(),
        columns.split(", ").map(str::to_string).collect(),
    ))
}

/// One line of a `COPY` block: tab-separated, `\N` for null, backslash escapes.
pub fn copy_values(line: &str) -> Vec<Option<String>> {
    line.trim_end_matches('\n')
        .split('\t')
        .map(|field| {
            if field == r"\N" {
                return None;
            }
            let mut out = String::with_capacity(field.len());
            let mut chars = field.chars();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some(other) => out.push(other),
                    None => {}
                }
            }
            Some(out)
        })
        .collect()
}

/// How the Sénat closed a question, its `sortquestion` labels.
fn senat_closure(code: &str) -> Result<&'static str, String> {
    Ok(match code {
        "0" => "En cours",
        "10" => "Réponse reçue",
        "20" => "Transformée (QE en QOSD)",
        "30" => "Transformée (autres)",
        "50" => "Retirée",
        "60" => "Retirée (décès)",
        "70" => "Retirée (fin de mandat)",
        "80" => "Retirée (rappelée répondue)",
        "100" => "Caduque",
        "200" => "Directe",
        other => return Err(format!("unknown sorquecod {other}")),
    })
}

/// A row of the Sénat's `tam_questions`, its answers still to attach by its `id`.
pub fn senat_question(row: &CopyRow) -> Result<(u64, Question), String> {
    let id = row
        .get("id")
        .and_then(|i| i.parse().ok())
        .ok_or("question without id")?;
    let uid = row
        .get("reference")
        .ok_or_else(|| format!("{id}: no reference"))?;
    let number = row
        .get("numero")
        .ok_or_else(|| format!("{uid}: no number"))?
        .to_string();
    let legislature = row
        .get("legislature")
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("{uid}: no legislature"))?;
    let name: Vec<&str> = ["codequalite", "prenom", "nom"]
        .into_iter()
        .filter_map(|c| row.get(c))
        .collect();
    let text = row.get("txtque").map(plain).unwrap_or_default();
    let asked = (row.get("datejodepot").is_some() || !text.is_empty()).then(|| Printing {
        date: row.get("datejodepot").and_then(day),
        page: row.get("pagejodepot").and_then(|p| p.parse().ok()),
        ministry: None,
        text,
        erratum: row.get("txterrque").map(plain),
    });
    Ok((
        id,
        Question {
            chamber: Chamber::Senat,
            uid: uid.to_string(),
            legislature,
            number,
            kind: row
                .get("natquecod")
                .ok_or_else(|| format!("{uid}: no nature"))?
                .to_string(),
            author: Author {
                name: (name.len() > 1).then(|| name.join(" ")),
                actor: row.get("matricule").map(str::to_string),
                group: row.get("groupe").map(str::to_string),
                constituency: row.get("circonscription").map(str::to_string),
            },
            ministry: row
                .get("mintranlib")
                .or(row.get("mindepotlib"))
                .map(str::to_string),
            rubric: row.get("rubrique").map(str::to_string),
            title: row.get("titre").map(str::to_string),
            asked: asked.into_iter().collect(),
            answers: Vec::new(),
            closure: row
                .get("sorquecod")
                .map(senat_closure)
                .transpose()?
                .map(str::to_string),
        },
    ))
}

/// A row of the Sénat's `tam_reponses`, with the `id` of its question.
pub fn senat_answer(row: &CopyRow) -> Result<(u64, Printing), String> {
    let question = row
        .get("idque")
        .and_then(|i| i.parse().ok())
        .ok_or("answer without question")?;
    Ok((
        question,
        Printing {
            date: row.get("datejorep").and_then(day),
            page: row.get("pagejorep").and_then(|p| p.parse().ok()),
            ministry: row.get("minreplib").map(str::to_string),
            text: row.get("txtrep").map(plain).unwrap_or_default(),
            erratum: row.get("txterrrep").map(plain),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_record_reads_one_or_many_nodes_both_date_forms_and_the_letterhead() {
        let record = serde_json::json!({
            "uid": "QANR5L15QE100", "type": "QE",
            "identifiant": {"numero": "100", "legislature": "15"},
            "indexationAN": {"rubrique": "culture", "teteAnalyse": null,
                             "analyses": {"analyse": ["artistes-auteurs", "statut"]}},
            "auteur": {"identite": {"acteurRef": "PA1"}, "groupe": {"abrege": "LFI"}},
            "minInt": {"developpe": "Ministère de la culture"},
            "minAttribs": {"minAttrib": [
                {"denomination": {"developpe": "Ministère de la culture"}},
                {"denomination": {"developpe": "Ministère du travail"}}]},
            "textesQuestion": {"texteQuestion": {"infoJO": {"dateJO": "25/03/2018", "pageJO": "1954"},
                "texte": "Mme Anne-Marie Dupont-Aignan interroge M. le ministre sur la man&#339;uvre.<br/>Merci."}},
            "textesReponse": {"texteReponse": [
                {"infoJO": {"dateJO": "2018-05-27"}, "texte": "Première."},
                {"infoJO": {"dateJO": "2018-06-03", "pageJO": "4000"}, "texte": "Corrigée.", "erratum": "Lire « 12 »."}]},
            "cloture": {"libelleCloture": "Réponse publiée"}
        });
        let q = an_question(&record).unwrap();
        assert_eq!(
            (q.legislature, q.number.as_str(), q.kind.as_str()),
            (15, "100", "QE")
        );
        assert_eq!(
            q.author.name.as_deref(),
            Some("Mme Anne-Marie Dupont-Aignan")
        );
        assert_eq!(q.title.as_deref(), Some("artistes-auteurs, statut"));
        assert_eq!(q.asked[0].date, Some(jiff::civil::date(2018, 3, 25)));
        assert_eq!(q.asked[0].page, Some(1954));
        assert_eq!(
            q.asked[0].text,
            "Mme Anne-Marie Dupont-Aignan interroge M. le ministre sur la manœuvre.\nMerci."
        );
        assert_eq!(q.answers.len(), 2);
        assert_eq!(
            q.answers[1].ministry.as_deref(),
            Some("Ministère du travail")
        );
        assert_eq!(q.answers[1].erratum.as_deref(), Some("Lire « 12 »."));
    }

    #[test]
    fn a_government_question_takes_its_asker_from_the_exchange() {
        let record = serde_json::json!({
            "uid": "QANR5L14QG2346", "type": "QG",
            "identifiant": {"numero": "2346", "legislature": "14"},
            "textesReponse": {"texteReponse": {"infoJO": {"dateJO": "2014-11-27"},
                "texte": "<strong>M. le président. </strong>La parole est à M. Gérard Charasse.<br><br><strong>M. Gérard Charasse. </strong>Monsieur le ministre…"}}
        });
        let q = an_question(&record).unwrap();
        assert_eq!(q.author.name.as_deref(), Some("M. Gérard Charasse"));
        assert!(q.asked.is_empty());
    }

    #[test]
    fn an_issue_gives_each_answer_its_questions_and_page() {
        let xml = "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><publicationJOQuestion><numero_jo>20250052</numero_jo><date_publication>30/12/2025</date_publication>\n<questionEcrite><numero>12084</numero><page_jo>10631</page_jo></questionEcrite>\n<questionReponse><question><numero>5078</numero><page_jo>10665</page_jo></question><question><numero>5550</numero><page_jo>10665</page_jo></question><reponse><page_jo>10666</page_jo><texteReponse>Le minist\u{e8}re<br/>r\u{e9}pond.</texteReponse></reponse></questionReponse></publicationJOQuestion>";
        let latin1: Vec<u8> = xml.chars().map(|c| c as u8).collect();
        let issue = jo_issue(&latin1).unwrap();
        assert_eq!(issue.published, jiff::civil::date(2025, 12, 30));
        assert_eq!(issue.questions, vec![(false, 12084, 10631)]);
        assert_eq!(issue.answers[0].questions, vec![5078, 5550]);
        assert_eq!(issue.answers[0].page, 10666);
        assert_eq!(issue.answers[0].text, "Le ministère\nrépond.");
        assert_eq!(an_legislature(issue.published), Some(17));
    }

    #[test]
    fn a_senat_dump_row_unescapes_and_names_its_columns() {
        let (table, columns) = copy_header(
            "COPY public.tam_reponses (idque, datejorep, txtrep, minreplib, pagejorep) FROM stdin;",
        )
        .unwrap();
        assert_eq!(table, "tam_reponses");
        let row = CopyRow {
            columns: &columns,
            values: copy_values(
                "92\t1978-05-27 00:00:00\tLigne un\\nligne deux\\\\fin\tIndustrie\t\\N\n",
            ),
        };
        let (question, answer) = senat_answer(&row).unwrap();
        assert_eq!(question, 92);
        assert_eq!(answer.date, Some(jiff::civil::date(1978, 5, 27)));
        assert_eq!(answer.text, "Ligne un\nligne deux\\fin");
        assert_eq!(answer.page, None);
    }
}
