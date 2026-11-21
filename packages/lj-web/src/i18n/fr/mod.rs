use super::Ui;

pub(super) mod account;
pub(super) mod api_guide;
pub(super) mod dates;
pub(super) mod decision;
pub(super) mod directory;
pub(super) mod entity;
pub(super) mod feedback;
pub(super) mod home;
pub(super) mod hubs;
pub(super) mod legal;
pub(super) mod mcp_guide;
pub(super) mod norm;
pub(super) mod notices;
pub(super) mod numbers;
pub(super) mod open_source;
pub(super) mod profile;
pub(super) mod provenance;
pub(super) mod redact;
pub(super) mod referential;
pub(super) mod search;
pub(super) mod shell;
pub(super) mod sources;
pub(super) mod tools;

pub static FR: Ui = Ui {
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

pub(super) fn lang_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "bg" => "bulgare",
        "cs" => "tchèque",
        "da" => "danois",
        "de" => "allemand",
        "el" => "grec",
        "en" => "anglais",
        "es" => "espagnol",
        "et" => "estonien",
        "fi" => "finnois",
        "fr" => "français",
        "ga" => "irlandais",
        "hr" => "croate",
        "hu" => "hongrois",
        "it" => "italien",
        "lb" => "luxembourgeois",
        "lt" => "lituanien",
        "lv" => "letton",
        "mt" => "maltais",
        "nl" => "néerlandais",
        "pl" => "polonais",
        "pt" => "portugais",
        "rm" => "romanche",
        "ro" => "roumain",
        "sk" => "slovaque",
        "sl" => "slovène",
        "sv" => "suédois",
        _ => return None,
    })
}

pub(super) fn law_adjective(code: &str) -> Option<&'static str> {
    Some(match code {
        "be" => "belge",
        "bf" => "burkinabè",
        "bg" => "bulgare",
        "bi" => "burundais",
        "bj" => "béninois",
        "ca" => "canadien",
        "cd" | "cg" => "congolais",
        "cf" => "centrafricain",
        "ch" => "suisse",
        "ci" => "ivoirien",
        "cm" => "camerounais",
        "dj" => "djiboutien",
        "fr" => "français",
        "ga" => "gabonais",
        "gn" => "guinéen",
        "ht" => "haïtien",
        "hu" => "hongrois",
        "km" => "comorien",
        "lb" => "libanais",
        "lu" => "luxembourgeois",
        "ma" => "marocain",
        "mc" => "monégasque",
        "mg" => "malgache",
        "ml" => "malien",
        "ne" => "nigérien",
        "nl" => "néerlandais",
        "sn" => "sénégalais",
        "td" => "tchadien",
        "tg" => "togolais",
        _ => return None,
    })
}

pub(super) fn list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => only.to_string(),
        [head @ .., last] => format!("{} et {last}", head.join(", ")),
    }
}
