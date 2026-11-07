//! A record projected into observations: one row per poste read, with its
//! abscissa when it has one and the reason for its absence otherwise, then one
//! row per tariff the poste writes lower down, in a component or in a period.
//!
//! What the referential publishes against it is not here: the gap depends on
//! the edition it is measured at, and is computed when read.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::spreadsheet::{Proof, Spreadsheet};
use super::{
    Bareme, Composante, Correction, Lien, ModeEmploi, NatureAide, Poste, Quantum, Regime, Tranche,
    Unite, Victime,
};

/// Les quatre natures d'abscisse, qui ne se mélangent pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Valeur du point de DFP : une cellule d'un croisement taux par âge, donc
    /// ce qui s'y mesure est l'écart à la cellule, jamais la médiane des cellules.
    Grille,
    /// Un tarif par jour, par heure, par mois, dont la médiane a un sens.
    Unite,
    /// Une note sur 7, sous-niveau de la nomenclature.
    Cotation,
    /// Le préjudice d'affection, indexé sur le lien découpé par ses modalités.
    Categorie,
}

impl Axis {
    pub fn name(self) -> &'static str {
        match self {
            Self::Grille => "grille",
            Self::Unite => "unite",
            Self::Cotation => "cotation",
            Self::Categorie => "categorie",
        }
    }
}

/// Which cell a row belongs to, without rereading the ruling. The rate is the
/// direct victim's even on a relative's row: within one category, a préjudice
/// d'affection varies with it. A filled `referentiel_cite` means a table copied
/// out, so not an independent testimony.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Comparabilite {
    pub regimes: Vec<Regime>,
    pub regime_libelle: Option<String>,
    pub fait: Option<String>,
    pub referentiel_cite: Option<String>,
    pub dfp_victime_directe: Option<f64>,
    pub bareme_capitalisation: Option<Bareme>,
    /// Whether the text names the nature of the aid, or the reading deduces it from what the aid does.
    pub nature_aide_ecrite: Option<bool>,
}

impl Comparabilite {
    fn from_record(q: &Quantum) -> Self {
        Self {
            regimes: q.regimes.clone(),
            regime_libelle: q.regime_libelle.clone(),
            fait: q.fait_generateur.clone(),
            referentiel_cite: q.referentiel_cite.clone(),
            dfp_victime_directe: q
                .victimes
                .iter()
                .find(|v| v.role == "directe")
                .and_then(|v| v.taux_dfp_pct),
            bareme_capitalisation: q.bareme_capitalisation.clone(),
            nature_aide_ecrite: None,
        }
    }
}

/// Une ligne de la distribution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Observation {
    pub victime_ord: i16,
    pub poste_ord: i16,
    /// The component and the period where the tariff is written, null on the
    /// poste's own row. A finer row carries no amount: the award lives once, on
    /// the poste, and the finer row observes the price alone.
    pub composante_ord: Option<i16>,
    pub tranche_ord: Option<i16>,
    pub poste: String,
    pub axe: Option<String>,
    /// Rempli quand la ligne ne se compare à rien. Elle reste écrite : sans
    /// elle, la couverture de la mesure serait invisible.
    pub hors_table: Option<String>,
    pub montant: Option<f64>,
    pub valeur_unitaire: Option<f64>,
    pub quantite: Option<f64>,
    pub unite: Option<String>,
    /// Une heure d'aide active et une heure de présence responsable sont deux
    /// tarifs, jamais la même médiane.
    pub nature_aide: Option<String>,
    pub mode_emploi: Option<String>,
    pub specialisee: Option<bool>,
    pub nuit: Option<bool>,
    pub taux_pct: Option<f64>,
    pub cotation: Option<f64>,
    pub categorie: Option<String>,
    /// Le grain qui manque pour affiner la catégorie : la ligne s'agrège plus
    /// grossièrement au lieu de disparaître.
    pub categorie_manque: Option<String>,
    /// La victime directe est décédée. Deux populations disjointes, et le
    /// référentiel ne chiffre que la première.
    pub deces: Option<bool>,
    /// Rempli pour les seules lignes qui ont une abscisse : ce sont les seules
    /// que la distribution compare, donc à reclasser.
    pub comparabilite: Option<Comparabilite>,
    pub taux_dfp_pct: Option<f64>,
    pub age: Option<f64>,
    pub sexe: Option<String>,
    pub role: Option<String>,
    pub lien: Option<String>,
    /// What determines the amount, by the spreadsheet: `confirmee` by two
    /// paths, `sommee` by the victim's total closing on it, `lue` alone,
    /// `derivee` from a single relation, or nothing when no cell holds it.
    pub preuve: Option<String>,
    pub equations: i16,
    /// The nature of the correction that put the amount there, and `cite` what
    /// the text carries at that spot. Empty when the value is as written.
    pub origine: Option<String>,
    pub cite: Option<String>,
    /// Sum of what earlier liquidations already paid on the poste; `montant`
    /// then adds it to the complement the decision awards.
    pub indemnites_acquises: Option<f64>,
}

/// The fields of a poste that a correction may touch without moving its value.
const BESIDE_THE_VALUE: &[&str] = &[
    "montant_demande",
    "montant_offert",
    "alloue_premiere_instance",
    "libelle_decision",
];

/// The path under the poste that `corrections[].champ` writes for this grain.
pub fn under_poste(composante_ord: Option<i16>, tranche_ord: Option<i16>) -> String {
    match (composante_ord, tranche_ord) {
        (Some(c), Some(t)) => format!("composantes[{c}].tranches[{t}]."),
        (Some(c), None) => format!("composantes[{c}]."),
        (None, Some(t)) => format!("tranches[{t}]."),
        (None, None) => String::new(),
    }
}

/// The correction that moved this row's value, if any: one that names the
/// victim, or every victim, the poste's code, and the same depth under it, on a
/// field the table reads.
fn correction_on<'a>(
    corrections: &'a [Correction],
    cle: &str,
    code: &str,
    under: &str,
) -> Option<&'a Correction> {
    corrections.iter().find(|c| {
        let Some(rest) = c.champ.strip_prefix("victimes[") else {
            return false;
        };
        let Some((who, rest)) = rest.split_once("].postes[") else {
            return false;
        };
        let Some((which, leaf)) = rest.split_once("].") else {
            return false;
        };
        let Some(champ) = leaf.strip_prefix(under) else {
            return false;
        };
        (who == "*" || who == cle)
            && which == code
            && !champ.contains('.')
            && !BESIDE_THE_VALUE.contains(&champ)
    })
}

/// A read amount is summed when an aggregate the court writes, the total of
/// the postes, the assiette of what is due or the net, closes on their sum:
/// that sum is its second path, shared with its siblings. An aggregate the
/// spreadsheet only derives proves nothing.
fn summed(t: &Spreadsheet, victime_ord: i16) -> bool {
    ["total", "assiette", "net"].iter().any(|k| {
        let key = format!("v{victime_ord}.{k}");
        t.proof(&key) == Some(Proof::Confirmed) && t.source(&key) == Some("lu")
    })
}

fn full_base(v: &Victime, p: &Poste) -> Option<String> {
    if v.partage_responsabilite_pct.is_none() {
        return Some("base des montants non établie".to_string());
    }
    if p.reduced_only() {
        return Some(match v.owed_pct().filter(|pct| *pct < 100.0) {
            Some(pct) => format!("poste chiffré réduit à {pct} %"),
            None => "poste chiffré réduit".to_string(),
        });
    }
    None
}

/// La sous-cotation du préjudice d'affection, et ce qui manque pour l'affiner.
/// À parenté égale, un enfant mineur, un enfant majeur au foyer et un enfant
/// majeur ailleurs relèvent de trois fourchettes disjointes.
pub fn paf_category(v: &Victime) -> (String, Option<String>) {
    let mineur = v.mineur.or_else(|| v.age_fait.map(|a| a < 18.0));
    let (coh, freq) = (v.cohabitation, v.frequence_relations.as_deref());

    match v.lien {
        Some(Lien::Conjoint) | Some(Lien::Concubin) | Some(Lien::Partenaire) => {
            ("conjoint".to_string(), None)
        }
        Some(Lien::Parent) => ("parent".to_string(), None),
        Some(Lien::Enfant) => match (mineur, coh) {
            (Some(true), _) => ("enfant mineur".to_string(), None),
            (None, _) => ("enfant".to_string(), Some("minorité".to_string())),
            (Some(false), Some(true)) => ("enfant majeur au foyer".to_string(), None),
            (Some(false), Some(false)) => ("enfant majeur hors foyer".to_string(), None),
            (Some(false), None) => (
                "enfant majeur".to_string(),
                Some("cohabitation".to_string()),
            ),
        },
        Some(Lien::Fratrie) => match coh {
            Some(true) => ("fratrie au foyer".to_string(), None),
            Some(false) => ("fratrie hors foyer".to_string(), None),
            None => ("fratrie".to_string(), Some("cohabitation".to_string())),
        },
        Some(l @ (Lien::GrandParent | Lien::PetitEnfant)) => {
            let stem = if l == Lien::GrandParent {
                "grand-parent"
            } else {
                "petit-enfant"
            };
            let low = freq.map(|f| f.trim().to_lowercase());
            let said = match low.as_deref() {
                Some(x) if x.contains("peu") && x.contains("fr") => Some("peu fréquentes"),
                Some(x) if x.contains("fr") => Some("fréquentes"),
                _ => None,
            };
            match said {
                Some(f) => (format!("{stem}, relations {f}"), None),
                None => (
                    stem.to_string(),
                    Some("fréquence des relations".to_string()),
                ),
            }
        }
        Some(Lien::BeauParent) => ("beau-parent".to_string(), None),
        Some(Lien::BeauFils) => ("beau-fils".to_string(), None),
        Some(Lien::OncleTante) => ("oncle ou tante".to_string(), None),
        Some(Lien::NeveuNiece) => ("neveu ou nièce".to_string(), None),
        Some(Lien::Cousin) => ("cousin".to_string(), None),
        Some(Lien::Autre) | None => ("autre proche".to_string(), None),
    }
}

/// The only relatives' postes whose abscissa is the link: funeral costs are
/// actual costs, and a loss of income placed by the link would skew the median.
const ON_LINK: &[&str] = &["PAF", "PAC"];

/// The CFP franc has been pegged to the euro since 1999: 1 000 F CFP = 8,38 euros.
const XPF_PER_EUR: f64 = 119.331_74;

/// Euros per unit of the record's currency; `None` for a currency without a
/// fixed parity, which no table can hold.
pub(super) fn euro_rate(devise: Option<&str>) -> Option<f64> {
    match devise {
        None | Some("EUR") => Some(1.0),
        Some("XPF") => Some(1.0 / XPF_PER_EUR),
        Some(_) => None,
    }
}

/// The axis a line takes, `None` when nothing places it on a table. One rule,
/// read by the projection that publishes the line and by the bench that asks a
/// reading whether it kept the abscissa the decision writes.
pub fn axis_of(
    v: &Victime,
    code: &str,
    unite: Option<&str>,
    valeur_unitaire: Option<f64>,
    cotation: Option<f64>,
) -> Option<Axis> {
    if code == "DFP" && unite == Some("point") {
        Some(Axis::Grille)
    } else if valeur_unitaire.is_some() {
        Some(Axis::Unite)
    } else if cotation.is_some() {
        Some(Axis::Cotation)
    } else if ON_LINK.contains(&code) && v.role == "indirecte" && v.lien.is_some() {
        Some(Axis::Categorie)
    } else {
        None
    }
}

/// The unit and the quantity a poste's own line carries: the number of points of
/// a DFP is the victim's rate, which lives once in the record.
pub fn priced_as(v: &Victime, p: &Poste, code: &str) -> (Option<Unite>, Option<f64>) {
    let (mut unite, mut quantite) = (p.unite, p.quantite);
    if code == "DFP" && quantite.is_none() {
        if let Some(taux) = v.taux_dfp_pct {
            quantite = Some(taux);
            unite = unite.or(Some(Unite::Point));
        }
    }
    (unite, quantite)
}

fn set_axis(o: &mut Observation, v: &Victime, code: &str, deces: Option<bool>) {
    let axe = axis_of(v, code, o.unite.as_deref(), o.valeur_unitaire, o.cotation);
    o.axe = axe.map(|a| a.name().to_string());
    if axe == Some(Axis::Categorie) {
        let (cat, missing) = paf_category(v);
        o.categorie = Some(cat);
        o.categorie_manque = missing.or_else(|| {
            deces
                .is_none()
                .then(|| "issue de la victime directe".to_string())
        });
        o.deces = deces;
    }
}

fn project(
    q: &Quantum,
    v: &Victime,
    p: &Poste,
    t: &Spreadsheet,
    victime_ord: i16,
    poste_ord: i16,
    comparabilite: &Comparabilite,
) -> Observation {
    let code = p
        .poste
        .clone()
        .unwrap_or_else(|| "HORS_NOMENCLATURE".to_string());
    let (unite, quantite) = priced_as(v, p, &code);

    let cell = format!("v{victime_ord}.p{poste_ord}.plein");
    let acquis =
        (!p.indemnites_acquises.is_empty()).then(|| p.indemnites_acquises.iter().sum::<f64>());
    let viager = p.prorata_survie.as_ref().map(|pr| pr.montant_viager);
    let correction = correction_on(&q.corrections, &v.cle, &code, "");
    let mut o = Observation {
        victime_ord,
        poste_ord,
        poste: code.clone(),
        preuve: t.proof(&cell).map(|pr| match pr {
            Proof::Read if summed(t, victime_ord) => "sommee".to_string(),
            other => other.name().to_string(),
        }),
        equations: t.constraints(&cell) as i16,
        origine: correction.map(|c| c.nature.name().to_string()),
        cite: correction.and_then(|c| c.cite.as_ref()).map(|c| match c {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }),
        indemnites_acquises: acquis,
        montant: viager.or_else(|| p.montant_alloue.map(|m| m + acquis.unwrap_or(0.0))),
        valeur_unitaire: p.valeur_unitaire,
        quantite,
        unite: unite.map(|u| u.name().to_string()),
        nature_aide: p.nature_aide.map(|n| n.name().to_string()),
        mode_emploi: p.mode_emploi.map(|m| m.name().to_string()),
        specialisee: p.specialisee,
        nuit: p.nuit,
        taux_pct: p.taux_pct,
        cotation: p.cotation,
        taux_dfp_pct: v.taux_dfp_pct,
        age: v.age_consolidation,
        sexe: v.sexe.clone(),
        role: Some(v.role.clone()),
        lien: v.lien.map(|l| l.name().to_string()),
        ..Default::default()
    };
    o.comparabilite = Some(comparabilite.clone());
    o
}

/// The axis, the currency and the reason for being off the table, once the row
/// is filled. Kept apart from filling it: a finer row clones the poste's row
/// before conversion, and converting twice would double the exchange.
fn settle(o: &mut Observation, q: &Quantum, v: &Victime, p: &Poste) {
    let code = o.poste.clone();
    let comparabilite = o.comparabilite.take();
    set_axis(o, v, &code, q.deces);
    if o.axe.is_some() {
        o.comparabilite = comparabilite;
    }

    let rate = euro_rate(q.devise.as_deref());
    if let Some(r) = rate.filter(|r| *r != 1.0) {
        o.montant = o.montant.map(|m| m * r);
        o.valeur_unitaire = o.valeur_unitaire.map(|x| x * r);
        o.indemnites_acquises = o.indemnites_acquises.map(|a| a * r);
    }
    o.hors_table = rate
        .is_none()
        .then(|| {
            format!(
                "devise {} : aucune parité fixe",
                q.devise.as_deref().unwrap_or("")
            )
        })
        .or_else(|| {
            q.regimes
                .contains(&Regime::Gav)
                .then(|| "garantie accidents de la vie : le contrat fixe le montant".to_string())
        })
        .or_else(|| {
            foreign_law(q).then(|| "loi étrangère : la cour chiffre selon un autre droit".to_string())
        })
        .or_else(|| (code == "HORS_NOMENCLATURE").then(|| "poste hors nomenclature".to_string()))
        .or_else(|| {
            let unpriced = p.prorata_survie.is_none() && p.valeur_unitaire.is_none();
            (code == "DFP" && q.deces == Some(true) && unpriced)
                .then(|| "décès : le montant du DFP ne court que jusqu'au décès".to_string())
        })
        .or_else(|| {
            (o.unite.as_deref() == Some("euro_de_rente")).then(|| {
                "montant annuel capitalisé : propre à la victime, il ne se compare pas".to_string()
            })
        })
        .or_else(|| {
            (matches!(code.as_str(), "PGPA" | "PGPF")
                && matches!(o.unite.as_deref(), Some("jour" | "mois" | "an")))
            .then(|| {
                "perte de revenus au jour, au mois ou à l'an : salaire, indemnités journalières et net s'y confondent"
                    .to_string()
            })
        })
        .or_else(|| {
            (code == "PAC" && q.deces != Some(true)).then(|| {
                "préjudice d'accompagnement d'une victime qui survit : la nomenclature le réserve au décès"
                    .to_string()
            })
        })
        .or_else(|| full_base(v, p))
        .or_else(|| match (o.montant, o.valeur_unitaire, o.axe.is_some()) {
            (Some(0.0), None, _) => Some("poste rejeté : la cour n'alloue rien".to_string()),
            (None, None, _) => Some("aucun montant relevé".to_string()),
            (_, _, false) => Some("ni tarif ni cotation : pas d'abscisse".to_string()),
            _ => None,
        })
        .or_else(|| {
            (o.unite.as_deref() == Some("heure") && o.nature_aide.is_none())
                .then(|| "tarif horaire sans la nature de l'aide".to_string())
        })
        .or_else(|| {
            merged_affection(&code, p).then(|| {
                "préjudice d'affection chiffré avec un autre chef : la somme ne se répartit pas"
                    .to_string()
            })
        });
}

/// A foundation that opens on a foreign law: the court values under it. A decision
/// citing one further on values under French law, the foreign one ruling the fault or a
/// claim.
static FOREIGN_LAW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^[^;]{0,30}\b(loi|droit)s? (interne )?(belge|espagnol|allemand|italien|luxembourgeois|",
        r"suisse|portugais|neerlandais|britannique|anglais|ecossais|irlandais|autrichien|",
        r"monegasque|andorran|polonais|roumain|bulgare|grec|tcheque|slovaque|hongrois|croate|",
        r"slovene|serbe|danois|suedois|norvegien|finlandais|turc|marocain|algerien|tunisien|",
        r"senegalais|ivoirien|camerounais|malgache|mauricien|americain|canadien|quebecois|",
        r"bresilien|mexicain|dominicain|russe|ukrainien|israelien|libanais|chinois|japonais)",
    ))
    .expect("foreign law pattern")
});

fn foreign_law(q: &Quantum) -> bool {
    q.regime_libelle
        .as_deref()
        .is_some_and(|r| FOREIGN_LAW.is_match(&lj_fold::fold(r)))
}

/// One sum the court gives a relative for the affection and another head.
fn merged_affection(code: &str, p: &Poste) -> bool {
    let label = p
        .libelle_decision
        .as_deref()
        .map(lj_fold::fold)
        .unwrap_or_default();
    code == "PAF" && (label.contains("accompagnement") || label.contains("conditions d'existence"))
}

/// A tariff written under the poste: its place in the record, the spreadsheet
/// cell that proves it, and what the court says of it.
pub struct Priced {
    pub composante_ord: Option<i16>,
    pub tranche_ord: Option<i16>,
    pub cell: String,
    pub valeur_unitaire: Option<f64>,
    pub quantite: Option<f64>,
    pub unite: Option<Unite>,
    pub taux_pct: Option<f64>,
    pub nature_aide: Option<NatureAide>,
    pub mode_emploi: Option<ModeEmploi>,
    pub specialisee: Option<bool>,
    pub nuit: Option<bool>,
}

impl Priced {
    fn from_composante(c: &Composante, ci: i16, at: &str) -> Self {
        Self {
            composante_ord: Some(ci),
            tranche_ord: None,
            cell: at.to_string(),
            valeur_unitaire: c.valeur_unitaire,
            quantite: c.quantite,
            unite: c.unite,
            taux_pct: c.taux_pct,
            nature_aide: c.nature_aide,
            mode_emploi: c.mode_emploi,
            specialisee: c.specialisee,
            nuit: c.nuit,
        }
    }

    fn from_tranche(b: &Tranche, ci: Option<i16>, ti: i16, at: &str) -> Self {
        Self {
            composante_ord: ci,
            tranche_ord: Some(ti),
            cell: at.to_string(),
            valeur_unitaire: b.valeur_unitaire,
            quantite: b.quantite,
            unite: b.unite,
            taux_pct: None,
            nature_aide: b.nature_aide,
            mode_emploi: b.mode_emploi,
            specialisee: b.specialisee,
            nuit: b.nuit,
        }
    }
}

/// The tariffs the poste writes below itself, in the order of the record.
pub fn priced_under(p: &Poste, vi: i16, pi: i16) -> Vec<Priced> {
    let mut out = Vec::new();
    for (ci, c) in p.composantes.iter().enumerate() {
        let at = format!("v{vi}.p{pi}.c{ci}");
        out.push(Priced::from_composante(c, ci as i16, &at));
        for (ti, b) in c.tranches.iter().enumerate() {
            out.push(Priced::from_tranche(
                b,
                Some(ci as i16),
                ti as i16,
                &format!("{at}.t{ti}"),
            ));
        }
    }
    for (ti, b) in p.tranches.iter().enumerate() {
        out.push(Priced::from_tranche(
            b,
            None,
            ti as i16,
            &format!("v{vi}.p{pi}.t{ti}"),
        ));
    }
    out
}

/// The price to the cent and its unit: the same tariff written twice under one
/// poste is one observation. The finer writings often only repeat the poste's
/// tariff, period by period, and counting them would put copies of one daily
/// price in the median.
fn price_key(vu: Option<f64>, unite: Option<Unite>) -> Option<(i64, Option<&'static str>)> {
    vu.map(|x| ((x * 100.0).round() as i64, unite.map(|u| u.name())))
}

/// The tariffs a poste writes below itself, one per price, the poste's own
/// being the price they are counted against: the projection makes a row of each.
pub fn tariffs_under(p: &Poste, vi: i16, pi: i16) -> Vec<Priced> {
    let mut seen = vec![price_key(p.valeur_unitaire, p.unite)];
    let mut out = Vec::new();
    for f in priced_under(p, vi, pi) {
        let key = price_key(f.valeur_unitaire, f.unite);
        if key.is_none() || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(f);
    }
    out
}

/// One row per tariff finer than the poste, cloned from its own: same victim,
/// same code, never its amount, which lives once.
fn finer(
    q: &Quantum,
    v: &Victime,
    p: &Poste,
    t: &Spreadsheet,
    base: &Observation,
) -> Vec<Observation> {
    let mut out = Vec::new();
    for f in tariffs_under(p, base.victime_ord, base.poste_ord) {
        let under = under_poste(f.composante_ord, f.tranche_ord);
        let correction = correction_on(&q.corrections, &v.cle, &base.poste, &under);
        out.push(Observation {
            composante_ord: f.composante_ord,
            tranche_ord: f.tranche_ord,
            montant: None,
            indemnites_acquises: None,
            cotation: None,
            valeur_unitaire: f.valeur_unitaire,
            quantite: f.quantite,
            unite: f.unite.map(|u| u.name().to_string()),
            taux_pct: f.taux_pct,
            nature_aide: f.nature_aide.map(|n| n.name().to_string()),
            mode_emploi: f.mode_emploi.map(|m| m.name().to_string()),
            specialisee: f.specialisee,
            nuit: f.nuit,
            preuve: t.proof(&f.cell).map(|pr| match pr {
                Proof::Read if summed(t, base.victime_ord) => "sommee".to_string(),
                other => other.name().to_string(),
            }),
            equations: t.constraints(&f.cell) as i16,
            origine: correction.map(|c| c.nature.name().to_string()),
            cite: correction.and_then(|c| c.cite.as_ref()).map(|c| match c {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            }),
            ..base.clone()
        });
    }
    out
}

/// Toutes les observations d'un record, dans l'ordre du record : l'ordinal est
/// la clé, donc il ne se dérive pas d'un tri.
pub fn observations(q: &Quantum, t: &Spreadsheet) -> Vec<Observation> {
    let comparabilite = Comparabilite::from_record(q);
    let mut out = Vec::new();
    for (iv, v) in q.victimes.iter().enumerate() {
        for (ip, p) in v.postes.iter().enumerate() {
            let poste = project(q, v, p, t, iv as i16, ip as i16, &comparabilite);
            let mut lignes = finer(q, v, p, t, &poste);
            lignes.insert(0, poste);
            for mut o in lignes {
                settle(&mut o, q, v, p);
                out.push(o);
            }
        }
    }
    out
}
