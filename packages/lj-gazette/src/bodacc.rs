//! BODACC parution files: one per publication day and kind, a header and the notices.

use crate::record::{decompose, parse_any_date, Codec, Column, Record, Spec};
use crate::tree::{Body, Node};
use crate::{dict, Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pcl = 1,
    RcsA = 2,
    RcsB = 3,
    Bilan = 4,
    Div = 5,
}

impl Kind {
    pub fn from_root(name: &str) -> Option<Kind> {
        Some(match name {
            "PCL_REDIFF" => Kind::Pcl,
            "RCS-A_IMMAT" => Kind::RcsA,
            "RCS-B_REDIFF" => Kind::RcsB,
            "Bilan_XML_Rediff" => Kind::Bilan,
            "Divers_XML_Rediff" => Kind::Div,
            _ => return None,
        })
    }

    /// 1, 2, 3 for BODACC A, B, C.
    pub fn publication(self) -> i64 {
        match self {
            Kind::Pcl | Kind::RcsA | Kind::Div => 1,
            Kind::RcsB => 2,
            Kind::Bilan => 3,
        }
    }
}

/// The Opendatasoft `familleavis` codes, which the pages already label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Famille {
    Inconnue = 0,
    Collective = 1,
    Conciliation = 2,
    RetablissementProfessionnel = 3,
    Creation = 4,
    Immatriculation = 5,
    Vente = 6,
    Modification = 7,
    Radiation = 8,
    Dpc = 9,
    Divers = 10,
}

impl Famille {
    pub const ALL: [Famille; 11] = [
        Famille::Collective,
        Famille::Conciliation,
        Famille::RetablissementProfessionnel,
        Famille::Creation,
        Famille::Immatriculation,
        Famille::Vente,
        Famille::Modification,
        Famille::Radiation,
        Famille::Dpc,
        Famille::Divers,
        Famille::Inconnue,
    ];

    pub fn from_code(code: i16) -> Famille {
        match code {
            1 => Famille::Collective,
            2 => Famille::Conciliation,
            3 => Famille::RetablissementProfessionnel,
            4 => Famille::Creation,
            5 => Famille::Immatriculation,
            6 => Famille::Vente,
            7 => Famille::Modification,
            8 => Famille::Radiation,
            9 => Famille::Dpc,
            10 => Famille::Divers,
            _ => Famille::Inconnue,
        }
    }

    pub fn ods_code(self) -> &'static str {
        match self {
            Famille::Inconnue => "inconnue",
            Famille::Collective => "collective",
            Famille::Conciliation => "conciliation",
            Famille::RetablissementProfessionnel => "retablissement_professionnel",
            Famille::Creation => "creation",
            Famille::Immatriculation => "immatriculation",
            Famille::Vente => "vente",
            Famille::Modification => "modification",
            Famille::Radiation => "radiation",
            Famille::Dpc => "dpc",
            Famille::Divers => "divers",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avis {
    Inconnu = 0,
    Initial = 1,
    Rectificatif = 2,
    Annulation = 3,
}

impl Avis {
    pub fn from_code(code: i16) -> Avis {
        match code {
            1 => Avis::Initial,
            2 => Avis::Rectificatif,
            3 => Avis::Annulation,
            _ => Avis::Inconnu,
        }
    }
}

/// Column ids are written into stored shapes: append only.
pub static SPEC: Spec = Spec::new(&[
    Column { name: "num", codec: Codec::Int, paths: &["numeroAnnonce"] },
    Column { name: "dept", codec: Codec::Dict(dict::DEPT), paths: &["numeroDepartement"] },
    Column { name: "tribunal", codec: Codec::Dict(dict::TRIBUNAL), paths: &["tribunal"] },
    Column {
        name: "siren",
        codec: Codec::Siren,
        paths: &[
            "numeroImmatriculation/numeroIdentificationRCS",
            "numeroImmatriculation/numeroIdentification",
            "personnes/personne/personneMorale/numeroImmatriculation/numeroIdentification",
            "personnes/personne/personnePhysique/numeroImmatriculation/numeroIdentification",
            "personnes/personne/numeroImmatriculation/numeroIdentificationRCS",
            "personnes/personne/numeroImmatriculation/numeroIdentification",
            "inscriptionRM/numeroIdentificationRM",
        ],
    },
    Column {
        name: "greffe",
        codec: Codec::Dict(dict::GREFFE),
        paths: &[
            "numeroImmatriculation/nomGreffeImmat",
            "personnes/personne/personneMorale/numeroImmatriculation/nomGreffeImmat",
            "personnes/personne/personnePhysique/numeroImmatriculation/nomGreffeImmat",
            "personnes/personne/numeroImmatriculation/nomGreffeImmat",
        ],
    },
    Column {
        name: "name",
        codec: Codec::Text,
        paths: &[
            "personneMorale/denomination",
            "denomination",
            "personnePhysique/nom",
            "personnes/personne/personneMorale/denomination",
            "personnes/personne/personnePhysique/nom",
        ],
    },
    Column {
        name: "first_name",
        codec: Codec::Text,
        paths: &["personnePhysique/prenom", "personnes/personne/personnePhysique/prenom"],
    },
    Column {
        name: "forme",
        codec: Codec::Dict(dict::FORME),
        paths: &["personneMorale/formeJuridique", "formeJuridique", "personnes/personne/personneMorale/formeJuridique"],
    },
    Column {
        name: "activity",
        codec: Codec::Text,
        paths: &["activite", "etablissement/activite", "personnes/personne/activite"],
    },
    Column {
        name: "administration",
        codec: Codec::Text,
        paths: &["personnes/personne/personneMorale/administration"],
    },
    Column {
        name: "address",
        codec: Codec::Address,
        paths: &["adresse", "personnes/personne/adresse", "personnes/personne/siegeSocial"],
    },
    Column {
        name: "address_etab",
        codec: Codec::Address,
        paths: &["etablissement/adresse", "personnes/personne/etablissementPrincipal"],
    },
    Column { name: "jug_famille", codec: Codec::Dict(dict::JUG_FAMILLE), paths: &["jugement/famille"] },
    Column { name: "jug_nature", codec: Codec::Dict(dict::JUG_NATURE), paths: &["jugement/nature"] },
    Column { name: "jug_date", codec: Codec::Date, paths: &["jugement/date"] },
    Column { name: "jug_complement", codec: Codec::Text, paths: &["jugement/complementJugement"] },
    Column {
        name: "category",
        codec: Codec::Dict(dict::CATEGORY),
        paths: &[
            "acte/creation/categorieCreation",
            "acte/immatriculation/categorieImmatriculation",
            "acte/vente/categorieVente",
        ],
    },
    Column { name: "origin", codec: Codec::Dict(dict::ORIGIN), paths: &["etablissement/origineFonds"] },
    Column { name: "qualite", codec: Codec::Dict(dict::QUALITE), paths: &["etablissement/qualiteEtablissement"] },
    Column { name: "modif", codec: Codec::Dict(dict::MODIF), paths: &["modificationsGenerales/descriptif"] },
    Column { name: "radiation", codec: Codec::Dict(dict::RADIATION), paths: &["radiationAuRCS/commentaire"] },
    Column { name: "depot_type", codec: Codec::Dict(dict::DEPOT_TYPE), paths: &["depot/typeDepot"] },
    Column { name: "depot_close", codec: Codec::Date, paths: &["depot/dateCloture"] },
    Column { name: "depot_desc", codec: Codec::Dict(dict::DEPOT_DESC), paths: &["depot/descriptif"] },
    Column {
        name: "code_rcs",
        codec: Codec::Dict(dict::CODE_RCS),
        paths: &[
            "numeroImmatriculation/codeRCS",
            "personnes/personne/numeroImmatriculation/codeRCS",
            "personnes/personne/personneMorale/numeroImmatriculation/codeRCS",
            "personnes/personne/personnePhysique/numeroImmatriculation/codeRCS",
        ],
    },
    Column {
        name: "commercial_name",
        codec: Codec::Text,
        paths: &[
            "nomCommercial",
            "personnePhysique/nomCommercial",
            "personneMorale/nomCommercial",
            "personnes/personne/personnePhysique/nomCommercial",
            "personnes/personne/personneMorale/nomCommercial",
        ],
    },
    Column {
        name: "sigle",
        codec: Codec::Text,
        paths: &["sigle", "personneMorale/sigle", "personnes/personne/personneMorale/sigle"],
    },
    Column {
        name: "usage_name",
        codec: Codec::Text,
        paths: &["personnePhysique/nomUsage", "personnes/personne/personnePhysique/nomUsage"],
    },
    Column { name: "enseigne", codec: Codec::Text, paths: &["etablissement/enseigne"] },
    Column {
        name: "capital",
        codec: Codec::Text,
        paths: &[
            "capital/montantCapital",
            "personnes/personne/capital/montantCapital",
            "personnes/personne/personneMorale/capital/montantCapital",
        ],
    },
    Column {
        name: "devise",
        codec: Codec::Dict(dict::DEVISE),
        paths: &["capital/devise", "personnes/personne/capital/devise", "personnes/personne/personneMorale/capital/devise"],
    },
    Column {
        name: "descriptif",
        codec: Codec::Text,
        paths: &["acte/vente/descriptif", "acte/creation/descriptif", "acte/immatriculation/descriptif"],
    },
    Column {
        name: "date_activite",
        codec: Codec::Date,
        paths: &[
            "acte/creation/dateCommencementActivite",
            "acte/immatriculation/dateCommencementActivite",
            "acte/vente/dateCommencementActivite",
            "modificationsGenerales/dateCommencementActivite",
        ],
    },
    Column {
        name: "date_immat",
        codec: Codec::Date,
        paths: &[
            "acte/creation/dateImmatriculation",
            "acte/immatriculation/dateImmatriculation",
            "acte/vente/dateImmatriculation",
        ],
    },
    Column {
        name: "date_effet",
        codec: Codec::Date,
        paths: &["modificationsGenerales/dateEffet", "acte/immatriculation/dateEffet", "acte/vente/dateEffet"],
    },
    Column {
        name: "date_cessation",
        codec: Codec::Date,
        paths: &[
            "radiationAuRCS/radiationPP/dateCessationActivitePP",
            "radiationAuRCS/radiationPM/dateCessationActivitePM",
        ],
    },
    Column {
        name: "previous_owner",
        codec: Codec::Text,
        paths: &[
            "precedentProprietairePM/denomination",
            "precedentProprietairePP/nom",
            "precedentExploitantPM/denomination",
            "precedentExploitantPP/nom",
            "modificationsGenerales/precedentExploitantPM/denomination",
            "modificationsGenerales/precedentExploitantPP/nom",
        ],
    },
    Column {
        name: "previous_siren",
        codec: Codec::Siren,
        paths: &[
            "precedentProprietairePM/numeroImmatriculation/numeroIdentification",
            "precedentProprietairePP/numeroImmatriculation/numeroIdentification",
            "precedentExploitantPM/numeroImmatriculation/numeroIdentification",
            "precedentExploitantPP/numeroImmatriculation/numeroIdentification",
            "modificationsGenerales/precedentExploitantPM/numeroImmatriculation/numeroIdentification",
            "modificationsGenerales/precedentExploitantPP/numeroImmatriculation/numeroIdentification",
        ],
    },
]);

pub static HEADER: Spec = Spec::new(&[]);

pub struct File {
    pub kind: Kind,
    pub parution: u32,
    /// Days since 1970-01-01.
    pub date: i32,
    pub header: Record,
    pub notices: Vec<Notice>,
}

pub struct Notice {
    pub id: i64,
    pub famille: Famille,
    pub avis: Avis,
    pub record: Record,
}

pub fn notice_id(kind: Kind, parution: u32, number: i64) -> i64 {
    kind.publication() * 100_000_000_000_000 + i64::from(parution) * 1_000_000 + number
}

/// The notice id a BODACC reference names: its publication letter, its parution, its number
/// (`A20260100042`).
pub fn parse_reference(reference: &str) -> Option<i64> {
    let publication = match reference.get(..1)? {
        "A" => 1,
        "B" => 2,
        "C" => 3,
        _ => return None,
    };
    if !reference[1..].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let parution: i64 = reference.get(1..9)?.parse().ok()?;
    let number: i64 = reference.get(9..)?.parse().ok()?;
    (number < 1_000_000)
        .then_some(publication * 100_000_000_000_000 + parution * 1_000_000 + number)
}

/// `(publication letter, parution, number)` of a notice id.
pub fn split_id(id: i64) -> (char, u32, u32) {
    let publication = match id / 100_000_000_000_000 {
        1 => 'A',
        2 => 'B',
        _ => 'C',
    };
    (
        publication,
        ((id / 1_000_000) % 100_000_000) as u32,
        (id % 1_000_000) as u32,
    )
}

fn famille(kind: Kind, notice: &Node) -> Famille {
    match kind {
        Kind::Pcl => {
            let j = notice.child("jugement");
            let text = [
                j.and_then(|j| j.child_text("famille")),
                j.and_then(|j| j.child_text("nature")),
            ]
            .into_iter()
            .flatten()
            .map(lj_fold::fold)
            .collect::<Vec<_>>()
            .join(" ");
            if text.contains("conciliation")
                || text.contains("homologation de l'accord")
                || text.contains("accord homologue")
            {
                Famille::Conciliation
            } else if text.contains("retablissement professionnel") {
                Famille::RetablissementProfessionnel
            } else {
                Famille::Collective
            }
        }
        Kind::RcsA => match notice
            .child("acte")
            .and_then(|a| a.children().first())
            .map(|c| c.name.as_str())
        {
            Some("creation") => Famille::Creation,
            Some("immatriculation") => Famille::Immatriculation,
            Some("vente") => Famille::Vente,
            _ => Famille::Inconnue,
        },
        Kind::RcsB => {
            if notice.child("radiationAuRCS").is_some() {
                Famille::Radiation
            } else if notice.child("modificationsGenerales").is_some() {
                Famille::Modification
            } else {
                Famille::Inconnue
            }
        }
        Kind::Bilan => Famille::Dpc,
        Kind::Div => Famille::Divers,
    }
}

fn avis(notice: &Node) -> Avis {
    match notice
        .child("typeAnnonce")
        .and_then(|t| t.children().first())
        .map(|c| c.name.as_str())
    {
        Some("annonce" | "creation") => Avis::Initial,
        Some("rectificatif") => Avis::Rectificatif,
        Some("annulation") => Avis::Annulation,
        _ => Avis::Inconnu,
    }
}

const LISTS: &[&str] = &["annonces", "listeAvis"];

/// A notice published without a number takes one counted down from 999 999, in document order.
const UNNUMBERED_FROM: i64 = 900_000;

/// The root with its notice list emptied, and the notices.
pub fn split(mut root: Node) -> Result<(Node, Vec<Node>), Error> {
    let Body::Children(children) = &mut root.body else {
        return Err(Error::new("empty root"));
    };
    let list = children
        .iter_mut()
        .find(|c| LISTS.contains(&c.name.as_str()))
        .ok_or_else(|| Error::new("no notice list"))?;
    let (items, stray): (Vec<Node>, Vec<Node>) =
        match std::mem::replace(&mut list.body, Body::Empty) {
            Body::Children(items) => items
                .into_iter()
                .partition(|n| n.name != crate::tree::STRAY_TEXT),
            Body::Empty => (Vec::new(), Vec::new()),
            Body::Text(_) => return Err(Error::new("notice list holds text")),
        };
    if !stray.is_empty() {
        list.body = Body::Children(stray);
    }
    Ok((root, items))
}

pub fn parse_file(root: Node) -> Result<File, Error> {
    let kind = Kind::from_root(&root.name)
        .ok_or_else(|| Error::new(format!("unknown BODACC root <{}>", root.name)))?;
    let parution_text = root
        .child_text("parution")
        .ok_or_else(|| Error::new("no <parution>"))?;
    let parution: u32 = parution_text
        .parse()
        .ok()
        .filter(|p: &u32| (10_000_000..100_000_000).contains(p) && p.to_string() == parution_text)
        .ok_or_else(|| Error::new(format!("parution {parution_text} is not yyyynnnn")))?;
    let date = root
        .child_text("dateParution")
        .and_then(|d| parse_any_date(d).map(|(d, _)| d))
        .ok_or_else(|| Error::new("no valid <dateParution>"))?;
    let (root, items) = split(root)?;
    let header = decompose(&root, &HEADER)?;
    let num_col = SPEC.column("num");
    let mut notices = Vec::with_capacity(items.len());
    let mut unnumbered = 0;
    for item in &items {
        let record = decompose(item, &SPEC)?;
        let number = match record.int(num_col) {
            Some(n) if n < UNNUMBERED_FROM => n,
            None if item.child_text("numeroAnnonce").is_none() => {
                unnumbered += 1;
                1_000_000 - unnumbered
            }
            _ => {
                return Err(Error::new(format!(
                    "notice without a usable number in parution {parution}"
                )))
            }
        };
        notices.push(Notice {
            id: notice_id(kind, parution, number),
            famille: famille(kind, item),
            avis: avis(item),
            record,
        });
    }
    Ok(File {
        kind,
        parution,
        date,
        header,
        notices,
    })
}
