use super::Ui;

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

pub static NL: Ui = Ui {
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
        "bg" => "Bulgaars",
        "cs" => "Tsjechisch",
        "da" => "Deens",
        "de" => "Duits",
        "el" => "Grieks",
        "en" => "Engels",
        "es" => "Spaans",
        "et" => "Ests",
        "fi" => "Fins",
        "fr" => "Frans",
        "ga" => "Iers",
        "hr" => "Kroatisch",
        "hu" => "Hongaars",
        "it" => "Italiaans",
        "lb" => "Luxemburgs",
        "lt" => "Litouws",
        "lv" => "Lets",
        "mt" => "Maltees",
        "nl" => "Nederlands",
        "pl" => "Pools",
        "pt" => "Portugees",
        "rm" => "Reto-Romaans",
        "ro" => "Roemeens",
        "sk" => "Slowaaks",
        "sl" => "Sloveens",
        "sv" => "Zweeds",
        _ => return None,
    })
}

fn law_adjective(code: &str) -> Option<&'static str> {
    Some(match code {
        "be" => "Belgisch",
        "bf" => "Burkinees",
        "bg" => "Bulgaars",
        "bi" => "Burundees",
        "bj" => "Beninees",
        "ca" => "Canadees",
        "cd" | "cg" => "Congolees",
        "cf" => "Centraal-Afrikaans",
        "ch" => "Zwitsers",
        "ci" => "Ivoriaans",
        "cm" => "Kameroens",
        "dj" => "Djiboutiaans",
        "fr" => "Frans",
        "ga" => "Gabonees",
        "gn" => "Guinees",
        "ht" => "Haïtiaans",
        "hu" => "Hongaars",
        "km" => "Comorees",
        "lb" => "Libanees",
        "lu" => "Luxemburgs",
        "ma" => "Marokkaans",
        "mc" => "Monegaskisch",
        "mg" => "Malagassisch",
        "ml" => "Malinees",
        "ne" => "Nigerees",
        "nl" => "Nederlands",
        "sn" => "Senegalees",
        "td" => "Tsjadisch",
        "tg" => "Togolees",
        _ => return None,
    })
}

/// A journal by the name its Dutch edition prints, else by its label as it came.
fn journal_name(label: &str) -> &str {
    match label {
        "Moniteur belge" => "Belgisch Staatsblad",
        "Moniteur belge (Communauté et Région flamandes)" => {
            "Belgisch Staatsblad (Vlaamse Gemeenschap en Vlaams Gewest)"
        }
        "Moniteur belge (Région wallonne)" => "Belgisch Staatsblad (Waals Gewest)",
        "Moniteur belge (Région de Bruxelles-Capitale, COCOM et COCOF)" => {
            "Belgisch Staatsblad (Brussels Hoofdstedelijk Gewest, GGC en Franse Gemeenschapscommissie)"
        }
        "Moniteur belge (Communauté française)" => "Belgisch Staatsblad (Franse Gemeenschap)",
        "Moniteur belge (Communauté germanophone)" => {
            "Belgisch Staatsblad (Duitstalige Gemeenschap)"
        }
        "Journal officiel de l'Union européenne" => "Publicatieblad van de Europese Unie",
        label => label,
    }
}

/// An article number as a Dutch text writes it: the French "1er" is "1".
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
        [head @ .., last] => format!("{} en {last}", head.join(", ")),
    }
}
