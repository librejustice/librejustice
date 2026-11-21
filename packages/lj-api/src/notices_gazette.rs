//! A BODACC or JOAFE notice read back from its stored rows, each printed field under its label.

use std::collections::HashMap;

use lj_dtos::{LinkedTextRef, NoticeField, PublicationNotice, TextPublication};
use lj_gazette::bodacc::{self, Avis, Famille};
use lj_gazette::joafe::{self, Action};
use lj_gazette::record::{format_date, recompose, Cell, Record};
use lj_gazette::rows::{Layout, Row, Value, ADDRESS};
use lj_gazette::tree::{Body, Node};
use lj_gazette::Source;
use lj_store::repository::DecisionRepository;

use crate::error::{ApiError, Result};
use crate::state::AppState;

/// A BODACC notice by its reference, `A20260100042`.
pub(crate) async fn bodacc(state: &AppState, reference: &str) -> Result<PublicationNotice> {
    let id = bodacc::parse_reference(reference).ok_or(ApiError::NotFound)?;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let (row, record) = load(&repo, Source::Bodacc, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let node = recompose(&record).map_err(stored)?;
    let layout = Source::Bodacc.layout();
    let spec = &bodacc::SPEC;
    let (letter, parution, number) = bodacc::split_id(id);
    let famille =
        crate::journals_bodacc::famille_label(Famille::from_code(small(layout, &row, "famille")));
    let nature = match Avis::from_code(small(layout, &row, "avis")) {
        Avis::Rectificatif => format!("{famille}, avis rectificatif"),
        Avis::Annulation => format!("{famille}, avis d'annulation"),
        Avis::Initial | Avis::Inconnu => famille.to_string(),
    };
    let name = match (
        record.text(spec.column("name")),
        record.text(spec.column("first_name")),
    ) {
        (Some(n), Some(p)) => Some(format!("{n} {p}")),
        (n, _) => n.map(str::to_string),
    };
    let entity = match &record.cells[spec.column("siren")] {
        Some(Cell::Siren(siren)) => entity(&repo, "siren", &format!("{siren:09}")).await?,
        _ => None,
    };
    let journal = lj_norm::applicability::journal("BODACC").expect("journals.tsv names the BODACC");
    Ok(PublicationNotice {
        uid: format!("bodacc:{letter}{parution}{number}"),
        jurisdiction: "FR".to_string(),
        journal: journal.code.to_string(),
        journal_label: Some(journal.label.to_string()),
        language: "fr".to_string(),
        other_languages: Vec::new(),
        title: name.unwrap_or_else(|| famille.to_string()),
        nature: Some(nature),
        nor: None,
        date_texte: None,
        date_publi: Some(date(layout, &row)),
        issuer: record.text(spec.column("tribunal")).map(str::to_string),
        source_url: Some(crate::journals_bodacc::official_url(
            letter, parution, number,
        )),
        publication: issue(format!("BODACC {letter} n° {parution}")),
        articles: Vec::new(),
        body: None,
        norm: None,
        fields: fields(&node),
        entity,
        // A notice names its trader, often a natural person at a home address.
        individual: true,
    })
}

/// A JOAFE notice by its twelve-digit id, its parution then its number.
pub(crate) async fn joafe(state: &AppState, reference: &str) -> Result<PublicationNotice> {
    let id: i64 = reference
        .parse()
        .ok()
        .filter(|_| reference.len() == 12)
        .ok_or(ApiError::NotFound)?;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let (row, record) = load(&repo, Source::Joafe, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let node = recompose(&record).map_err(stored)?;
    let layout = Source::Joafe.layout();
    let spec = &joafe::SPEC;
    let action =
        crate::journals_joafe::action_label(Action::from_code(small(layout, &row, "action")));
    let nature = match node.name.as_str() {
        "rectificatif" => format!("{action}, avis rectificatif"),
        "annulation" => format!("{action}, avis d'annulation"),
        _ => action.to_string(),
    };
    let fields = fields(&node);
    let entity = match record.text(spec.column("rna")) {
        Some(rna) => entity(&repo, "rna", rna).await?,
        None => None,
    };
    let journal = lj_norm::applicability::journal("JOAFE").expect("journals.tsv names the JOAFE");
    Ok(PublicationNotice {
        uid: format!("joafe:{reference}"),
        jurisdiction: "FR".to_string(),
        journal: journal.code.to_string(),
        journal_label: Some(journal.label.to_string()),
        language: "fr".to_string(),
        other_languages: Vec::new(),
        title: ["Titre", "Nouveau titre", "Ancien titre"]
            .iter()
            .find_map(|l| fields.iter().find(|f| f.label == *l))
            .map_or_else(|| action.to_string(), |f| f.value.clone()),
        nature: Some(nature),
        nor: None,
        date_texte: None,
        date_publi: Some(date(layout, &row)),
        issuer: None,
        source_url: crate::journals_joafe::detail_url(reference),
        publication: issue(format!("JOAFE n° {}", id / 10_000)),
        articles: Vec::new(),
        body: None,
        norm: None,
        fields,
        entity,
        // An association's notice names the persons who declared it.
        individual: true,
    })
}

/// A notice's row and its record, with only the addresses and dictionary entries it names.
async fn load(
    repo: &DecisionRepository<'_>,
    source: Source,
    id: i64,
) -> Result<Option<(Row, Record)>> {
    let layout = source.layout();
    let Some(row) = repo
        .gazette_rows(&layout.table, &[id])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let address_ids = layout.address_ids(&row);
    let address_rows = match address_ids.is_empty() {
        true => Vec::new(),
        false => repo.gazette_address_rows(&address_ids).await?,
    };
    let mut codes = layout.dictionary_codes(&row);
    codes.extend(
        address_rows
            .iter()
            .flat_map(|r| ADDRESS.dictionary_codes(r)),
    );
    let dict = repo.gazette_dict_entries(&codes).await?;
    let addresses = address_rows
        .iter()
        .map(|r| {
            let (id, rec) = ADDRESS.record(r, &dict, &HashMap::new())?;
            Ok((id as i32, rec))
        })
        .collect::<std::result::Result<HashMap<_, _>, lj_gazette::Error>>()
        .map_err(stored)?;
    let (_, record) = layout.record(&row, &dict, &addresses).map_err(stored)?;
    Ok(Some((row, record)))
}

fn stored(e: lj_gazette::Error) -> ApiError {
    ApiError::Internal(format!("stored notice: {e}"))
}

fn own<'r>(layout: &Layout, row: &'r Row, name: &str) -> &'r Value {
    let i = layout
        .table
        .columns
        .iter()
        .position(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no column {name} in {}", layout.table.name));
    &row[i]
}

fn small(layout: &Layout, row: &Row, name: &str) -> i16 {
    match own(layout, row, name) {
        Value::Small(v) => *v,
        other => panic!("{name}: {other:?} is not a smallint"),
    }
}

fn date(layout: &Layout, row: &Row) -> String {
    match own(layout, row, "date") {
        Value::Date(days) => format_date(*days),
        other => panic!("date: {other:?} is not a date"),
    }
}

fn issue(label: String) -> TextPublication {
    TextPublication {
        issue: Some(LinkedTextRef {
            label,
            href: None,
            date: None,
        }),
        ..TextPublication::default()
    }
}

/// The entity page `ns:id` names, when the base holds one.
async fn entity(
    repo: &DecisionRepository<'_>,
    ns: &str,
    id: &str,
) -> Result<Option<LinkedTextRef>> {
    Ok(repo
        .entity_header(&format!("{ns}:{id}"))
        .await?
        .map(|e| LinkedTextRef {
            label: e.denomination,
            href: Some(lj_vocab::entity_path(ns, id)),
            date: None,
        }))
}

/// Elements that only number or file the notice, or that its header already shows.
const UNPRINTED: &[&str] = &[
    "nojo",
    "identifiantClient",
    "typeAnnonce",
    "numeroAnnonce",
    "numeroDepartement",
    "tribunal",
    "numAnnonce",
    "identifiant",
    "dept",
    "type",
    "page",
    "fichierJoint",
    "TYPE",
    "IDENTIFIANT",
    "PAGES",
    "FICHIERS_JOINTS",
    "PARUTION_JO_ASSOCIATION",
    "sourceParution",
    "etatAnnonce",
    "typeAssociation",
    "dateParution",
    "numParution",
    // The address the association is managed from, often a person's home: shown nowhere.
    "coordonneesGestion",
    // The association's officers, natural persons: shown nowhere.
    "dirigeants",
];

/// An element's name without the mark a JSON delivery adds: `#` a number, `{}` an object, `[]` a list.
fn bare(name: &str) -> &str {
    name.strip_suffix('#')
        .or_else(|| name.strip_suffix("{}"))
        .or_else(|| name.strip_suffix("[]"))
        .unwrap_or(name)
}

/// The fields of a notice in printed order, each under the nearest part that names a group.
pub(crate) fn fields(notice: &Node) -> Vec<NoticeField> {
    let mut out = Vec::new();
    attributes(notice, None, &mut out);
    walk(notice, None, &mut out);
    out
}

fn attributes(node: &Node, group: Option<&str>, out: &mut Vec<NoticeField>) {
    for (attr, value) in node.attrs.iter().filter(|(_, v)| !v.trim().is_empty()) {
        if let Some(label) = attribute_label(&node.name, attr) {
            out.push(NoticeField {
                group: group.map(str::to_string),
                label: label.to_string(),
                value: printed(attr, value),
                href: None,
            });
        }
    }
}

fn walk(node: &Node, group: Option<&str>, out: &mut Vec<NoticeField>) {
    for child in node.children() {
        let name = bare(&child.name);
        if UNPRINTED.contains(&name) {
            continue;
        }
        let push = |out: &mut Vec<NoticeField>, label: &str, value: String, href| {
            out.push(NoticeField {
                group: group.map(str::to_string),
                label: label.to_string(),
                value,
                href,
            })
        };
        if let Some((label, value, href)) = joined(child) {
            if !value.is_empty() {
                push(out, label, value, href);
            }
            continue;
        }
        attributes(child, group, out);
        match &child.body {
            Body::Empty => {}
            Body::Text(text) => push(out, &label(name), printed(name, text), None),
            Body::Children(_) => walk(child, group_label(name).or(group), out),
        }
    }
}

/// The value as the register printed it, a date written the French way.
fn printed(name: &str, text: &str) -> String {
    let text = text.trim();
    match (name, text) {
        ("radiationPM", "O") => "Oui".to_string(),
        _ => french_date(text).unwrap_or_else(|| text.to_string()),
    }
}

fn french_date(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let iso = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    iso.then(|| format!("{}/{}/{}", &text[8..], &text[5..7], &text[..4]))
}

/// The leaf texts under a node, in order.
fn leaves<'n>(node: &'n Node, out: &mut Vec<(&'n str, &'n str)>) {
    match &node.body {
        Body::Text(t) if !t.trim().is_empty() => out.push((bare(&node.name), t.trim())),
        Body::Children(children) => children.iter().for_each(|c| leaves(c, out)),
        _ => {}
    }
}

const ADDRESS_PARTS: &[&str] = &[
    "adresse",
    "unstructured_address",
    "numeroVoie",
    "extensionVoie",
    "typeVoie",
    "nomVoie",
    "immeubleBatimentResidence",
    "etageEscalierAppartement",
    "complGeographique",
    "BP",
    "bp",
    "BP_ou_Lieudit_ou_CommununeDeleguee",
    "codePostal",
    "ville",
    "commune",
    "localite",
    "pays",
];

/// An address on one line: the street, its complement, the post box, the town, the country.
fn address(parts: &[(&str, &str)]) -> Option<String> {
    if parts.is_empty() || !parts.iter().all(|(n, _)| ADDRESS_PARTS.contains(n)) {
        return None;
    }
    let get = |names: &[&str]| -> Option<String> {
        let words: Vec<&str> = parts
            .iter()
            .filter(|(n, _)| names.contains(n))
            .map(|(_, v)| *v)
            .collect();
        (!words.is_empty()).then(|| words.join(" "))
    };
    let lines: Vec<String> = [
        get(&["adresse", "unstructured_address"]),
        get(&["numeroVoie", "extensionVoie", "typeVoie", "nomVoie"]),
        get(&["immeubleBatimentResidence"]),
        get(&["etageEscalierAppartement"]),
        get(&["complGeographique"]),
        get(&["BP", "bp", "BP_ou_Lieudit_ou_CommununeDeleguee"]),
        get(&["codePostal", "ville", "commune"]),
        get(&["localite"]),
        get(&["pays"]),
    ]
    .into_iter()
    .flatten()
    .map(|l| l.trim_end_matches([',', ' ']).to_string())
    .collect();
    Some(lines.join(", "))
}

/// A part printed as one value: an address, a registration, a capital, a previous notice.
fn joined(node: &Node) -> Option<(&'static str, String, Option<String>)> {
    if !matches!(node.body, Body::Children(_)) {
        return None;
    }
    let mut parts = Vec::new();
    leaves(node, &mut parts);
    let part = |name: &str| parts.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
    let spaced = || parts.iter().map(|(_, v)| *v).collect::<Vec<_>>().join(" ");
    let named = |names: &[&str]| -> Vec<&str> {
        parts
            .iter()
            .filter(|(n, _)| names.contains(n))
            .map(|(_, v)| *v)
            .collect()
    };
    let label = match bare(&node.name) {
        "numeroImmatriculation" => return Some(("Immatriculation", spaced(), None)),
        "inscriptionRM" => {
            return Some(("Inscription au répertoire des métiers", spaced(), None));
        }
        "capital" => return Some(("Capital", spaced(), None)),
        "lieuDeclaration" => {
            let value = match named(&["libelle"])[..] {
                [] => spaced(),
                ref libelle => libelle.join(" "),
            };
            return Some(("Lieu de déclaration", value, None));
        }
        "themes" | "THEMES" => {
            let themes = named(&["theme", "THEME", "Libelle"]);
            return Some(("Thèmes", themes.join(" ; "), None));
        }
        "journal" => {
            let value = match (part("titre"), part("date")) {
                (Some(t), Some(d)) => format!("{t}, {}", printed("date", d)),
                (t, d) => t.or(d).unwrap_or_default().to_string(),
            };
            return Some(("Journal d'annonces légales", value, None));
        }
        "parutionAvisPrecedent" => {
            let publication = part("nomPublication").unwrap_or("BODACC");
            let parution = part("numeroParution").unwrap_or_default();
            let number = part("numeroAnnonce").unwrap_or_default();
            let mut value = format!("{publication} n° {parution}");
            if let Some(d) = part("dateParution") {
                value.push_str(&format!(" du {}", printed("date", d)));
            }
            value.push_str(&format!(", annonce n° {number}"));
            let href = publication
                .strip_prefix("BODACC ")
                .map(|letter| format!("{letter}{parution}{number}"))
                .filter(|r| bodacc::parse_reference(r).is_some())
                .map(|r| lj_vocab::notice_path("FR", &format!("bodacc:{r}")));
            return Some(("Avis précédent", value, href));
        }
        "refAvisInitial" => {
            let printed_parution = part("numParution").unwrap_or_default();
            let parution = joafe_parution(printed_parution, part("dateParution"));
            let number = part("numAnnonce").unwrap_or_default();
            let mut value = match parution {
                Some(p) => format!("JOAFE n° {p}"),
                None => format!("JOAFE n° {printed_parution}"),
            };
            if let Some(d) = part("dateParution") {
                value.push_str(&format!(" du {}", printed("date", d)));
            }
            value.push_str(&format!(", annonce n° {number}"));
            let href = match (parution, number.parse::<i64>()) {
                (Some(p), Ok(n)) if n < 10_000 => Some(lj_vocab::notice_path(
                    "FR",
                    &format!("joafe:{:012}", p * 10_000 + n),
                )),
                _ => None,
            };
            return Some(("Avis initial", value, href));
        }
        "adresse" | "france" | "etranger" => "Adresse",
        "siegeSocial" | "SIEGE_SOCIAL" => "Siège social",
        "etablissementPrincipal" => "Établissement principal",
        "ancienSiegeSocial" => "Ancien siège social",
        "transfereNouvelleAdresse" => "Nouvelle adresse",
        _ => return None,
    };
    Some((label, address(&parts)?, None))
}

/// A JOAFE parution `yyyynnnn`, printed whole or as its number in the year of its date.
fn joafe_parution(printed: &str, date: Option<&str>) -> Option<i64> {
    let n: i64 = printed.parse().ok()?;
    match printed.len() {
        8 => Some(n),
        1..=4 => Some(date?.get(..4)?.parse::<i64>().ok()? * 10_000 + n),
        _ => None,
    }
}

/// The heading of a part that groups its fields; the others pass their fields to their parent's.
fn group_label(name: &str) -> Option<&'static str> {
    Some(match name {
        "personne" => "Personne",
        "etablissement" => "Établissement",
        "creation" => "Création",
        "immatriculation" => "Immatriculation",
        "vente" => "Vente",
        "modification" => "Modification",
        "dissolution" => "Dissolution",
        "suspension" => "Suspension",
        "jugement" => "Jugement",
        "jugementAnnule" => "Jugement annulé",
        "precedentProprietairePM" | "precedentProprietairePP" => "Précédent propriétaire",
        "precedentExploitantPM" | "precedentExploitantPP" => "Précédent exploitant",
        "modificationsGenerales" => "Modifications",
        "radiationAuRCS" => "Radiation",
        "depot" => "Dépôt des comptes",
        "autorisationAdmin" => "Autorisation administrative",
        "assoLoi1901" => "Association loi de 1901",
        "assoSyndicale" => "Association syndicale de propriétaires",
        "fondDotation" => "Fonds de dotation",
        "fondationEntreprise" => "Fondation d'entreprise",
        "fondationPartenariale" => "Fondation partenariale",
        "fondPerennite" => "Fonds de pérennité",
        _ => return None,
    })
}

fn attribute_label(element: &str, attr: &str) -> Option<&'static str> {
    match (element, attr) {
        ("INTERNET", "http") => Some("Site web"),
        ("INTERNET", "smtp") => Some("Courriel"),
        ("ANNONCE_REF", "datedeclaration") => Some("Date de déclaration"),
        _ => None,
    }
}

fn label(name: &str) -> String {
    let known = match name {
        "denomination" => "Dénomination",
        "denominationEIRL" => "Dénomination de l'EIRL",
        "formeJuridique" => "Forme juridique",
        "administration" => "Administration",
        "nom" => "Nom",
        "prenom" => "Prénom",
        "nomUsage" => "Nom d'usage",
        "pseudonyme" => "Pseudonyme",
        "nomCommercial" => "Nom commercial",
        "nationnalite" => "Nationalité",
        "sigle" => "Sigle",
        "enseigne" => "Enseigne",
        "activite" => "Activité",
        "origineFonds" => "Origine du fonds",
        "qualiteEtablissement" => "Qualité de l'établissement",
        "categorieCreation" | "categorieImmatriculation" | "categorieVente" => "Catégorie",
        "dateCommencementActivite" => "Début d'activité",
        "dateImmatriculation" => "Date d'immatriculation",
        "dateEffet" => "Date d'effet",
        "dateCessationActivitePP" => "Cessation d'activité",
        "descriptif" => "Descriptif",
        "opposition" => "Oppositions",
        "declarationCreance" => "Déclaration des créances",
        "famille" => "Famille",
        "nature" => "Nature",
        "date" => "Date",
        "complementJugement" => "Complément",
        "radiationPM" => "Radiation de la personne morale",
        "commentaire" => "Commentaire",
        "dateCloture" => "Clôture de l'exercice",
        "typeDepot" => "Type de dépôt",
        "nonInscrit" => "Immatriculation",
        "idAssoc" | "IDASSOC" => "Numéro RNA",
        "titre" | "TITRE" => "Titre",
        "objet" | "object" | "OBJET" => "Objet",
        "numRNF" => "Numéro RNF",
        "siteWeb" => "Site web",
        "ancienTitre" | "ANCIEN_TITRE" => "Ancien titre",
        "nouveauTitre" | "NOUVEAU_TITRE" => "Nouveau titre",
        "ancienObjet" => "Ancien objet",
        "nouvelObjet" | "NOUVEL_OBJET" => "Nouvel objet",
        "ADDITIF_OBJET" => "Additif à l'objet",
        "ANCIEN_SIEGE" => "Ancien siège social",
        "TRANSFERE_NOUVELLE_ADRESSE" | "transfereNouvelleAdresse" => "Nouvelle adresse",
        "siegeSocial" | "SIEGE_SOCIAL" => "Siège social",
        "LIEU_DECLARATION" => "Lieu de déclaration",
        "BP" | "bp" => "Boîte postale",
        "INTERNET" => "Internet",
        "themes" | "THEMES" => "Thèmes",
        "dateDeclaration" => "Date de déclaration",
        "dateDelivrance" => "Date de délivrance",
        "dateActeDissolution" => "Date de l'acte de dissolution",
        "actifNet" => "Actif net",
        "attributaireActifNet" => "Attributaire de l'actif net",
        "liquidateur" => "Liquidateur",
        "duree" => "Durée",
        "nouvelleDuree" => "Nouvelle durée",
        "dateProrogation" => "Date de prorogation",
        "dateModifStatuts" | "dateModificationStatuts" => "Modification des statuts",
        "modifStatutaires" => "Modifications statutaires",
        "modaliteSuspension" => "Modalités de la suspension",
        "motifSuspension" => "Motif de la suspension",
        "dureeSuspension" => "Durée de la suspension",
        "fondateurs" => "Fondateurs",
        "programmePluriannuel" => "Programme pluriannuel",
        "dateAutorisationAdmin" => "Date de l'autorisation",
        "dateCreationAutorisationAdmin" => "Date de l'autorisation de création",
        "prefetAutorisationAdmin"
        | "prefetCreation"
        | "prefetModifications"
        | "prefetProrogation" => "Préfet",
        "recteurAutorisationAdmin"
        | "recteurCreation"
        | "recteurModifications"
        | "recteurProrogation" => "Recteur",
        "saisine" => "Saisine",
        "titreAnnonce" => "Titre de l'annonce",
        "contenuAnnonce" => "Annonce",
        "lieuPref" => "Préfecture",
        "lieuSousPref" => "Sous-préfecture",
        part => match group_label(part) {
            Some(heading) => heading,
            None => {
                tracing::warn!(element = part, "notice field without a label");
                return part.to_string();
            }
        },
    };
    known.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(name: &str, children: Vec<Node>) -> Node {
        Node {
            name: name.to_string(),
            attrs: Vec::new(),
            body: Body::Children(children),
        }
    }

    fn text(name: &str, t: &str) -> Node {
        Node {
            name: name.to_string(),
            attrs: Vec::new(),
            body: Body::Text(t.to_string()),
        }
    }

    fn field(group: Option<&str>, label: &str, value: &str) -> NoticeField {
        NoticeField {
            group: group.map(str::to_string),
            label: label.to_string(),
            value: value.to_string(),
            href: None,
        }
    }

    #[test]
    fn a_sale_reads_as_its_parts_with_addresses_and_registrations_on_one_line() {
        let notice = el(
            "avis",
            vec![
                el("typeAnnonce", vec![el("annonce", vec![])]),
                text("nojo", "0000202MYG29413"),
                text("tribunal", "GREFFE DU TRIBUNAL DE COMMERCE DE ST-QUENTIN"),
                el(
                    "personnes",
                    vec![el(
                        "personne",
                        vec![
                            el(
                                "personneMorale",
                                vec![
                                    el(
                                        "numeroImmatriculation",
                                        vec![
                                            text("numeroIdentification", "529 488 520"),
                                            text("codeRCS", "RCS"),
                                            text("nomGreffeImmat", "Saint-Quentin"),
                                        ],
                                    ),
                                    text("denomination", "ATOUT'PAINS"),
                                ],
                            ),
                            el(
                                "adresse",
                                vec![el(
                                    "france",
                                    vec![
                                        text("numeroVoie", "8"),
                                        text("typeVoie", "Rue"),
                                        text("nomVoie", "de Flandre"),
                                        text("codePostal", "02690"),
                                        text("ville", "Essigny-le-Grand"),
                                    ],
                                )],
                            ),
                        ],
                    )],
                ),
                el(
                    "acte",
                    vec![el(
                        "vente",
                        vec![
                            text("dateCommencementActivite", "2026-10-30"),
                            el(
                                "journal",
                                vec![
                                    text("titre", "democrate-aisne.fr"),
                                    text("date", "2026-11-06"),
                                ],
                            ),
                        ],
                    )],
                ),
            ],
        );
        assert_eq!(
            fields(&notice),
            [
                field(
                    Some("Personne"),
                    "Immatriculation",
                    "529 488 520 RCS Saint-Quentin"
                ),
                field(Some("Personne"), "Dénomination", "ATOUT'PAINS"),
                field(
                    Some("Personne"),
                    "Adresse",
                    "8 Rue de Flandre, 02690 Essigny-le-Grand"
                ),
                field(Some("Vente"), "Début d'activité", "30/10/2026"),
                field(
                    Some("Vente"),
                    "Journal d'annonces légales",
                    "democrate-aisne.fr, 06/11/2026"
                ),
            ]
        );
    }

    #[test]
    fn a_notice_correcting_another_links_to_it() {
        let bodacc = el(
            "avis",
            vec![el(
                "parutionAvisPrecedent",
                vec![
                    text("nomPublication", "BODACC B"),
                    text("numeroParution", "20260211"),
                    text("dateParution", "2026-11-02"),
                    text("numeroAnnonce", "95"),
                ],
            )],
        );
        let [f] = &fields(&bodacc)[..] else {
            panic!("one field")
        };
        assert_eq!(f.value, "BODACC B n° 20260211 du 02/11/2026, annonce n° 95");
        assert_eq!(f.href.as_deref(), Some("/notice/fr/bodacc:B2026021195"));
        let joafe = el(
            "rectificatif",
            vec![el(
                "contenu",
                vec![el(
                    "refAvisInitial",
                    vec![
                        text("numAnnonce", "1384"),
                        text("numParution", "20260037"),
                        text("dateParution", "2026-09-15"),
                    ],
                )],
            )],
        );
        let [f] = &fields(&joafe)[..] else {
            panic!("one field")
        };
        assert_eq!(f.href.as_deref(), Some("/notice/fr/joafe:202600371384"));
    }

    #[test]
    fn a_json_delivery_reads_as_the_xml_and_never_shows_the_management_address() {
        let empty = |name: &str| Node {
            name: name.to_string(),
            attrs: Vec::new(),
            body: Body::Empty,
        };
        let address = |town: &str| {
            vec![
                text("codePostal", "01470"),
                text("commune", town),
                empty("pays"),
                text("numeroVoie", "13"),
                empty("extensionVoie"),
                text("typeVoie", "rue"),
                text("nomVoie", "de la Cité"),
            ]
        };
        let notice = el(
            "annonce",
            vec![
                el(
                    "metadonnees",
                    vec![
                        text("page#", "1"),
                        el(
                            "themes",
                            vec![el(
                                "theme[]",
                                vec![el(
                                    "theme",
                                    vec![
                                        text("code", "024026"),
                                        text("Libelle", "protection des animaux"),
                                    ],
                                )],
                            )],
                        ),
                        el(
                            "coordonneesGestion",
                            vec![el("adresseGestion", address("BRIORD"))],
                        ),
                    ],
                ),
                el(
                    "contenu",
                    vec![
                        el(
                            "assoLoi1901",
                            vec![
                                el("creation", vec![el("siegeSocial", address("Briord"))]),
                                el(
                                    "lieuDeclaration",
                                    vec![el(
                                        "lieuSousPref",
                                        vec![
                                            text("libelle", "Belley"),
                                            text("description", "SOUS-PREFECTURE DE BELLEY"),
                                        ],
                                    )],
                                ),
                            ],
                        ),
                        empty("refAvisInitial{}"),
                    ],
                ),
            ],
        );
        assert_eq!(
            fields(&notice),
            [
                field(None, "Thèmes", "protection des animaux"),
                field(
                    Some("Création"),
                    "Siège social",
                    "13 rue de la Cité, 01470 Briord"
                ),
                field(
                    Some("Association loi de 1901"),
                    "Lieu de déclaration",
                    "Belley"
                ),
            ]
        );
    }
}
