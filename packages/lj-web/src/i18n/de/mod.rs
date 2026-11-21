mod account;
mod api_guide;
mod dates;
mod decision;
mod directory;
mod entity;
mod feedback;
mod home;
mod hubs;
mod legal;
mod mcp_guide;
mod norm;
mod notices;
mod numbers;
mod open_source;
mod profile;
mod provenance;
mod redact;
mod referential;
mod search;
mod shell;
mod sources;
mod tools;

use super::Ui;

pub static DE: Ui = Ui {
    lang_name,
    law_adjective,
    list,
    shell: shell::SHELL,
    home: home::HOME,
    search: search::SEARCH,
    notices: notices::NOTICES,
    norm: norm::NORM,
    hubs: hubs::HUBS,
    sources: sources::SOURCES,
    legal: legal::LEGAL,
    open_source: open_source::OPEN_SOURCE,
    api_guide: api_guide::API_GUIDE,
    mcp_guide: mcp_guide::MCP_GUIDE,
    account: account::ACCOUNT,
    profile: profile::PROFILE,
    decision: decision::DECISION,
    tools: tools::TOOLS,
    feedback: feedback::FEEDBACK,
    dates: dates::DATES,
    directory: directory::DIRECTORY,
    entity: entity::ENTITY,
    redact: redact::REDACT,
    provenance: provenance::PROVENANCE,
    numbers: numbers::NUMBERS,
    referential: referential::REFERENTIAL,
};

fn lang_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "bg" => "Bulgarisch",
        "cs" => "Tschechisch",
        "da" => "Dänisch",
        "de" => "Deutsch",
        "el" => "Griechisch",
        "en" => "Englisch",
        "es" => "Spanisch",
        "et" => "Estnisch",
        "fi" => "Finnisch",
        "fr" => "Französisch",
        "ga" => "Irisch",
        "hr" => "Kroatisch",
        "hu" => "Ungarisch",
        "it" => "Italienisch",
        "lb" => "Luxemburgisch",
        "lt" => "Litauisch",
        "lv" => "Lettisch",
        "mt" => "Maltesisch",
        "nl" => "Niederländisch",
        "pl" => "Polnisch",
        "pt" => "Portugiesisch",
        "rm" => "Rätoromanisch",
        "ro" => "Rumänisch",
        "sk" => "Slowakisch",
        "sl" => "Slowenisch",
        "sv" => "Schwedisch",
        _ => return None,
    })
}

/// The form after the definite article, `das schweizerische Recht`.
fn law_adjective(code: &str) -> Option<&'static str> {
    Some(match code {
        "be" => "belgische",
        "bf" => "burkinische",
        "bg" => "bulgarische",
        "bi" => "burundische",
        "bj" => "beninische",
        "ca" => "kanadische",
        "cd" | "cg" => "kongolesische",
        "cf" => "zentralafrikanische",
        "ch" => "schweizerische",
        "ci" => "ivorische",
        "cm" => "kamerunische",
        "dj" => "dschibutische",
        "fr" => "französische",
        "ga" => "gabunische",
        "gn" => "guineische",
        "ht" => "haitianische",
        "hu" => "ungarische",
        "km" => "komorische",
        "lb" => "libanesische",
        "lu" => "luxemburgische",
        "ma" => "marokkanische",
        "mc" => "monegassische",
        "mg" => "madagassische",
        "ml" => "malische",
        "ne" => "nigrische",
        "nl" => "niederländische",
        "sn" => "senegalesische",
        "td" => "tschadische",
        "tg" => "togoische",
        _ => return None,
    })
}

/// A journal by the name its German edition prints, else by its label as it came.
fn journal_name(label: &str) -> &str {
    match label {
        "Moniteur belge" => "Belgisches Staatsblatt",
        "Moniteur belge (Communauté et Région flamandes)" => {
            "Belgisches Staatsblatt (Flämische Gemeinschaft und Flämische Region)"
        }
        "Moniteur belge (Région wallonne)" => "Belgisches Staatsblatt (Wallonische Region)",
        "Moniteur belge (Région de Bruxelles-Capitale, COCOM et COCOF)" => {
            "Belgisches Staatsblatt (Region Brüssel-Hauptstadt, GGK und Französische Gemeinschaftskommission)"
        }
        "Moniteur belge (Communauté française)" => {
            "Belgisches Staatsblatt (Französische Gemeinschaft)"
        }
        "Moniteur belge (Communauté germanophone)" => {
            "Belgisches Staatsblatt (Deutschsprachige Gemeinschaft)"
        }
        "Journal officiel de l'Union européenne" => "Amtsblatt der Europäischen Union",
        "Recueil officiel du droit fédéral" => "Amtliche Sammlung des Bundesrechts",
        "Feuille officielle du canton de Berne" => "Amtsblatt des Kantons Bern",
        "Feuille officielle du canton de Fribourg" => "Amtsblatt des Kantons Freiburg",
        "Bulletin officiel du canton du Valais" => "Amtsblatt des Kantons Wallis",
        label => label,
    }
}

/// An article number as a German text writes it: the French "1er" is "1".
fn article_num(num: &str) -> String {
    match num.strip_prefix("1er") {
        Some(rest) => format!("1{rest}"),
        None => num.to_string(),
    }
}

fn list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => only.to_string(),
        [head @ .., last] => format!("{} und {last}", head.join(", ")),
    }
}
