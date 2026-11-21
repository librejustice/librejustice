//! JOAFE files, in three schemas: `parution` (1901-1996 and since 2020, in JSON since parution 20260045),
//! `PARUTION_JO_ASSOCIATION` (2004-2019) and `ANNONCE_REF` alone, one file per notice (1997-2003).

use crate::record::{decompose, parse_any_date, Codec, Column, Record, Spec};
use crate::tree::{Body, Node};
use crate::{dict, Error};

/// Column ids are written into stored shapes: append only.
pub static SPEC: Spec = Spec::new(&[
    Column {
        name: "id",
        codec: Codec::Long,
        paths: &["metadonnees/identifiant", "IDENTIFIANT"],
    },
    Column {
        name: "num",
        codec: Codec::Int,
        paths: &["metadonnees/numAnnonce", "@numannonce"],
    },
    Column {
        name: "dept",
        codec: Codec::Dict(dict::JOAFE_DEPT),
        paths: &["metadonnees/dept", "@dept"],
    },
    Column {
        name: "type_code",
        codec: Codec::Int,
        paths: &["metadonnees/type/@code", "TYPE/@code"],
    },
    Column {
        name: "type_label",
        codec: Codec::Dict(dict::JOAFE_TYPE),
        paths: &["metadonnees/type"],
    },
    Column {
        name: "rna",
        codec: Codec::Text,
        paths: &["metadonnees/idAssoc", "IDASSOC"],
    },
    Column {
        name: "page",
        codec: Codec::Int,
        paths: &["metadonnees/page", "metadonnees/page#", "PAGES/@debut"],
    },
    Column {
        name: "theme1",
        codec: Codec::Int,
        paths: &[
            "metadonnees/themes/theme/@code",
            "metadonnees/themes/theme[]/theme/code",
            "THEMES/THEME/@code",
        ],
    },
    Column {
        name: "theme1_label",
        codec: Codec::Dict(dict::JOAFE_THEME),
        paths: &[
            "metadonnees/themes/theme",
            "metadonnees/themes/theme[]/theme/Libelle",
        ],
    },
    Column {
        name: "theme2",
        codec: Codec::Int,
        paths: &[
            "metadonnees/themes/theme/@code",
            "metadonnees/themes/theme[]/theme/code",
            "THEMES/THEME/@code",
        ],
    },
    Column {
        name: "theme2_label",
        codec: Codec::Dict(dict::JOAFE_THEME),
        paths: &[
            "metadonnees/themes/theme",
            "metadonnees/themes/theme[]/theme/Libelle",
        ],
    },
    Column {
        name: "theme3",
        codec: Codec::Int,
        paths: &[
            "metadonnees/themes/theme/@code",
            "metadonnees/themes/theme[]/theme/code",
            "THEMES/THEME/@code",
        ],
    },
    Column {
        name: "theme3_label",
        codec: Codec::Dict(dict::JOAFE_THEME),
        paths: &[
            "metadonnees/themes/theme",
            "metadonnees/themes/theme[]/theme/Libelle",
        ],
    },
    Column {
        name: "lieu",
        codec: Codec::Dict(dict::JOAFE_LIEU),
        paths: &[
            "contenu/*/lieuDeclaration/lieuSousPref",
            "contenu/*/lieuDeclaration/lieuPref",
            "contenu/*/lieuDeclaration/lieuSousPref/libelle",
            "contenu/*/lieuDeclaration/lieuPref/libelle",
            "LIEU_DECLARATION",
        ],
    },
    Column {
        name: "date_declaration",
        codec: Codec::Date,
        paths: &["contenu/*/dateDeclaration", "@datedeclaration"],
    },
    Column {
        name: "titre",
        codec: Codec::Text,
        paths: &["contenu/*/*/titre", "TITRE"],
    },
    Column {
        name: "objet",
        codec: Codec::Text,
        paths: &[
            "contenu/*/*/objet",
            "contenu/*/*/nouvelObjet",
            "OBJET",
            "NOUVEL_OBJET",
        ],
    },
    Column {
        name: "siege",
        codec: Codec::Address,
        paths: &["contenu/*/*/siegeSocial", "SIEGE_SOCIAL"],
    },
    Column {
        name: "siege_new",
        codec: Codec::Address,
        paths: &[
            "contenu/*/*/transfereNouvelleAdresse",
            "TRANSFERE_NOUVELLE_ADRESSE",
        ],
    },
    Column {
        name: "cp",
        codec: Codec::Text,
        paths: &["@cp"],
    },
    Column {
        name: "parution_ref",
        codec: Codec::Int,
        paths: &["PARUTION_JO_ASSOCIATION/@numparution"],
    },
    Column {
        name: "date_ref",
        codec: Codec::Date,
        paths: &["PARUTION_JO_ASSOCIATION/@dateparution"],
    },
    Column {
        name: "fichier",
        codec: Codec::Text,
        paths: &["fichierJoint", "FICHIERS_JOINTS/FICHIER_HTML"],
    },
    Column {
        name: "old_title",
        codec: Codec::Text,
        paths: &["contenu/*/modification/ancienTitre", "ANCIEN_TITRE"],
    },
    Column {
        name: "new_title",
        codec: Codec::Text,
        paths: &["contenu/*/modification/nouveauTitre", "NOUVEAU_TITRE"],
    },
    Column {
        name: "email",
        codec: Codec::Text,
        paths: &["INTERNET/@smtp"],
    },
    Column {
        name: "website",
        codec: Codec::Text,
        paths: &[
            "INTERNET/@http",
            "contenu/*/creation/siteWeb",
            "contenu/*/modification/siteWeb",
        ],
    },
]);

pub static HEADER: Spec = Spec::new(&[]);

/// What a notice is about, from its content element or else its type code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Autre = 0,
    Creation = 1,
    Modification = 2,
    Dissolution = 3,
    Suspension = 4,
    Saisine = 5,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::Creation,
        Action::Modification,
        Action::Dissolution,
        Action::Suspension,
        Action::Saisine,
        Action::Autre,
    ];

    pub fn from_code(code: i16) -> Action {
        match code {
            1 => Action::Creation,
            2 => Action::Modification,
            3 => Action::Dissolution,
            4 => Action::Suspension,
            5 => Action::Saisine,
            _ => Action::Autre,
        }
    }

    /// The type codes of each action, as the official site files them.
    pub fn of_type(code: u16) -> Action {
        match code {
            1 | 4 | 7 | 11 | 44 | 101..=103 | 111 | 444 | 900..=902 | 1000 | 1001 | 2000 | 2001 => {
                Action::Creation
            }
            2 | 5 | 22 | 55 | 201..=203 | 222 | 555 | 903..=905 | 1003 | 1004 | 2003 => {
                Action::Modification
            }
            3 | 6 | 33 | 66 | 301..=303 | 333 | 906 | 907 | 1006 | 2006 => Action::Dissolution,
            909..=911 => Action::Suspension,
            2009 | 2010 => Action::Saisine,
            _ => Action::Autre,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Action::Autre => "autre",
            Action::Creation => "creation",
            Action::Modification => "modification",
            Action::Dissolution => "dissolution",
            Action::Suspension => "suspension",
            Action::Saisine => "saisine",
        }
    }
}

/// A parution, or for the 1997-2003 files a single notice, then `header` is `None`.
pub struct Doc {
    pub parution: u32,
    /// Days since 1970-01-01.
    pub date: i32,
    pub header: Option<Record>,
    pub notices: Vec<Notice>,
}

pub struct Notice {
    pub id: i64,
    pub action: Action,
    pub rectificatif: bool,
    pub record: Record,
}

fn parution_of(text: &str) -> Result<u32, Error> {
    text.parse()
        .ok()
        .filter(|p: &u32| (10_000_000..100_000_000).contains(p))
        .ok_or_else(|| Error::new(format!("parution {text} is not yyyynnnn")))
}

fn date_of(text: Option<&str>) -> Result<i32, Error> {
    text.and_then(|t| parse_any_date(t).map(|(d, _)| d))
        .ok_or_else(|| Error::new("no valid parution date"))
}

fn action(notice: &Node) -> Action {
    let code = notice
        .child("metadonnees")
        .and_then(|m| m.child("type"))
        .or_else(|| notice.child("TYPE"))
        .and_then(|t| t.attr("code"))
        .map(str::trim);
    let element = notice
        .child("contenu")
        .and_then(|c| c.children().first())
        .and_then(|t| {
            t.children().iter().find(|c| {
                !matches!(
                    c.name.as_str(),
                    "lieuDeclaration" | "dateDeclaration" | "dateDelivrance"
                )
            })
        })
        .map(|c| c.name.as_str());
    match element {
        Some("creation") => Action::Creation,
        Some("modification") => Action::Modification,
        Some("dissolution") => Action::Dissolution,
        Some("suspension") => Action::Suspension,
        _ => code
            .and_then(|c| c.parse().ok())
            .map_or(Action::Autre, Action::of_type),
    }
}

fn notice(item: &Node, parution: u32) -> Result<Notice, Error> {
    let record = decompose(item, &SPEC)?;
    let num = record
        .int(SPEC.column("num"))
        .filter(|n| *n < 10_000)
        .ok_or_else(|| {
            Error::new(format!(
                "notice without a usable number in parution {parution}"
            ))
        })?;
    let id = i64::from(parution) * 10_000 + num;
    if let Some(stated) = record.int(SPEC.column("id")) {
        if stated != id {
            return Err(Error::new(format!(
                "notice {stated} numbered {num} in parution {parution}"
            )));
        }
    }
    Ok(Notice {
        id,
        action: action(item),
        rectificatif: item.name == "rectificatif",
        record,
    })
}

fn take_children(node: &mut Node, names: &[&str]) -> Vec<Node> {
    let Body::Children(children) = &mut node.body else {
        return Vec::new();
    };
    let (taken, kept): (Vec<Node>, Vec<Node>) = std::mem::take(children)
        .into_iter()
        .partition(|c| names.contains(&c.name.as_str()));
    node.body = if kept.is_empty() {
        Body::Empty
    } else {
        Body::Children(kept)
    };
    taken
}

/// The notices of a `listeAnnonces`, repeated in XML, in arrays in JSON.
const NOTICES: &[&str] = &[
    "annonce",
    "rectificatif",
    "annulation",
    "annonce[]",
    "rectificatif[]",
    "annulation[]",
];

/// The root with its notices taken out, and the notices.
pub fn split(mut root: Node) -> Result<(Node, Vec<Node>), Error> {
    let items = match root.name.as_str() {
        "parution" => {
            let Body::Children(children) = &mut root.body else {
                return Err(Error::new("empty <parution>"));
            };
            match children.iter_mut().find(|c| c.name == "listeAnnonces") {
                Some(list) => {
                    let mut items = Vec::new();
                    for taken in take_children(list, NOTICES) {
                        match taken.body {
                            Body::Children(listed) if taken.name.ends_with("[]") => {
                                items.extend(listed)
                            }
                            Body::Empty if taken.name.ends_with("[]") => {}
                            _ => items.push(taken),
                        }
                    }
                    if let Some(other) = list.children().first() {
                        return Err(Error::new(format!(
                            "<{}> in <listeAnnonces> is no notice",
                            other.name
                        )));
                    }
                    items
                }
                None => Vec::new(),
            }
        }
        "PARUTION_JO_ASSOCIATION" => take_children(&mut root, &["ANNONCE_REF"]),
        "ANNONCE_REF" => {
            let notice = std::mem::replace(
                &mut root,
                Node {
                    name: String::new(),
                    attrs: Vec::new(),
                    body: Body::Empty,
                },
            );
            vec![notice]
        }
        other => return Err(Error::new(format!("unknown JOAFE root <{other}>"))),
    };
    Ok((root, items))
}

pub fn parse(root: Node) -> Result<Doc, Error> {
    let (parution, date) = match root.name.as_str() {
        "parution" => (
            parution_of(
                root.child_text("numParution")
                    .ok_or_else(|| Error::new("no <numParution>"))?,
            )?,
            date_of(root.child_text("dateParution"))?,
        ),
        "PARUTION_JO_ASSOCIATION" => (
            parution_of(
                root.attr("numparution")
                    .ok_or_else(|| Error::new("no @numparution"))?,
            )?,
            date_of(root.attr("dateparution"))?,
        ),
        "ANNONCE_REF" => {
            let p = root
                .child("PARUTION_JO_ASSOCIATION")
                .ok_or_else(|| Error::new("no <PARUTION_JO_ASSOCIATION> in <ANNONCE_REF>"))?;
            (
                parution_of(
                    p.attr("numparution")
                        .ok_or_else(|| Error::new("no @numparution"))?,
                )?,
                date_of(p.attr("dateparution"))?,
            )
        }
        other => return Err(Error::new(format!("unknown JOAFE root <{other}>"))),
    };
    let unitary = root.name == "ANNONCE_REF";
    let (header, items) = split(root)?;
    let header = if unitary {
        None
    } else {
        Some(decompose(&header, &HEADER)?)
    };
    let notices = items
        .iter()
        .map(|item| notice(item, parution))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Doc {
        parution,
        date,
        header,
        notices,
    })
}
