use std::sync::LazyLock;

use jiff::Timestamp;
use regex::Regex;

use lj_annonces::zones::{zones_departements, Zone};
use lj_annonces::{
    departement_from_cp, echeance_ts, normalise_departement, parse_heure, AnnonceBrute,
    ContenuAnnonce, LotAnnonce, NatureMarche,
};
use lj_core::dates::parse_date_fr;
use lj_norm::referential_labels::departement_label;

use super::{
    apres_balise, client, dedup_source_uid, fiche, send_text, txt, DetailAnnonce, MoteurRun,
};
use lj_sources::error::Result;

const BASE: &str = "https://www.klekoon.com";
const RECHERCHE: &str =
    "https://www.klekoon.com/rechercher-une-annonce-ou-un-dce-dematerialise-sur-klekoon";
const PER_PAGE: &str = "100";

const POLITESSE: std::time::Duration = std::time::Duration::from_secs(1);

/// Pages consécutives sans aucune consultation ouverte avant d'arrêter.
///
/// Le tri est par **parution**, pas par clôture : les avis clos sont mêlés aux
/// ouverts tout du long, et s'arrêter à la première clôture passée coupe
/// l'énumération bien trop tôt. Le seul repère solide est la page entière
/// muette.
const PAGES_MUETTES: u32 = 2;

/// Plafond dur : l'archive fait 640 pages à 100 par page et on ne moissonne
/// que l'ouvert.
const MAX_PAGES: u64 = 10;

/// Une page de résultats. Le formulaire poste ses dix champs ; `page` et
/// `per_page` sont les seuls qui varient.
async fn page(client: &wreq::Client, p: u64) -> Result<String> {
    let p = p.to_string();
    send_text(
        client.post(RECHERCHE).header("Referer", RECHERCHE).form(&[
            ("type_recherche", "1"),
            ("page", &p),
            ("per_page", PER_PAGE),
            ("motcle", ""),
            ("source", "1"),
            ("list_region", ""),
            ("selection_list", ""),
            ("retour", "0"),
            ("id_retour", ""),
            ("critere_emetteur", ""),
        ]),
        &format!("klekoon page {p}"),
    )
    .await
}

/// Total déclaré : le `<select name="critere_emetteur">` du formulaire, servi
/// avec chaque page de résultats, annote ses acheteurs d'un « (N marchés) » qui
/// compte leurs consultations ouvertes. La somme est un compte **indépendant**
/// du listing.
fn total_declare(html: &str) -> u64 {
    static SELECT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)name="critere_emetteur".*?</select>"#).expect("regex select valide")
    });
    static MARCHES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\((\d+)\s*march").expect("regex marchés valide"));
    let Some(bloc) = SELECT.find(html) else {
        return 0;
    };
    MARCHES
        .captures_iter(bloc.as_str())
        .filter_map(|c| c[1].parse::<u64>().ok())
        .sum()
}

/// Le délai de dépôt court encore. Sans clôture lisible, une ligne n'entre
/// jamais dans la fenêtre ouverte du fond (`echeance > NOW()`).
fn ouverte(r: &AnnonceBrute, maintenant: Timestamp) -> bool {
    r.echeance.is_some_and(|e| e > maintenant)
}

/// Columns read, in order: objet, émetteur, procédure, parution, clôture. The
/// sixth one carries the action icons only.
const COLONNES: usize = 5;

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    const LIGNE: &str = r#"<tr id=""#;
    static SLUG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"/appels-offres/avis/([^"?#]+\.htm)"#).expect("regex slug valide")
    });
    static DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{2}/\d{2}/\d{4}").expect("regex date valide"));
    // The buyer label ends on its département in brackets, sometimes followed
    // by the restricted-access flag, which is no part of a buyer's name.
    static DEPT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\s*\((\d{2,3}|2[AB])\)(?:\s*Accès restreint)?\s*$")
            .expect("regex dept valide")
    });

    let mut out = Vec::new();
    for bloc in html.split(LIGNE).skip(1) {
        let id = bloc.split('"').next().unwrap_or_default();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // Without the closing bound the last row of the page swallows the
        // whole footer.
        let corps = apres_balise(bloc);
        let corps = corps.split_once("</tr>").map_or(corps, |(l, _)| l);
        let Some(slug) = SLUG.captures(corps).map(|c| c[1].to_string()) else {
            continue;
        };
        let cellules: Vec<String> = corps
            .split("</td>")
            .map(|c| txt(c).trim().to_string())
            .collect();
        if cellules.len() < COLONNES {
            continue;
        }
        let avis = format!("{BASE}/appels-offres/avis/{slug}");
        let emetteur = Some(cellules[1].clone()).filter(|e| !e.is_empty());
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "klekoon",
            source_uid: format!("klekoon:{id}"),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: emetteur
                .as_deref()
                .map(|e| DEPT.replace(e, "").trim().to_string())
                .filter(|a| !a.is_empty()),
            siren: None,
            objet: cellules[0].chars().take(400).collect(),
            nature: None,
            cpv: None,
            // Le suffixe du libellé note la Corse « (20) », code d'avant 1976
            // qui ne tranche pas entre 2A et 2B : hors référentiel, donc pas
            // de département. La fiche, elle, porte le code postal.
            zones: zones_departements(
                emetteur
                    .as_deref()
                    .and_then(|e| DEPT.captures(e))
                    .and_then(|c| normalise_departement(&c[1]))
                    .filter(|d| departement_label(d).is_some()),
            ),
            date_publication: DATE
                .find(&cellules[3])
                .and_then(|m| parse_date_fr(m.as_str())),
            echeance: DATE
                .find(&cellules[4])
                .and_then(|m| parse_date_fr(m.as_str()))
                .map(|d| echeance_ts(d, None)),
            url_avis: Some(avis.clone()),
            url_rc: None,
            // Les pièces du marché et le dépôt de pli sont deux vues de la
            // fiche, commutées sur le fragment (`case "#depot_plis"` du
            // routeur de la page) — le dépôt route vers
            // `/dematernet/entreprise_privee_02_enveloppe.asp`.
            url_dce: Some(format!("{avis}#section_pieces")),
            url_depot: Some(format!("{avis}#depot_plis")),
            procedure: cellules[2]
                .strip_prefix("Klekoon - ")
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty()),
        });
    }
    out
}

/// Ce que la fiche apprend en plus de la ligne de liste.
#[derive(Debug, Default, PartialEq, Eq)]
struct Fiche {
    reference: Option<String>,
    /// Full object, which the list truncates.
    objet: Option<String>,
    nature: Option<NatureMarche>,
    /// Division CPV seule : le moteur publie « Classe 45 : Travaux de
    /// construction », jamais un code à huit chiffres.
    cpv: Option<String>,
    zones: Vec<Zone>,
    date_publication: Option<jiff::civil::Date>,
    /// Date **et heure** limite de remise des offres, que la liste n'a pas.
    echeance: Option<Timestamp>,
    procedure: Option<String>,
}

fn champs_fiche(html: &str) -> Fiche {
    static REF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Référence de la consultation\s*:\s*(.*?)\s*Mise en ligne")
            .expect("regex référence valide")
    });
    static MISE_EN_LIGNE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Mise en ligne\s*:\s*(\d{2}/\d{2}/\d{4})").expect("regex mise en ligne valide")
    });
    static PASSATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Mode de passation\s*:\s*(.*?)\s*Catégorie de marché")
            .expect("regex passation valide")
    });
    static CATEGORIE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Catégorie de marché\s*:\s*(.*?)\s*Classe d'activité")
            .expect("regex catégorie valide")
    });
    static CLASSE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Classe d'activité\s*:\s*Classe\s*(\d{1,2})\s*:").expect("regex classe valide")
    });
    static LIMITE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"Date limite des offres \(heure de Paris\)\s*:\s*(\d{2}/\d{2}/\d{4})(?:\s+([\d:]+))?",
        )
        .expect("regex date limite valide")
    });
    // Le bloc « Acheteur public » porte l'adresse postale : son code postal
    // tranche la Corse (20213 → 2B) là où le champ « Département : Corse (20) »
    // de la fiche reste ambigu.
    static ADRESSE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Acheteur public\s+(.*?)\s+Informations générales")
            .expect("regex adresse valide")
    });
    static CP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b\d{5}\b").expect("regex code postal valide"));

    let flat = txt(html);
    let adresse = ADRESSE
        .captures(&flat)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    Fiche {
        reference: REF
            .captures(&flat)
            .map(|c| c[1].chars().take(120).collect::<String>())
            .filter(|r| !r.is_empty()),
        objet: objet_consultation(html).map(|o| o.chars().take(400).collect::<String>()),
        nature: CATEGORIE
            .captures(&flat)
            .and_then(|c| NatureMarche::parse(&c[1])),
        cpv: CLASSE
            .captures(&flat)
            .map(|c| format!("{:0>2}", &c[1]))
            .filter(|d| lj_annonces::cpv_division_uid(d).is_some()),
        zones: zones_departements(
            CP.find_iter(adresse.split("Téléphone").next().unwrap_or_default())
                .last()
                .and_then(|m| departement_from_cp(m.as_str())),
        ),
        date_publication: MISE_EN_LIGNE
            .captures(&flat)
            .and_then(|c| parse_date_fr(&c[1])),
        echeance: LIMITE.captures(&flat).and_then(|c| {
            let d = parse_date_fr(&c[1])?;
            Some(echeance_ts(
                d,
                c.get(2).and_then(|h| parse_heure(h.as_str())),
            ))
        }),
        procedure: PASSATION
            .captures(&flat)
            .map(|c| c[1].trim().to_string())
            .filter(|p| !p.is_empty()),
    }
}

/// Complète une ligne par sa fiche. Une fiche en échec garde la ligne de
/// liste.
async fn completer_par_fiche(client: &wreq::Client, mut row: AnnonceBrute) -> AnnonceBrute {
    let url = row.url_avis.clone().unwrap_or_default();
    let html = match send_text(client.get(&url), "klekoon fiche").await {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(url = %url, error = %e, "klekoon: record failed");
            return row;
        }
    };
    let f = champs_fiche(&html);
    row.reference = f.reference;
    row.nature = f.nature;
    row.cpv = f.cpv;
    if let Some(objet) = f.objet {
        row.objet = objet;
    }
    if !f.zones.is_empty() {
        row.zones = f.zones;
    }
    if f.date_publication.is_some() {
        row.date_publication = f.date_publication;
    }
    if f.echeance.is_some() {
        row.echeance = f.echeance;
    }
    if f.procedure.is_some() {
        row.procedure = f.procedure;
    }
    row
}

/// Re-enumerates the open stock, then completes each row from its notice page. All
/// in series, `POLITESSE` apart: the listing pages, then one page per open
/// consultation, in parallel with the other engines.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = client();
    let maintenant = Timestamp::now();
    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut declare = 0;
    let mut muettes = 0;
    for p in 1..=MAX_PAGES {
        if p > 1 {
            tokio::time::sleep(POLITESSE).await;
        }
        let html = page(&client, p).await?;
        if p == 1 {
            declare = total_declare(&html);
        }
        let lignes = parse_page(&html);
        if lignes.is_empty() {
            break;
        }
        let avant = rows.len();
        rows.extend(lignes.into_iter().filter(|r| ouverte(r, maintenant)));
        if rows.len() > avant {
            muettes = 0;
        } else {
            muettes += 1;
            if muettes >= PAGES_MUETTES {
                break;
            }
        }
    }

    let mut completes = Vec::with_capacity(rows.len());
    for row in rows {
        tokio::time::sleep(POLITESSE).await;
        completes.push(completer_par_fiche(&client, row).await);
    }
    let rows = completes;

    Ok(MoteurRun {
        annonce: (declare > 0).then_some(declare),
        echecs: 0,
        hotes: Vec::new(),
        rows: dedup_source_uid(rows),
    })
}

/// The `Objet de la consultation` block, which the buyer fills with a sentence
/// or a whole paragraph. The headings of the page carry no class the label walk
/// recognises, so nothing else reaches it.
fn objet_consultation(html: &str) -> Option<String> {
    static OBJET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Objet de la consultation\s*</(?:span|h5)>\s*<p>(.*?)</p>")
            .expect("regex objet valide")
    });
    OBJET
        .captures(html)
        .map(|c| txt(&c[1]).trim().to_string())
        .filter(|o| !o.is_empty())
}

/// One paragraph a lot, the next one its description. A market that is not
/// allotted still lists a single lot, which is then the market itself.
fn lots(html: &str) -> Vec<LotAnnonce> {
    static SECTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Liste des lots\s*</(?:span|h5)>(.*?)</div>")
            .expect("regex liste des lots valide")
    });
    static PARAGRAPHE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<p>(.*?)</p>").expect("regex paragraphe valide"));
    static LOT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^Lot\s*:\s*(\S.*)$").expect("regex lot valide"));

    let Some(section) = SECTION.captures(html) else {
        return Vec::new();
    };
    let mut out: Vec<LotAnnonce> = Vec::new();
    for p in PARAGRAPHE.captures_iter(&section[1]) {
        let texte = txt(&p[1]).trim().to_string();
        if texte.is_empty() {
            continue;
        }
        match LOT.captures(&texte) {
            Some(m) => out.push(LotAnnonce {
                nom: Some(m[1].trim().to_string()),
                description: None,
                cpv: None,
            }),
            None => {
                if let Some(lot) = out.last_mut() {
                    lot.description.get_or_insert(texte);
                }
            }
        }
    }
    out
}

/// Notice page. The eight-digit CPV and the procedure sit in the
/// `Informations complémentaires` block, which a buyer fills or leaves empty;
/// without it the CPV falls back to the division alone (`Classe 45`).
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    tokio::time::sleep(POLITESSE).await;
    let html = send_text(client().get(url_avis), "klekoon détail").await?;
    Ok(parse_detail(&html))
}

fn parse_detail(html: &str) -> DetailAnnonce {
    let f = champs_fiche(html);
    let champs = fiche::champs(html);
    let mut contenu = fiche::contenu(&champs, objet_consultation(html));
    let lots = lots(html);
    if !lots.is_empty() {
        contenu.lots = lots;
    }
    DetailAnnonce {
        contenu,
        zones: Vec::new(),
        cpv: fiche::premier(&champs, &["codes cpv"])
            .as_deref()
            .and_then(fiche::cpv_de)
            .or(f.cpv),
        procedure: fiche::premier(&champs, &["type de procedure"]).or(f.procedure),
        acheteur: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_annonces::zones::codes;

    /// A real listing row. The sixth cell, action icons only, is dropped.
    const LIGNE: &str = r#"<tr id="93552">
<td>

        <a href="/appels-offres/avis/travaux-renovation-eclairage-terrain-football-nyambadao-93552.htm" class="text-decoration-none text-gray-6 text-600">
            Travaux de rénovation de l’éclairage du terrain de football de Nyambadao.
        </a>

    </td><td>
        MAIRIE DE BANDRELE (976)
                 </span>
    </td><td class="text-700">Klekoon - Procédure adaptée
    </td><td>
        14/09/2026
    </td><td class="">


        <span class="text-green-custom">
            13/10/2026
        </span>


    </td>
</tr>"#;

    /// The restricted-access flag sticks to the buyer label, behind the
    /// département, and the closing cell opens a dropdown of its own dates.
    const LIGNE_RESTREINTE: &str = r#"<tr id="92316">
<td>

        <a href="/appels-offres/avis/fourniture-carburants-services-associes-92316.htm" class="text-decoration-none text-gray-6 text-600">
            FOURNITURE DE CARBURANTS ET SERVICES ASSOCIES
        </a>

    </td><td>
        SEOLIS SAEML (79)

        <span class="text-warning">&nbsp;Acc&#232;s restreint</span>
         </span>
    </td><td class="text-700">Klekoon - Procédure négociée avec mise en concurrence préalable
    </td><td>
        10/11/2025
    </td><td class="">


        <div class="dropdown">
            <a class="pointer dropdown-toggle pl-0">

                <span class="text-danger">
                    06/01/2026
                </span>

            </a>

            <div class="dropdown-menu">
                <span class="dropdown-item">Date limite :</span>
                <span class="dropdown-item">Candidature :
                <span class="text-danger">02/12/2025 12:00:00</span>
                </span>
                <span class="dropdown-item">Offre :
                <span class="text-danger">06/01/2026 12:00:00</span>
                </span>
            </div>
        </div>


    </td>
</tr>"#;

    fn le(annee: i16, mois: i8, jour: i8, h: i8, m: i8) -> Timestamp {
        echeance_ts(
            jiff::civil::date(annee, mois, jour),
            Some(jiff::civil::time(h, m, 0, 0)),
        )
    }

    #[test]
    fn ligne_de_liste() {
        let r = &parse_page(LIGNE)[0];
        assert_eq!(r.source_uid, "klekoon:93552");
        assert_eq!(
            r.objet,
            "Travaux de rénovation de l’éclairage du terrain de football de Nyambadao."
        );
        assert_eq!(r.acheteur.as_deref(), Some("MAIRIE DE BANDRELE"));
        assert_eq!(codes(&r.zones), ["976"]);
        assert_eq!(r.procedure.as_deref(), Some("Procédure adaptée"));
        assert_eq!(
            r.date_publication,
            Some(jiff::civil::date(2026, 9, 14)),
            "la parution est la date de mise en ligne, pas la clôture"
        );
        // Sans heure, la consultation reste ouverte jusqu'au bout du jour civil.
        assert_eq!(
            r.echeance,
            Some(echeance_ts(jiff::civil::date(2026, 10, 13), None))
        );
        assert_eq!(
            r.url_avis.as_deref(),
            Some(
                "https://www.klekoon.com/appels-offres/avis/travaux-renovation-eclairage-terrain-football-nyambadao-93552.htm"
            )
        );
        assert_eq!(
            r.url_depot.as_deref(),
            Some(
                "https://www.klekoon.com/appels-offres/avis/travaux-renovation-eclairage-terrain-football-nyambadao-93552.htm#depot_plis"
            )
        );
    }

    #[test]
    fn le_fanion_acces_restreint_quitte_le_libelle() {
        let r = &parse_page(LIGNE_RESTREINTE)[0];
        assert_eq!(r.acheteur.as_deref(), Some("SEOLIS SAEML"));
        assert_eq!(codes(&r.zones), ["79"]);
        assert_eq!(
            r.procedure.as_deref(),
            Some("Procédure négociée avec mise en concurrence préalable")
        );
    }

    #[test]
    fn la_corse_du_libelle_ne_donne_pas_de_departement() {
        let html = LIGNE.replace("MAIRIE DE BANDRELE (976)", "PENTA DI CASINCA (20)");
        let r = &parse_page(&html)[0];
        assert_eq!(r.acheteur.as_deref(), Some("PENTA DI CASINCA"));
        assert!(r.zones.is_empty());
    }

    #[test]
    fn seule_une_page_sans_ouverte_arrete_l_enumeration() {
        let maintenant = le(2026, 8, 8, 12, 0);
        let melangee = format!(
            "{}{}",
            LIGNE
                .replace("id=\"93552\"", "id=\"93088\"")
                .replace("13/10/2026", "09/07/2026"),
            LIGNE_RESTREINTE
                .replace("id=\"92316\"", "id=\"93080\"")
                .replace("06/01/2026", "21/08/2026")
        );
        let lignes = parse_page(&melangee);
        assert_eq!(lignes.len(), 2);
        let ouvertes: Vec<_> = lignes.iter().filter(|r| ouverte(r, maintenant)).collect();
        assert_eq!(ouvertes.len(), 1, "la page porte encore une ouverte");
        assert_eq!(ouvertes[0].source_uid, "klekoon:93080");

        let close = LIGNE.replace("13/10/2026", "11/03/2026");
        assert!(!parse_page(&close).iter().any(|r| ouverte(r, maintenant)));
    }

    #[test]
    fn un_bloc_sans_cloture_ne_compte_pas_comme_ouvert() {
        let html = LIGNE.replace("13/10/2026", "");
        let r = &parse_page(&html)[0];
        assert_eq!(r.echeance, None);
        assert_eq!(r.source_uid, "klekoon:93552");
        assert!(!ouverte(r, le(2026, 8, 8, 12, 0)));
    }

    /// Fiche réelle (`consultation_ID=93409`), réduite aux deux blocs que lit
    /// `champs_fiche`.
    const FICHE: &str = r#"<div class="col-8 col-lg-10">
        <span>Acheteur public</span>
        <p>
            MAIRIE DE BANDRELE
            <br>
            PLACE DE LA MAIRIE<br />97660 BANDRELE FR
            <br>
            T&eacute;l&eacute;phone : 0102030405
        </p>
    </div>
    <div class="col-sm-12">
        <span>Informations g&eacute;n&eacute;rales </span>
        <p>
            R&eacute;f&eacute;rence de la consultation : K4192977
            <br>
            Mise en ligne : 30/07/2026 08:14:11
            <br>
            Mode de passation :
            <br>
            Cat&eacute;gorie de march&eacute; : Travaux
            <br />
            Classe d'activit&eacute; : Classe 31 : Machines, appareils, équipements et consommables électriques; éclairage<br />
            <br />
            D&eacute;partement : Mayotte  (976)
            <br />
            Date limite des candidatures (heure de Paris) :
            <br />
            Date limite des offres (heure de Paris) : <font color=#00B71C>28/08/2026 10:00</font>
        </p>
    </div>
    <div class="col-sm-12">
        <span>Objet de la consultation </span>
        <p>Travaux de rénovation de l’éclairage du terrain de football de Nyambadao</p>
    </div>"#;

    #[test]
    fn la_fiche_donne_reference_heure_et_division_cpv() {
        let f = champs_fiche(FICHE);
        assert_eq!(f.reference.as_deref(), Some("K4192977"));
        assert_eq!(
            f.objet.as_deref(),
            Some("Travaux de rénovation de l’éclairage du terrain de football de Nyambadao")
        );
        assert_eq!(f.nature, Some(NatureMarche::Travaux));
        assert_eq!(f.cpv.as_deref(), Some("31"));
        assert_eq!(codes(&f.zones), ["976"]);
        assert_eq!(f.date_publication, Some(jiff::civil::date(2026, 7, 30)));
        assert_eq!(f.echeance, Some(le(2026, 8, 28, 10, 0)));
        assert_eq!(f.procedure, None);
    }

    #[test]
    fn le_code_postal_tranche_la_corse_et_la_reference_peut_manquer() {
        let f = champs_fiche(
            &FICHE
                .replace("97660 BANDRELE FR", "20213 PENTA DI CASINCA FR")
                .replace("Mayotte  (976)", "Corse  (20)")
                .replace("consultation : K4192977", "consultation : "),
        );
        assert_eq!(codes(&f.zones), ["2B"]);
        assert_eq!(f.reference, None);
        let f3 = champs_fiche(&FICHE.replace("Classe 31 :", "Classe 3 :"));
        assert_eq!(f3.cpv.as_deref(), Some("03"));
    }

    /// Real notice 93393, cut down to its two headed blocks.
    const FICHE_LOTS: &str = r#"<div>
        <h5 class="text-700 font-18 pb-2 mt-24px">Objet de la consultation</h5>
        <p>Les prestations, objets de la pr&eacute;sente consultation, sont les prestations d'hydrocurage des grilles, des regards, des r&eacute;seaux d'eaux pluviales.</p>
        </div>
        <div>
        <h5 class="text-700 font-18 pb-2 mt-24px">Liste des lots</h5>
        <p>Lot  : 2026-12 Curage des grilles, des regards, des r&eacute;seaux d'eaux pluviales</p>
        <p>2026-12 Curage des grilles et pompage des puisards sur les sites des &eacute;quipements sportifs</p>
        <p>Lot  : 2026-13 Nettoyage des s&eacute;parateurs d'hydrocarbures</p>
        <p></p>
        </div>"#;

    #[test]
    fn a_lot_takes_the_paragraph_that_follows_it_as_its_description() {
        let d = parse_detail(FICHE_LOTS);
        assert_eq!(
            d.contenu.description.as_deref(),
            Some(
                "Les prestations, objets de la présente consultation, sont les prestations \
                 d'hydrocurage des grilles, des regards, des réseaux d'eaux pluviales."
            )
        );
        assert_eq!(
            d.contenu.lots,
            [
                LotAnnonce {
                    nom: Some(
                        "2026-12 Curage des grilles, des regards, des réseaux d'eaux pluviales"
                            .to_string()
                    ),
                    description: Some(
                        "2026-12 Curage des grilles et pompage des puisards sur les sites des \
                         équipements sportifs"
                            .to_string()
                    ),
                    cpv: None,
                },
                LotAnnonce {
                    nom: Some("2026-13 Nettoyage des séparateurs d'hydrocarbures".to_string()),
                    description: None,
                    cpv: None,
                },
            ]
        );
    }

    /// Real complementary block of notice 93556, which a buyer fills and the
    /// next one leaves empty.
    const COMPLEMENTAIRES: &str = r#"<div>
    <h5 class="text-700 font-18 pb-2 mt-24px" id="informationsComplementaires">Informations compl&#233;mentaires</h5>
    <p>Type de proc&#233;dure: Proc&#233;dure adapt&#233;e ouverte<br/>Crit&#232;res d&#8217;attribution: voir r&#232;glement de consultation<br/><br/>Codes CPV: 71000000-8<br/>Allotissement: Non</p>
    </div>"#;

    #[test]
    fn the_complementary_block_gives_the_full_cpv_where_the_notice_gave_a_division() {
        let html = format!("{FICHE}{COMPLEMENTAIRES}");
        let d = parse_detail(&html);
        assert_eq!(d.cpv.as_deref(), Some("71000000-8"));
        assert_eq!(d.procedure.as_deref(), Some("Procédure adaptée ouverte"));
        assert!(
            d.contenu.criteres.is_empty(),
            "a referral to the rulebook names no criterion"
        );
        assert_eq!(parse_detail(FICHE).cpv.as_deref(), Some("31"));
    }

    #[test]
    fn la_page_d_erreur_rend_un_detail_vide() {
        let html = "<h1>&nbsp;</h1><p>La page que vous recherchez n'est pas actuellement \
                    disponible.</p><a href=\"/inscription-klekoon\">Inscrivez-vous</a>";
        assert_eq!(champs_fiche(html), Fiche::default());
        assert_eq!(parse_detail(html), DetailAnnonce::default());
    }

    #[test]
    fn total_declare_somme_le_selecteur_d_acheteurs() {
        let html = r#"<select class="form-control w-100 pointer" name="critere_emetteur" id="critere-emetteur" >
            <option  selected="selected"  value="0">Sélectionner un acheteur public</option>
            <optgroup label="01 - Ain">
                <option value="82890" >Mairie de Ferney-Voltaire (0 marché) (39 données)</option>
            <optgroup label="03 - Allier">
                <option value="611519" >COMMUNE D'AVERMES (1 marché) (8 données)</option>
            <optgroup label="57 - Moselle">
                <option value="94582" >MAIRIE SAINT AVOLD (15 marchés) (272 données)</option>
            </select>
            <p>Klekoon publie (999 marchés) hors du sélecteur</p>"#;
        assert_eq!(total_declare(html), 16);
        assert_eq!(total_declare("<html>pas de sélecteur</html>"), 0);
    }
}
