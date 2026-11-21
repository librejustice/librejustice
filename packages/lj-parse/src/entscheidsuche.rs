//! An entscheidsuche.ch document into a [`Decision`] and its `source_fields`: the court and the
//! chamber from its hierarchy, the dockets from its references, the text its index extracted.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use lj_core::decision::Decision;
use lj_core::error::{CoreError, Result};

use crate::reflow_page_text;

/// A Swiss court the load takes, by its id in the index.
#[derive(Debug)]
pub struct SwissCourt {
    pub id: &'static str,
    pub jurisdiction_type: &'static str,
    pub code: &'static str,
    pub label: &'static str,
    /// `geo:CH` for a federal court, its canton for a cantonal one.
    pub place: &'static str,
}

/// The country a court's name ends on, in each language but French the index names courts in.
const SWITZERLAND: [(&str, &str); 2] = [("de", "Schweiz"), ("it", "Svizzera")];

/// The courts the facets name after a chamber, a collection or with a typo: their name in German
/// and Italian, as the label departs from the facets' French.
const RENAMED: [(&str, &str, &str); 11] = [
    ("GE_CJ", "Cour de justice", "Cour de justice"),
    (
        "GE_CAPJ",
        "Cour d'appel du pouvoir judiciaire",
        "Cour d'appel du pouvoir judiciaire",
    ),
    (
        "GE_TAPI",
        "Tribunal administratif de première instance",
        "Tribunal administratif de première instance",
    ),
    ("NE_TR", "Tribunaux régionaux", "Tribunaux régionaux"),
    (
        "LU_AVW",
        "Andere Verwaltungsbehörden",
        "Andere Verwaltungsbehörden",
    ),
    ("NW_OG", "Obergericht", "Obergericht"),
    (
        "LU_SA",
        "Schiedsgericht nach den Bestimmungen des eidg. Sozialversicherungsrechts",
        "Schiedsgericht nach den Bestimmungen des eidg. Sozialversicherungsrechts",
    ),
    (
        "AG_BV",
        "Departement Bau, Verkehr und Umwelt",
        "Departement Bau, Verkehr und Umwelt",
    ),
    ("TI_PP", "Pretura penale", "Pretura penale"),
    (
        "CH_VB",
        "Verwaltungsbehörden des Bundes",
        "Autorità amministrative della Confederazione",
    ),
    (
        "CH_EDÖB",
        "Eidgenössischer Datenschutz- und Öffentlichkeitsbeauftragter",
        "Incaricato federale della protezione dei dati e della trasparenza",
    ),
];

/// Each court's name in German and Italian, with its canton's, and its chambers' keyed by the
/// French name a decision carries: `{"de": {"name": "Obergericht (Bern, Schweiz)", "chambers": {..}}}`.
pub fn court_names(facets: &Value) -> Vec<(&'static str, Value)> {
    SWISS_COURTS
        .iter()
        .chain(std::iter::once(&VS_TC))
        .filter_map(|court| {
            let (canton_id, canton) = facets
                .as_object()?
                .iter()
                .find(|(_, canton)| canton["gerichte"].get(court.id).is_some())?;
            let entry = &canton["gerichte"][court.id];
            let mut names = serde_json::Map::new();
            for (lang, country) in SWITZERLAND {
                let (Some(name), Some(canton_name)) = (
                    entry.get(lang).and_then(Value::as_str),
                    canton.get(lang).and_then(Value::as_str),
                ) else {
                    continue;
                };
                let name = match RENAMED.iter().find(|(id, ..)| *id == court.id) {
                    Some((_, german, _)) if lang == "de" => german,
                    Some((_, _, italian)) => italian,
                    None => name,
                };
                let place = match canton_id.as_str() {
                    "CH" => country.to_string(),
                    _ => format!("{}, {country}", canton_name.trim()),
                };
                let chambers: serde_json::Map<String, Value> = entry
                    .get("kammern")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .filter_map(|(_, chamber)| {
                        let french = chamber.get("fr")?.as_str()?.trim();
                        let named = chamber.get(lang)?.as_str()?.trim();
                        (french != named).then(|| (french.to_string(), Value::from(named)))
                    })
                    .collect();
                let mut localized = json!({ "name": format!("{} ({place})", name.trim()) });
                if !chambers.is_empty() {
                    localized["chambers"] = Value::Object(chambers);
                }
                names.insert(lang.to_string(), localized);
            }
            Some((court.code, Value::Object(names)))
        })
        .collect()
}

pub(crate) const fn court(
    id: &'static str,
    jurisdiction_type: &'static str,
    code: &'static str,
    label: &'static str,
    place: &'static str,
) -> SwissCourt {
    SwissCourt {
        id,
        jurisdiction_type,
        code,
        label,
        place,
    }
}

/// The courts and administrative authorities the index serves decisions of. The sport
/// arbitration, the uploads, the yearly volumes of Appenzell Innerrhoden and the Federal Court's
/// reports (ATF) stay out.
pub const SWISS_COURTS: [SwissCourt; 109] = [
    court(
        "CH_BGer",
        "COUR_SUPREME",
        "ch_cour_supreme",
        "Tribunal fédéral (Suisse)",
        "geo:CH",
    ),
    court(
        "CH_BVGE",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_federal",
        "Tribunal administratif fédéral (Suisse)",
        "geo:CH",
    ),
    court(
        "CH_BSTG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_penal_federal",
        "Tribunal pénal fédéral (Suisse)",
        "geo:CH",
    ),
    court(
        "CH_PATG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_brevets",
        "Tribunal fédéral des brevets (Suisse)",
        "geo:CH",
    ),
    court(
        "CH_MKG",
        "COUR_CASSATION",
        "ch_cour_cassation_militaire",
        "Tribunal militaire de cassation (Suisse)",
        "geo:CH",
    ),
    court(
        "BE_OG",
        "COUR_APPEL",
        "ch_cour_appel_berne",
        "Cour suprême (Berne, Suisse)",
        "geo:CH-BE",
    ),
    court(
        "BE_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_berne",
        "Tribunal administratif (Berne, Suisse)",
        "geo:CH-BE",
    ),
    court(
        "FR_TC",
        "COUR_APPEL",
        "ch_cour_appel_fribourg",
        "Tribunal cantonal (Fribourg, Suisse)",
        "geo:CH-FR",
    ),
    court(
        "GE_CJ",
        "COUR_APPEL",
        "ch_cour_appel_geneve",
        "Cour de justice (Genève, Suisse)",
        "geo:CH-GE",
    ),
    court(
        "GE_CAPJ",
        "COUR_APPEL",
        "ch_cour_appel_geneve_pouvoir_judiciaire",
        "Cour d'appel du pouvoir judiciaire (Genève, Suisse)",
        "geo:CH-GE",
    ),
    court(
        "GE_TP",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_geneve_penal",
        "Tribunal pénal (Genève, Suisse)",
        "geo:CH-GE",
    ),
    court(
        "GE_TAPI",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_geneve",
        "Tribunal administratif de première instance (Genève, Suisse)",
        "geo:CH-GE",
    ),
    court(
        "JU_TC",
        "COUR_APPEL",
        "ch_cour_appel_jura",
        "Tribunal cantonal (Jura, Suisse)",
        "geo:CH-JU",
    ),
    court(
        "JU_TPI",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_jura",
        "Tribunal de première instance (Jura, Suisse)",
        "geo:CH-JU",
    ),
    court(
        "NE_TC",
        "COUR_APPEL",
        "ch_cour_appel_neuchatel",
        "Tribunal cantonal (Neuchâtel, Suisse)",
        "geo:CH-NE",
    ),
    court(
        "NE_TR",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_neuchatel",
        "Tribunaux régionaux (Neuchâtel, Suisse)",
        "geo:CH-NE",
    ),
    court(
        "VD_TC",
        "COUR_APPEL",
        "ch_cour_appel_vaud",
        "Tribunal cantonal (Vaud, Suisse)",
        "geo:CH-VD",
    ),
    court(
        "VS_BZG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_valais",
        "Tribunaux de première instance (Valais, Suisse)",
        "geo:CH-VS",
    ),
    court(
        "ZH_OG",
        "COUR_APPEL",
        "ch_cour_appel_zurich",
        "Tribunal supérieur (Zurich, Suisse)",
        "geo:CH-ZH",
    ),
    court(
        "ZH_KSG",
        "COUR_CASSATION",
        "ch_cour_cassation_zurich",
        "Cour de cassation (Zurich, Suisse)",
        "geo:CH-ZH",
    ),
    court(
        "ZH_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_zurich",
        "Tribunal administratif (Zurich, Suisse)",
        "geo:CH-ZH",
    ),
    court(
        "ZH_SVG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_zurich_assurances_sociales",
        "Tribunal des assurances sociales (Zurich, Suisse)",
        "geo:CH-ZH",
    ),
    court(
        "ZH_SRK",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_zurich_fiscal",
        "Tribunal de recours en matière fiscale (Zurich, Suisse)",
        "geo:CH-ZH",
    ),
    court(
        "ZH_BRK",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_zurich_construction",
        "Tribunal de recours en matière de construction (Zurich, Suisse)",
        "geo:CH-ZH",
    ),
    court(
        "ZH_HG",
        "TRIBUNAL_COMMERCE",
        "ch_tribunal_commerce_zurich",
        "Tribunal de commerce (Zurich, Suisse)",
        "geo:CH-ZH",
    ),
    court(
        "ZH_BK",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_zurich",
        "Tribunaux de district (Zurich, Suisse)",
        "geo:CH-ZH",
    ),
    court(
        "LU_KG",
        "COUR_APPEL",
        "ch_cour_appel_lucerne",
        "Tribunal cantonal (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_OG",
        "COUR_APPEL",
        "ch_cour_appel_lucerne_tribunal_superieur",
        "Tribunal supérieur (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_VWG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_lucerne",
        "Tribunal administratif (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_BZG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_lucerne",
        "Tribunaux de première instance (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "UR_OG",
        "COUR_APPEL",
        "ch_cour_appel_uri",
        "Tribunal supérieur (Uri, Suisse)",
        "geo:CH-UR",
    ),
    court(
        "SZ_KG",
        "COUR_APPEL",
        "ch_cour_appel_schwytz",
        "Tribunal cantonal (Schwytz, Suisse)",
        "geo:CH-SZ",
    ),
    court(
        "SZ_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_schwytz",
        "Tribunal administratif (Schwytz, Suisse)",
        "geo:CH-SZ",
    ),
    court(
        "NW_OG",
        "COUR_APPEL",
        "ch_cour_appel_nidwald",
        "Tribunal supérieur (Nidwald, Suisse)",
        "geo:CH-NW",
    ),
    court(
        "GL_OG",
        "COUR_APPEL",
        "ch_cour_appel_glaris",
        "Tribunal supérieur (Glaris, Suisse)",
        "geo:CH-GL",
    ),
    court(
        "GL_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_glaris",
        "Tribunal administratif (Glaris, Suisse)",
        "geo:CH-GL",
    ),
    court(
        "GL_KG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_glaris",
        "Tribunal cantonal (Glaris, Suisse)",
        "geo:CH-GL",
    ),
    court(
        "ZG_OG",
        "COUR_APPEL",
        "ch_cour_appel_zoug",
        "Tribunal supérieur (Zoug, Suisse)",
        "geo:CH-ZG",
    ),
    court(
        "ZG_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_zoug",
        "Tribunal administratif (Zoug, Suisse)",
        "geo:CH-ZG",
    ),
    court(
        "SO_OG",
        "COUR_APPEL",
        "ch_cour_appel_soleure",
        "Tribunal supérieur (Soleure, Suisse)",
        "geo:CH-SO",
    ),
    court(
        "SO_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_soleure",
        "Tribunal administratif (Soleure, Suisse)",
        "geo:CH-SO",
    ),
    court(
        "SO_VSG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_soleure_assurances",
        "Tribunal des assurances (Soleure, Suisse)",
        "geo:CH-SO",
    ),
    court(
        "SO_STG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_soleure_fiscal",
        "Tribunal fiscal (Soleure, Suisse)",
        "geo:CH-SO",
    ),
    court(
        "BS_APG",
        "COUR_APPEL",
        "ch_cour_appel_bale_ville",
        "Cour d'appel (Bâle-Ville, Suisse)",
        "geo:CH-BS",
    ),
    court(
        "BS_SVG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_bale_ville_assurances_sociales",
        "Tribunal des assurances sociales (Bâle-Ville, Suisse)",
        "geo:CH-BS",
    ),
    court(
        "BL_KG",
        "COUR_APPEL",
        "ch_cour_appel_bale_campagne",
        "Tribunal cantonal (Bâle-Campagne, Suisse)",
        "geo:CH-BL",
    ),
    court(
        "BL_SG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_bale_campagne_fiscal",
        "Tribunal fiscal (Bâle-Campagne, Suisse)",
        "geo:CH-BL",
    ),
    court(
        "BL_EG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_bale_campagne_expropriation",
        "Tribunal d'expropriation (Bâle-Campagne, Suisse)",
        "geo:CH-BL",
    ),
    court(
        "BL_ZMG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_bale_campagne_mesures_contrainte",
        "Tribunal des mesures de contrainte (Bâle-Campagne, Suisse)",
        "geo:CH-BL",
    ),
    court(
        "SH_OG",
        "COUR_APPEL",
        "ch_cour_appel_schaffhouse",
        "Tribunal supérieur (Schaffhouse, Suisse)",
        "geo:CH-SH",
    ),
    court(
        "AR_OG",
        "COUR_APPEL",
        "ch_cour_appel_appenzell_rhodes_exterieures",
        "Tribunal supérieur (Appenzell Rhodes-Extérieures, Suisse)",
        "geo:CH-AR",
    ),
    court(
        "AR_KG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_appenzell_rhodes_exterieures",
        "Tribunal cantonal (Appenzell Rhodes-Extérieures, Suisse)",
        "geo:CH-AR",
    ),
    court(
        "AI_KG",
        "COUR_APPEL",
        "ch_cour_appel_appenzell_rhodes_interieures",
        "Tribunal cantonal (Appenzell Rhodes-Intérieures, Suisse)",
        "geo:CH-AI",
    ),
    court(
        "AI_BZG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_appenzell_rhodes_interieures",
        "Tribunal de district (Appenzell Rhodes-Intérieures, Suisse)",
        "geo:CH-AI",
    ),
    court(
        "SG_KG",
        "COUR_APPEL",
        "ch_cour_appel_saint_gall",
        "Tribunal cantonal (Saint-Gall, Suisse)",
        "geo:CH-SG",
    ),
    court(
        "SG_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_saint_gall",
        "Tribunal administratif (Saint-Gall, Suisse)",
        "geo:CH-SG",
    ),
    court(
        "SG_VSG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_saint_gall_assurances",
        "Tribunal des assurances (Saint-Gall, Suisse)",
        "geo:CH-SG",
    ),
    court(
        "SG_HG",
        "TRIBUNAL_COMMERCE",
        "ch_tribunal_commerce_saint_gall",
        "Tribunal de commerce (Saint-Gall, Suisse)",
        "geo:CH-SG",
    ),
    court(
        "GR_KG",
        "COUR_APPEL",
        "ch_cour_appel_grisons",
        "Tribunal cantonal (Grisons, Suisse)",
        "geo:CH-GR",
    ),
    court(
        "GR_VG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_grisons",
        "Tribunal administratif (Grisons, Suisse)",
        "geo:CH-GR",
    ),
    court(
        "AG_OG",
        "COUR_APPEL",
        "ch_cour_appel_argovie",
        "Tribunal supérieur (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_SVWG",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_argovie_special",
        "Tribunal administratif spécial (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "TG_OG",
        "COUR_APPEL",
        "ch_cour_appel_thurgovie",
        "Tribunal supérieur (Thurgovie, Suisse)",
        "geo:CH-TG",
    ),
    court(
        "TI_TRAC",
        "COUR_APPEL",
        "ch_cour_appel_tessin_civil",
        "Tribunal d'appel, chambres civiles (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_TRAP",
        "COUR_APPEL",
        "ch_cour_appel_tessin_penal",
        "Tribunal d'appel, chambres pénales (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_CARP",
        "COUR_APPEL",
        "ch_cour_appel_tessin_revision_penale",
        "Cour d'appel et de révision pénale (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_CRP",
        "COUR_APPEL",
        "ch_cour_appel_tessin_recours_penal",
        "Chambre des recours pénale (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_TCA",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_tessin",
        "Tribunal cantonal administratif (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_TCAS",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_tessin_assurances",
        "Tribunal cantonal des assurances (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_CATI",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_tessin_fiscal",
        "Chambre de droit fiscal (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_TRPI",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_tessin_amenagement",
        "Tribunal de l'aménagement du territoire (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_TPC",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_tessin_penal",
        "Tribunal pénal cantonal (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_PP",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_tessin_preture_penale",
        "Préture pénale (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_GIAR",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_tessin_instruction",
        "Juge de l'instruction et de l'arrestation (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "TI_TE",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_tessin_expropriation",
        "Tribunal d'expropriation (Tessin, Suisse)",
        "geo:CH-TI",
    ),
    court(
        "CH_VB",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_federale",
        "Autorités administratives de la Confédération (Suisse)",
        "geo:CH",
    ),
    court(
        "CH_BR",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_conseil_federal",
        "Conseil fédéral (Suisse)",
        "geo:CH",
    ),
    court(
        "CH_WBK",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_concurrence",
        "Commission de la concurrence (Suisse)",
        "geo:CH",
    ),
    court(
        "CH_EDÖB",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_protection_donnees",
        "Préposé fédéral à la protection des données et à la transparence (Suisse)",
        "geo:CH",
    ),
    court(
        "BE_VB",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_berne",
        "Directions (Berne, Suisse)",
        "geo:CH-BE",
    ),
    court(
        "BE_AK",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_berne_avocats",
        "Autorité de surveillance des avocats (Berne, Suisse)",
        "geo:CH-BE",
    ),
    court(
        "BE_SRK",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_berne_fiscal",
        "Commission des recours en matière fiscale (Berne, Suisse)",
        "geo:CH-BE",
    ),
    court(
        "VD_TN",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_vaud_neutre",
        "Tribunal neutre (Vaud, Suisse)",
        "geo:CH-VD",
    ),
    court(
        "NE_ASL",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_neuchatel_surveillance_lp",
        "Autorité de surveillance LP (Neuchâtel, Suisse)",
        "geo:CH-NE",
    ),
    court(
        "NE_ASS",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_neuchatel_surveillance_superieure_lp",
        "Autorité supérieure de surveillance LP (Neuchâtel, Suisse)",
        "geo:CH-NE",
    ),
    court(
        "NE_ATS",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_neuchatel_tutelle",
        "Autorité tutélaire de surveillance (Neuchâtel, Suisse)",
        "geo:CH-NE",
    ),
    court(
        "NE_CA",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_neuchatel_avocats",
        "Autorité de surveillance des avocats (Neuchâtel, Suisse)",
        "geo:CH-NE",
    ),
    court(
        "NE_ARA",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_neuchatel_recours_avocats_notariat",
        "Autorité de recours des avocats et du notariat (Neuchâtel, Suisse)",
        "geo:CH-NE",
    ),
    court(
        "NE_CN",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_neuchatel_notariat",
        "Autorité de surveillance du notariat (Neuchâtel, Suisse)",
        "geo:CH-NE",
    ),
    court(
        "LU_RR",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_lucerne_conseil_etat",
        "Conseil d'État (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_JSD",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_lucerne_justice_securite",
        "Département de la justice et de la sécurité (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_GSD",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_lucerne_sante_social",
        "Département de la santé et des affaires sociales (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_BKD",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_lucerne_education_culture",
        "Département de l'éducation et de la culture (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_AVW",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_lucerne",
        "Autres autorités administratives (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_AUK",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_lucerne_surveillance",
        "Autorités de surveillance et commissions (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "LU_SA",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_lucerne_arbitral_assurances_sociales",
        "Tribunal arbitral des assurances sociales (Lucerne, Suisse)",
        "geo:CH-LU",
    ),
    court(
        "SO_SK",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_soleure_estimation",
        "Commission d'estimation (Soleure, Suisse)",
        "geo:CH-SO",
    ),
    court(
        "SG_VWEK",
        "TRIBUNAL_ADMINISTRATIF",
        "ch_tribunal_administratif_saint_gall_recours",
        "Commission de recours administrative (Saint-Gall, Suisse)",
        "geo:CH-SG",
    ),
    court(
        "SG_DEP",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_saint_gall",
        "Départements (Saint-Gall, Suisse)",
        "geo:CH-SG",
    ),
    court(
        "SG_ABSK",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_saint_gall_poursuites_faillites",
        "Autorité de surveillance en matière de poursuite et de faillite (Saint-Gall, Suisse)",
        "geo:CH-SG",
    ),
    court(
        "AG_RR",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_argovie_conseil_etat",
        "Conseil d'État (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_BV",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_argovie_construction_transports_environnement",
        "Département de la construction, des transports et de l'environnement (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_VB",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_argovie",
        "Départements (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_BG",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_argovie_construction",
        "Décisions en droit de la construction (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_GB",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_argovie_registre_foncier_notariat",
        "Registre foncier et notariat (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_AK",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_argovie_avocats",
        "Commission des avocats (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_AUK",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_argovie_surveillance",
        "Commission de surveillance (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_JL",
        "AUTORITE_ADMINISTRATIVE",
        "ch_autorite_administrative_argovie_direction_justice",
        "Direction de la justice (Argovie, Suisse)",
        "geo:CH-AG",
    ),
    court(
        "AG_JG",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ch_tribunal_premiere_instance_argovie_justice",
        "Tribunal de la justice (Argovie, Suisse)",
        "geo:CH-AG",
    ),
];

/// The Valais cantonal court, whose decisions the index files with the district courts' under
/// `VS_BZG`.
pub static VS_TC: SwissCourt = court(
    "VS_TC",
    "COUR_APPEL",
    "ch_cour_appel_valais",
    "Tribunal cantonal (Valais, Suisse)",
    "geo:CH-VS",
);

pub fn swiss_court(id: &str) -> Option<&'static SwissCourt> {
    SWISS_COURTS
        .iter()
        .chain(std::iter::once(&VS_TC))
        .find(|c| c.id == id)
}

/// The first court a Valais text names, the cantonal court or a district.
static VALAIS_COURT_NAMED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:ATC|KGE)\b|(?i:tribunal cantonal|kantonsgericht|autorité (?:de recours|supérieure|cantonale)|aufsichtsbehörde|gerichtshof|\b(?P<district>districts?|bezirk\w*)\b)",
    )
    .expect("VALAIS_COURT_NAMED compiles")
});

/// Whether a document the index files under the Valais district courts is the cantonal court's,
/// by the code its docket opens on: one of the cantonal court's, `C1 03 158`, `F1 24 57`, or its
/// lawyers' supervisory authority's, `ZZ`. A debt enforcement docket, `LP`, is the cantonal
/// court's when its text names that court before any district. A district court's opens on `Z1`
/// or `BK`, the labour court's on `TR`.
fn valais_cantonal(docket: &str, text: &str) -> bool {
    match docket
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .as_bytes()
    {
        [b'A' | b'C' | b'P' | b'S' | b'F', b'1'..=b'3'] | b"ZZ" => true,
        b"LP" => {
            let head: String = text.chars().take(800).collect();
            VALAIS_COURT_NAMED
                .captures(&head)
                .is_some_and(|c| c.name("district").is_none())
        }
        _ => false,
    }
}

#[derive(Debug)]
pub struct EntscheidsucheParsed {
    pub decision: Decision,
    pub source_fields: Value,
    pub court: &'static SwissCourt,
}

/// The non-blank characters a decision text holds at least.
const DECISION_FLOOR: usize = 200;

fn below_floor(text: &str) -> bool {
    text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR
}

/// Whether the document is a PDF the index read no decision text from, a scan or a file whose
/// fonts map no glyph to a character, which only an OCR of the file reads.
pub fn entscheidsuche_scan(doc: &Value) -> bool {
    let content_type = text_field(doc, &["attachment", "content_type"]);
    content_type == Some("application/pdf")
        && below_floor(&clean_text(
            text_field(doc, &["attachment", "content"]).unwrap_or_default(),
            content_type,
        ))
}

fn text_field<'a>(doc: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(doc, |v, k| v.get(k))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The language whose function words the text holds most of, as a three-letter code: the index
/// labels some decisions written in Italian as French, and some French ones as Lithuanian.
pub(crate) fn language(text: &str) -> &'static str {
    const WORDS: [(&str, [&str; 12]); 3] = [
        (
            "fra",
            [
                "le", "les", "du", "et", "que", "qui", "pour", "dans", "est", "par", "sur", "aux",
            ],
        ),
        (
            "deu",
            [
                "der", "die", "das", "und", "nicht", "ist", "mit", "von", "zu", "den", "dem", "auf",
            ],
        ),
        (
            "ita",
            [
                "della", "che", "per", "del", "di", "non", "una", "gli", "nel", "alla", "sono",
                "delle",
            ],
        ),
    ];
    let mut counts = [0usize; 3];
    for word in text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .take(5_000)
    {
        let word = word.to_lowercase();
        if let Some(i) = WORDS.iter().position(|(_, w)| w.contains(&word.as_str())) {
            counts[i] += 1;
        }
    }
    let best = (0..3).fold(0, |best, i| if counts[i] > counts[best] { i } else { best });
    WORDS[best].0
}

/// A PDF's text back into paragraphs; an HTML's already holds one per line.
fn clean_text(content: &str, content_type: Option<&str>) -> String {
    // An original file can carry a NUL, which Postgres refuses in a text column.
    let content = content.replace('\0', "");
    if content_type == Some("application/pdf") {
        reflow_page_text(&content)
    } else {
        plain_text(&content)
    }
}

/// A text read from a page, its lines trimmed and its blank lines dropped.
pub(crate) fn plain_text(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn decision_of(
    source_uid: String,
    id: &str,
    court: &SwissCourt,
    chamber: Option<String>,
    date: String,
    dockets: Vec<String>,
    text: String,
) -> Decision {
    Decision {
        source_uid,
        member_name: id.to_string(),
        ecli: None,
        jurisdiction_source_code: Some(court.code.to_string()),
        chamber,
        nac: None,
        jurisdiction_name: Some(court.label.to_string()),
        jurisdiction_type: Some(court.jurisdiction_type.to_string()),
        jurisdiction_location: None,
        numero_dossier: dockets.first().cloned(),
        numero_dossiers: (!dockets.is_empty()).then_some(dockets),
        numero_role: None,
        date_lecture: Some(date),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: text.clone(),
        texte_integral_clean: text,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

fn dockets_of(source_fields: &Value) -> Vec<String> {
    let mut dockets: Vec<String> = Vec::new();
    for reference in source_fields
        .get("references")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        if !dockets.iter().any(|d| d == reference) {
            dockets.push(reference.to_string());
        }
    }
    dockets
}

/// A line that is a decision's date and nothing else: the closing line of an entry of a yearly
/// report, its docket first or last (`ZK2 14 18 Urteil vom 16. September 2014`, `Urteil vom 19.
/// Dezember 2019 ZK2 19 77`, `R 08 104 Sentenza del 20 gennaio 2009`), or the heading of a
/// decision (`ARRÊT DU 19 JANVIER 2021`), a tag after it (`[rechtskräftig]`), and the `in Sachen`
/// a PDF's reflow joins to it. A judgment deliberated over two days takes the second, `vom 7./8.
/// Mai 2003`, `vom 18. April 2005/4. Juli 2005`, `del 22 e 29 agosto 2017`.
static DATE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[ \t]*(?P<docket>[A-Z][A-Za-z]{0,5}(?:[ \t]?\d)?(?:[ \t]+\d{2}[ \t]+\d+|[ \t]*\d+/\d+)[^\n]{0,40}?)?[ \t]*(?i:(?:teil|zwischen|end)?(?:urteil|beschluss|entscheid|verfügung)|sentenza|sentenzia|decisione|decreto|giudizio|arr[êe]t|d[ée]cision|jugement|ordonnance)[ \t]+(?i:vom|del|dal|dals|dils|dell[’‘']|della|du)[ \t]*(?:\d{1,2}\.?(?:[ \t]*\p{L}+)?(?:[ \t]+\d{4})?[ \t]*(?:/|e|und|et)[ \t]*)?(?P<day>\d{1,2})(?:\.|er)?[ \t]*(?:(?:da|d[’‘'])[ \t]*)?(?P<month>\p{L}+)[ \t]+(?P<year>\d{4})(?:[ \t]+(?P<docket_after>[A-Z][A-Za-z]{0,5}(?:[ \t]?\d)?[ \t]+\d{2}[ \t]+\d+))?(?:[ \t]*\[[^\]\n]{1,40}\])?(?:[ \t]+in[ \t]+Sachen)?[ \t]*$",
    )
    .expect("DATE_LINE compiles")
});

/// A date line whose year fell on the next line, `RB 28/95 Urteil vom 24. August` then `1995`.
static YEAR_BELOW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)(?P<head>(?i:vom|del|dal|dals|dils|du)[ \t]*\d{1,2}\.?[ \t]*\p{L}+)[ \t]*\n(?P<year>\d{4})[ \t]*$",
    )
    .expect("YEAR_BELOW compiles")
});

/// A report entry's close, its docket first, that a PDF's reflow ran into the next entry's title:
/// `ZB 32/94 Urteil vom 7. September 1994/17. Oktober 1994 Garantie des Wohnsitzgerichtsstandes`.
static CLOSE_RUN_ON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[ \t]*(?P<close>(?P<docket>[A-Z][A-Za-z]{0,5}(?:[ \t]?\d)?(?:[ \t]+\d{2}[ \t]+\d+|[ \t]*\d+/\d+))[ \t]*(?i:urteil|beschluss|entscheid|verfügung)[ \t]+vom[ \t]*(?:\d{1,2}\.?(?:[ \t]*\p{L}+)?(?:[ \t]+\d{4})?[ \t]*/[ \t]*)?(?P<day>\d{1,2})\.[ \t]*(?P<month>\p{L}+)[ \t]+(?P<year>\d{4}))[ \t]+\p{Lu}",
    )
    .expect("CLOSE_RUN_ON compiles")
});

/// The docket of an entry of a Grisons court's yearly report, `PKG 1996 24`, `PVG 2002 33`.
static REPORT_ENTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^P[KV]G \d{4} \d+$").expect("REPORT_ENTRY compiles"));

/// The place and day a French decision is signed on, its clerk's initials after it: `Porrentruy,
/// le 19 mars 2026/bc`.
static SIGNED_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[ \t]*\p{Lu}[\p{L}\- ]{1,40},[ \t]+le[ \t]+(?P<day>\d{1,2})(?:er)?[ \t]+(?P<month>\p{L}+)[ \t]+(?P<year>\d{4})(?:[ \t]*/[ \t]*\p{L}{1,6})?[ \t]*$",
    )
    .expect("SIGNED_LINE compiles")
});

/// The months' names in German, Italian, French and Romansh, January first, misspellings the
/// reports carry included.
const MONTHS: [&[&str]; 12] = [
    &["januar", "gennaio", "janvier", "schaner"],
    &["februar", "febbraio", "février", "fevrier", "favrer"],
    &["märz", "marzo", "mars"],
    &["april", "aprile", "avril", "avrigl"],
    &["mai", "maggio", "matg"],
    &["juni", "giugno", "juin", "zercladur"],
    &["juli", "luglio", "juillet", "fanadur"],
    &["august", "agosto", "août", "aout", "avust"],
    &["september", "settembre", "septembre", "settember"],
    &["oktober", "ottobre", "octobre", "october"],
    &["november", "novembre"],
    &["dezember", "dicembre", "décembre", "decembre", "december"],
];

fn month_number(month: &str) -> Option<i8> {
    let month = month.to_lowercase();
    let index = MONTHS
        .iter()
        .position(|names| names.contains(&month.as_str()))?;
    i8::try_from(index + 1).ok()
}

/// Whether a text states this day, in words (`7. September 1994`, `1er juillet 2010`,
/// `dell'11 giugno 2021`, `28 da zercladur 2016`) or in figures (`07.09.1994`).
pub fn states_day(text: &str, day: jiff::civil::Date) -> bool {
    let (d, m, y) = (day.day(), day.month(), day.year());
    let names = MONTHS[usize::from(m.unsigned_abs()) - 1].join("|");
    Regex::new(&format!(
        r"(?i)(?:^|\D)0?{d}(?:\.|er)?\s*(?:(?:da|d[’‘'])\s*)?(?:{names})\s+{y}(?:\D|$)|(?:^|\D)0?{d}\.\s*0?{m}\.\s*{y}(?:\D|$)"
    ))
    .expect("a day's pattern compiles")
    .is_match(text)
}

fn captured_date(c: &regex::Captures) -> Option<jiff::civil::Date> {
    jiff::civil::Date::new(
        c["year"].parse().ok()?,
        month_number(&c["month"])?,
        c["day"].parse().ok()?,
    )
    .ok()
}

/// The date an undated document states on a line of its own, with that line's docket. A report's
/// entry takes its last such line, which closes it: its text opens on the close of the entry
/// before, docket included, and its own close may have lost its docket over the lines above. Any
/// other document takes the last line naming a docket, else the first line without one, which
/// heads a decision, else the last line it is signed on.
pub(crate) fn date_line(
    text: &str,
    report_entry: bool,
) -> Option<(jiff::civil::Date, Option<String>, String)> {
    let text = YEAR_BELOW.replace_all(text, "$head $year");
    let line_of = |c: regex::Captures| {
        let date = captured_date(&c)?;
        let docket = c
            .name("docket")
            .or_else(|| c.name("docket_after"))
            .map(|d| d.as_str().trim().to_string())
            .filter(|d| d.chars().any(|ch| ch.is_ascii_digit()));
        let line = c.name("close").or_else(|| c.get(0))?;
        Some((
            line.start(),
            (date, docket, line.as_str().trim().to_string()),
        ))
    };
    let mut at: Vec<_> = DATE_LINE.captures_iter(&text).filter_map(line_of).collect();
    if report_entry {
        at.extend(CLOSE_RUN_ON.captures_iter(&text).filter_map(line_of));
        at.sort_by_key(|(start, _)| *start);
    }
    let dated: Vec<_> = at.into_iter().map(|(_, line)| line).collect();
    let chosen = if report_entry {
        dated.last()
    } else {
        dated
            .iter()
            .rev()
            .find(|(_, docket, _)| docket.is_some())
            .or_else(|| dated.first())
    };
    chosen.cloned().or_else(|| {
        SIGNED_LINE
            .captures_iter(&text)
            .filter_map(|c| Some((captured_date(&c)?, None, c[0].trim().to_string())))
            .last()
    })
}

/// Version of [`parse_entscheidsuche`], which the catch-up keys a refused document on beside its
/// scrape day: bump it when the parse comes to read documents it refused.
pub const ENTSCHEIDSUCHE_PARSE_VERSION: u32 = 2;

/// Parses a document's `_source`, its chamber named as the index's facets name it. `original`
/// is the text of the file the index extracted it from, read when the index cut its text short
/// or read none of an [`entscheidsuche_scan`].
pub fn parse_entscheidsuche(
    raw: &str,
    chamber: Option<&str>,
    original: Option<&str>,
) -> Result<EntscheidsucheParsed> {
    let fail = |id: &str, why: String| CoreError::Xml(format!("entscheidsuche {id}: {why}"));
    let doc: Value =
        serde_json::from_str(raw).map_err(|e| CoreError::Xml(format!("entscheidsuche: {e}")))?;
    let id = text_field(&doc, &["id"]).ok_or_else(|| fail("?", "no id".into()))?;
    let hierarchy: Vec<&str> = doc
        .get("hierarchy")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let court_id = hierarchy
        .get(1)
        .ok_or_else(|| fail(id, "no court in the hierarchy".into()))?;
    let court =
        swiss_court(court_id).ok_or_else(|| fail(id, format!("court {court_id} not taken")))?;
    let content_type = text_field(&doc, &["attachment", "content_type"]);
    let text = clean_text(
        original
            .or_else(|| text_field(&doc, &["attachment", "content"]))
            .unwrap_or_default(),
        content_type,
    );
    if below_floor(&text) {
        return Err(fail(id, "no decision text".into()));
    }
    let court = match doc.pointer("/reference/0").and_then(Value::as_str) {
        Some(docket) if court.id == "VS_BZG" && valais_cantonal(docket, &text) => &VS_TC,
        _ => court,
    };
    let mut references = doc.get("reference").cloned().unwrap_or(json!([]));
    let (date, date_line) = match text_field(&doc, &["date"]) {
        Some(date) => {
            date.parse::<jiff::civil::Date>()
                .map_err(|e| fail(id, format!("date {date:?}: {e}")))?;
            (date.to_string(), None)
        }
        None => {
            let report_entry = references
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|r| REPORT_ENTRY.is_match(r));
            let (date, docket, line) =
                date_line(&text, report_entry).ok_or_else(|| fail(id, "no date".into()))?;
            if let (Some(docket), Some(refs)) = (docket, references.as_array_mut()) {
                refs.push(json!(docket));
            }
            (date.to_string(), Some(line))
        }
    };
    let source_fields = json!({
        "entscheidsuche_id": id,
        "court": court.id,
        "chamber_id": hierarchy.get(2),
        "chamber": chamber,
        "date": date,
        "date_line": date_line,
        "references": references,
        "title": text_field(&doc, &["title", "fr"]),
        "abstract": text_field(&doc, &["abstract", "fr"]),
        "scrapedate": text_field(&doc, &["scrapedate"]),
        "content_type": content_type,
        "source_url": text_field(&doc, &["attachment", "content_url"]),
        "text_from_original": original.is_some(),
        "lang": language(&text),
    });
    let decision = decision_of(
        format!("entscheidsuche/{id}"),
        id,
        court,
        chamber.map(str::to_string),
        date,
        dockets_of(&source_fields),
        text,
    );
    Ok(EntscheidsucheParsed {
        decision,
        source_fields,
        court,
    })
}

/// Branch of [`Decision::from_source_fields`] for `entscheidsuche/`, on what the parse stored.
pub(crate) fn from_source_fields_entscheidsuche(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let id = source_uid
        .strip_prefix("entscheidsuche/")
        .unwrap_or(source_uid);
    let court = field("court")
        .and_then(swiss_court)
        .unwrap_or_else(|| panic!("stored entscheidsuche court not taken ({source_uid})"));
    let date = field("date")
        .unwrap_or_else(|| panic!("source_fields entscheidsuche without a date ({source_uid})"));
    decision_of(
        source_uid.to_string(),
        id,
        court,
        field("chamber").map(str::to_string),
        date.to_string(),
        dockets_of(source_fields),
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A court is named in its canton's language when the index has no other, a federal court
    /// with the country alone, and a chamber only where its name differs from the French.
    #[test]
    fn a_court_is_named_with_its_canton_and_its_chambers() {
        let facets = json!({
            "CH": {"de": "Eidgenossenschaft", "fr": "Conféderation", "it": "Confederazione",
                "gerichte": {"CH_BGer": {"de": "Bundesgericht", "fr": "Tribunal fédéral", "it": "Tribunale federale",
                    "kammern": {"CH_BGer_004": {"de": "I. Zivilrechtliche Abteilung", "fr": "Ire Cour de droit civil", "it": "I Corte di diritto civile"}}}}},
            "BS": {"de": "Basel-Stadt", "fr": "Bâle-Ville", "it": "Basilea Città",
                "gerichte": {"BS_APG": {"de": "Appellationsgericht", "fr": "Appellationsgericht", "it": "Appellationsgericht",
                    "kammern": {"BS_APG_001": {"de": "Appellationsgericht", "fr": "Appellationsgericht", "it": "Appellationsgericht"}}}}},
            "GE": {"de": "Genf", "fr": "Genève", "it": "Ginevra",
                "gerichte": {"GE_CJ": {"de": "Cour de Justice (Cour civile)", "fr": "Cour de Justice (Cour civile)", "it": "Cour de Justice (Cour civile)"}}},
            "VS": {"de": "Wallis", "fr": "Valais", "it": "Vallese",
                "gerichte": {"VS_TC": {"de": "Kantonsgericht", "fr": "Tribunal cantonal", "it": "Kantonsgericht"}}}
        });
        let names: std::collections::HashMap<_, _> = court_names(&facets).into_iter().collect();
        assert_eq!(
            names["ch_cour_supreme"]["de"],
            json!({"name": "Bundesgericht (Schweiz)", "chambers": {"Ire Cour de droit civil": "I. Zivilrechtliche Abteilung"}})
        );
        assert_eq!(
            names["ch_cour_appel_bale_ville"]["it"],
            json!({"name": "Appellationsgericht (Basilea Città, Svizzera)"})
        );
        assert_eq!(
            names["ch_cour_appel_geneve"]["de"],
            json!({"name": "Cour de justice (Genf, Schweiz)"})
        );
        assert_eq!(
            names["ch_cour_appel_valais"]["de"],
            json!({"name": "Kantonsgericht (Wallis, Schweiz)"})
        );
        assert_eq!(names.len(), 4);
    }

    fn document(court: &str, text: &str) -> String {
        json!({
            "id": format!("{court}_001_PE19-021590_2026-05-06"),
            "date": "2026-05-06",
            "hierarchy": ["VD", court, format!("{court}_001")],
            "reference": ["PE19.021590", " ", "PE19.021590"],
            "title": {"fr": "Vaud Tribunal cantonal Cour d'appel pénale 06.05.2026 PE19.021590"},
            "scrapedate": "2026-07-15",
            "attachment": {"content": text, "content_type": "text/html"},
        })
        .to_string()
    }

    const FRENCH: &str = "La Cour d'appel pénale considère que le prévenu a été condamné par le \
        tribunal de police pour des faits qui sont établis dans les pièces du dossier. Il fait \
        valoir sur ce point que la peine est excessive et que le premier juge a mal apprécié les \
        circonstances. Par ces motifs, la Cour rejette l'appel et confirme le jugement entrepris \
        dans toutes ses dispositions, les frais étant mis à la charge de l'appelant.";

    const ITALIAN: &str = "La Corte di appello civile ritiene che il ricorso non sia fondato. \
        Il ricorrente sostiene che la decisione della pretura sia arbitraria e che il giudice di \
        prime cure non abbia considerato le prove. Per questi motivi la Corte respinge il ricorso \
        e conferma la sentenza impugnata, le spese della procedura sono poste a carico del \
        ricorrente che non ha ottenuto ragione nelle sue conclusioni.";

    #[test]
    fn a_cantonal_decision_is_placed_in_its_canton_with_its_dockets_once() {
        let parsed = parse_entscheidsuche(
            &document("VD_TC", FRENCH),
            Some("Cour d'appel pénale"),
            None,
        )
        .unwrap();
        assert_eq!(parsed.court.place, "geo:CH-VD");
        let d = &parsed.decision;
        assert_eq!(
            d.source_uid,
            "entscheidsuche/VD_TC_001_PE19-021590_2026-05-06"
        );
        assert_eq!(
            d.jurisdiction_source_code.as_deref(),
            Some("ch_cour_appel_vaud")
        );
        assert_eq!(d.numero_dossiers, Some(vec!["PE19.021590".to_string()]));
        assert_eq!(d.date_lecture.as_deref(), Some("2026-05-06"));
        assert_eq!(d.chamber.as_deref(), Some("Cour d'appel pénale"));
        let rebuilt = from_source_fields_entscheidsuche(
            &d.texte_integral_clean,
            &parsed.source_fields,
            &d.source_uid,
        );
        assert_eq!(rebuilt.numero_dossiers, d.numero_dossiers);
        assert_eq!(rebuilt.jurisdiction_type, d.jurisdiction_type);
    }

    #[test]
    fn a_text_is_read_in_its_own_language_whatever_the_index_labels_it() {
        let parsed = parse_entscheidsuche(&document("TI_TRAC", ITALIAN), None, None).unwrap();
        assert_eq!(parsed.source_fields["lang"], "ita");
        let parsed = parse_entscheidsuche(&document("VD_TC", FRENCH), None, None).unwrap();
        assert_eq!(parsed.source_fields["lang"], "fra");
    }

    #[test]
    fn an_undated_report_entry_takes_the_day_of_its_closing_line() {
        let entry = "PKG 2002 5\n\n5 - Feststellung des Streitbetrages. Klageänderung.\n\
            Das Bundesgericht hat mit Urteil vom 4. September 1996 entschieden.\n\
            Die Berufung ist abzuweisen.\nVB 4/96 Urteil vom 15. Mai 1996\n\
            (Eine gegen dieses Urteil erhobene Beschwerde wurde mit Urteil vom 4. September \n1996 abgewiesen.)\n173";
        let (date, docket, line) = date_line(entry, true).unwrap();
        assert_eq!(date.to_string(), "1996-05-15");
        assert_eq!(docket.as_deref(), Some("VB 4/96"));
        assert_eq!(line, "VB 4/96 Urteil vom 15. Mai 1996");
        let (date, docket, _) =
            date_line("ricorrente.\nR 08 19 Sentenza dell’11 aprile 2008\n", true).unwrap();
        assert_eq!(date.to_string(), "2008-04-11");
        assert_eq!(docket.as_deref(), Some("R 08 19"));
        let (date, _, _) = date_line(
            "Abweisung.\nZK2 19 43 Teilurteil vom 15. Februar 2022",
            true,
        )
        .unwrap();
        assert_eq!(date.to_string(), "2022-02-15");
    }

    #[test]
    fn an_undated_decision_takes_the_day_of_its_heading() {
        let heading = "COUR ADMINISTRATIVE\n\nADM 4 / 2021\n\nARRET DU 19 JANVIER 2021\n\n\
            relatif à la requête introduite le 3 janvier 2021\nDÉCISION DU 2 MARS 2022\n";
        let (date, docket, _) = date_line(heading, false).unwrap();
        assert_eq!(date.to_string(), "2021-01-19");
        assert_eq!(docket, None);
        assert!(date_line(
            "Le recours est rejeté.\nS 09 106 Sentenza dell’ 111 settembre 2009",
            true
        )
        .is_none());
        let (date, docket, _) = date_line(
            "insoweit nicht einzutreten.\n\nUrteil vom 19. Dezember 2019 ZK2 19 77\n\nPKG 2020",
            true,
        )
        .unwrap();
        assert_eq!(date, jiff::civil::date(2019, 12, 19));
        assert_eq!(docket.as_deref(), Some("ZK2 19 77"));
        let (date, docket, _) =
            date_line("la notification de la présente décision aux parties.\n\nPorrentruy, le 19 mars 2026/bc\n\nBruno Carnazzi", false)
                .unwrap();
        assert_eq!(date, jiff::civil::date(2026, 3, 19));
        assert_eq!(docket, None);
        let (date, docket, _) = date_line(
            "Beschwerde ist abzuweisen.\nSB 03 5 Urteil vom 7./ 8. Mai 2003",
            true,
        )
        .unwrap();
        assert_eq!(date, jiff::civil::date(2003, 5, 8));
        assert_eq!(docket.as_deref(), Some("SB 03 5"));
        let (date, _, _) = date_line(
            "refusà.\nR 13 182 Sentenzia dals 24 da zercladur 2014",
            true,
        )
        .unwrap();
        assert_eq!(date, jiff::civil::date(2014, 6, 24));
        let (date, _, _) = date_line(
            "constituziunals.\nV 12 7 Sentenzia dals 9 d’avust 2012",
            true,
        )
        .unwrap();
        assert_eq!(date, jiff::civil::date(2012, 8, 9));
    }

    #[test]
    fn a_pdf_the_index_read_no_text_from_goes_to_the_ocr() {
        let doc = |content_type: &str, content: &str| json!({"attachment": {"content_type": content_type, "content": content}});
        // GE_CJ_003_C-28219-2004_2006-06-22: the index read the letterhead and the page numbers.
        let numbers = "RÉPUBLIQUE ET CANTON DE GENÈVE\n\nPOUVOIR  JUDICIAIRE\n\n( )\n\n2\n\n• \n\n3\n\n4\n\n13";
        assert!(entscheidsuche_scan(&doc("application/pdf", "")));
        assert!(entscheidsuche_scan(&doc("application/pdf", numbers)));
        assert!(!entscheidsuche_scan(&doc("text/html; charset=UTF-8", "")));
        let decision = "Le recours est rejeté dans la mesure où il est recevable. ".repeat(5);
        assert!(!entscheidsuche_scan(&doc("application/pdf", &decision)));
    }

    #[test]
    fn a_text_states_a_day_in_words_or_in_figures() {
        let day = jiff::civil::date(2021, 6, 11);
        for text in [
            "A 21 8 sentenza dell‘11 giugno 2021",
            "Urteil vom 11. Juni\n2021",
            "Chur, 11.06.2021",
            "arrêt du 11 juin 2021",
        ] {
            assert!(states_day(text, day), "{text}");
        }
        for text in [
            "Urteil vom 1. Juni 2021",
            "Urteil vom 11. Juni 2020",
            "111.06.2021",
        ] {
            assert!(!states_day(text, day), "{text}");
        }
        assert!(states_day(
            "Lugano, 1er juillet 2010",
            jiff::civil::date(2010, 7, 1)
        ));
    }

    #[test]
    fn a_report_entry_takes_its_own_closing_line_after_the_entry_before() {
        // 18 PKG entries of 1994 to 1998 move to their own day. PKG 1995 25 moves off it: its text
        // runs on over the whole of entry 26, laid out as PKG 1995 26 below, and takes its day.
        let cases = [
            ("PKG 1996 24", "SKG 96\n46\nUrteil vom 16. Dezember 1996\nZum Verhältnis zwischen Anerkennungsklage und Rechtsöffnung.\nSKG 96\n42\nUrteil vom 16. Oktober 1996\n109", true, Some((1996, 10, 16))),
            ("PKG 1995 31", "SB 84/94 Urteil vom 11. Januar 1995\nStrafprozess; Anforderungen an das Urteil.\nUrteil vom 8. August 1995\n32 -\n120", true, Some((1995, 8, 8))),
            ("PKG 1995 26", "RB 31/95 Urteil vom 12. September 1995\nRechtsöffnung; örtliche Zuständigkeit.\nRB 28/95 Urteil vom 24. August\n1995\n26 -\n103", true, Some((1995, 8, 24))),
            ("PKG 1994 23", "ZB 35/94 Urteil vom 7. September 1994\n23 - Regress des Unfallversicherers.\nZB 32/94 Urteil vom 7. September 1994/17. Oktober 1994 Garantie des Wohnsitzgerichtsstandes (Art. 59 BV).", true, Some((1994, 10, 17))),
            ("PKG 2010 19", "Abweisung.\nERZ 09 90 Verfügung vom 3. Dezember 2009 /18. Januar 2010", true, Some((2010, 1, 18))),
            ("PVG 2014 31", "Abweisung.\nU 13 92 e U 12 125 Urteil vom 4. September / 20. November 2014", true, Some((2014, 11, 20))),
            ("PVG 2017 32", "respinto.\nU 17 38 Sentenza del 22 e 29 agosto  2017", true, Some((2017, 8, 29))),
            ("PVG 2021 6", "negato.\nA 21 8 sentenza dell‘11 giugno 2021", true, Some((2021, 6, 11))),
            ("PVG 2016 15", "abolidas.\nR 15 98 Sentenzia dils 28 da zercladur 2016", true, Some((2016, 6, 28))),
            ("Z2 2021 15", "Obergericht des Kantons Zug\nUrteil vom 16. Februar 2022 [rechtskräftig] in Sachen", false, Some((2022, 2, 16))),
            ("Z2 2018 33", "Gerichtsschreiberin\nTeilurteil vom 19. Mai 2022 in Sachen", false, Some((2022, 5, 19))),
            ("PVG 2010 18", "Abweisung.\nA 10 16 Urteil vom 111 . Mai 2010", true, None),
        ];
        for (docket, text, report_entry, day) in cases {
            assert_eq!(
                date_line(text, report_entry).map(|(d, _, _)| (d.year(), d.month(), d.day())),
                day,
                "{docket}"
            );
        }
    }

    #[test]
    fn an_undated_document_without_a_date_line_is_refused() {
        let doc = json!({
            "id": "GR_KG_001_PKG-2002-5_0000-00-00",
            "hierarchy": ["GR", "GR_KG", "GR_KG_001"],
            "reference": ["PKG 2002 5"],
            "attachment": {"content": FRENCH, "content_type": "application/pdf"},
        });
        assert!(parse_entscheidsuche(&doc.to_string(), None, None).is_err());
        let dated = format!("{FRENCH}\nZK2 14 18 Urteil vom 16. September 2014");
        let doc = json!({
            "id": "GR_KG_001_PKG-2014-9_0000-00-00",
            "hierarchy": ["GR", "GR_KG", "GR_KG_001"],
            "reference": ["PKG 2014 9"],
            "attachment": {"content": dated, "content_type": "text/html"},
        });
        let parsed = parse_entscheidsuche(&doc.to_string(), None, None).unwrap();
        assert_eq!(parsed.decision.date_lecture.as_deref(), Some("2014-09-16"));
        assert_eq!(
            parsed.decision.numero_dossiers,
            Some(vec!["PKG 2014 9".to_string(), "ZK2 14 18".to_string()])
        );
    }

    #[test]
    fn sport_arbitration_is_refused() {
        assert!(parse_entscheidsuche(&document("TA_SST", FRENCH), None, None).is_err());
    }

    #[test]
    fn an_authority_is_typed_apart_from_the_courts() {
        let parsed = parse_entscheidsuche(&document("CH_WBK", FRENCH), None, None).unwrap();
        assert_eq!(parsed.court.jurisdiction_type, "AUTORITE_ADMINISTRATIVE");
    }

    #[test]
    fn a_valais_decision_goes_to_the_court_its_docket_names() {
        let court = |docket: &str, text: &str| {
            let mut doc: Value = serde_json::from_str(&document("VS_BZG", text)).unwrap();
            doc["reference"] = json!([docket]);
            parse_entscheidsuche(&doc.to_string(), None, None)
                .unwrap()
                .court
                .id
        };
        assert_eq!(court("C1 03 158", FRENCH), "VS_TC");
        assert_eq!(court("ZZ 09 7", FRENCH), "VS_TC");
        assert_eq!(court("Z1 07 106", FRENCH), "VS_BZG");
        assert_eq!(court("TR 16 518", FRENCH), "VS_BZG");
        let district =
            format!("LP 14 820 DÉCISION DU 5 FÉVRIER 2015 Tribunal du district de Sion. {FRENCH}");
        assert_eq!(court("LP 14 820", &district), "VS_BZG");
        let cantonal = format!("ATC (Autorité supérieure de surveillance en matière de LP) du 24 avril 2006, recours contre le juge du district. {FRENCH}");
        assert_eq!(court("LP 06 13", &cantonal), "VS_TC");
    }
}
