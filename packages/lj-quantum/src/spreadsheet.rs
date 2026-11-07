//! Des cases nommées, chacune posée une fois : une seconde pose divergente est
//! une contradiction localisée sur la case, et non un écart global à arbitrer.
//! Une case que rien ne détermine est un trou, pas une faute.

use rustc_hash::{FxHashMap, FxHashSet};

use super::{product, Breach, Conflict, Poste, Quantum, Rounding, Verdict, Victime, EPS};

/// Le nombre de passes de propagation. Une relation en débloque une autre, et le
/// point fixe est atteint bien avant : la borne n'est là que pour interdire une
/// boucle si une relation future se met à osciller.
const PASSES: usize = 40;

#[derive(Debug, Default)]
pub struct Spreadsheet {
    val: FxHashMap<String, f64>,
    src: FxHashMap<String, String>,
    pub conflicts: Vec<Conflict>,
    pub roundings: Vec<Rounding>,
    pub breaches: Vec<Breach>,
    seen: FxHashSet<(String, i64)>,
    agreements: FxHashMap<String, FxHashSet<String>>,
    relations: FxHashMap<String, FxHashSet<String>>,
    links: FxHashMap<String, FxHashSet<String>>,
}

impl Spreadsheet {
    pub fn new() -> Self {
        Self::default()
    }

    /// `tol` est le budget d'arrondi de la relation qui pose la valeur, non une
    /// tolérance de confort : la cour arrondit chaque part au centime, donc la
    /// somme des `arrondi(poste x taux)` dérive d'un demi-centime par poste face
    /// à `(somme des postes) x taux`.
    pub fn set(&mut self, key: &str, v: Option<f64>, src: &str, tol: f64) -> bool {
        let Some(v) = v else { return false };
        if let Some(&previous) = self.val.get(key) {
            let gap = v - previous;
            let previous_source = self.src[key].clone();
            if gap.abs() >= tol {
                // Le point fixe repasse : une contradiction ne se compte qu'une fois.
                let cents = (v * 100.0).round() as i64;
                if self.seen.insert((key.to_string(), cents)) {
                    self.conflicts.push(Conflict {
                        cell: key.to_string(),
                        first: previous,
                        first_source: previous_source.clone(),
                        second: v,
                        second_source: src.to_string(),
                    });
                } else if let Some(i) = self
                    .conflicts
                    .iter()
                    .position(|c| c.cell == key && (c.second * 100.0).round() as i64 == cents)
                {
                    // Plusieurs relations réclament la même valeur : celle qui
                    // met le moins de cases en jeu est celle qui la désigne, et
                    // l'ordre de propagation n'a pas à décider laquelle parle.
                    let holder = self.conflicts[i].second_source.clone();
                    if self.arity_of(key, src) < self.arity_of(key, &holder) {
                        self.conflicts[i].second_source = src.to_string();
                    }
                }
            } else if gap.abs() >= EPS {
                self.roundings.push(Rounding {
                    cell: key.to_string(),
                    gap,
                    tolerance: tol,
                    source: src.to_string(),
                });
            }
            // Deux chemins concordants sur une case : le seul compteur qui
            // mesure la force d'un record.
            self.agreements
                .entry(key.to_string())
                .or_insert_with(|| FxHashSet::from_iter([previous_source]))
                .insert(src.to_string());
            return false;
        }
        self.val.insert(key.to_string(), v);
        self.src.insert(key.to_string(), src.to_string());
        true
    }

    fn set_exact(&mut self, key: &str, v: Option<f64>, src: &str) -> bool {
        self.set(key, v, src, EPS)
    }

    pub fn get(&self, key: &str) -> Option<f64> {
        self.val.get(key).copied()
    }

    /// Enregistre qu'une équation lie ces cases. Le nombre d'équations où une
    /// case figure dit à quel point elle est contrainte : une case seule dans
    /// son coin n'est vérifiée par rien.
    pub fn relation(&mut self, name: &str, keys: &[String]) {
        let e = self.relations.entry(name.to_string()).or_default();
        for c in keys {
            e.insert(c.clone());
        }
        for c in keys {
            self.links
                .entry(c.clone())
                .or_default()
                .insert(name.to_string());
        }
    }

    pub fn constraints(&self, key: &str) -> usize {
        self.links.get(key).map_or(0, FxHashSet::len)
    }

    /// Le nombre de termes de la relation sommante qui touche cette case, s'il en
    /// existe une. C'est le nombre de cases entre lesquelles un écart se répartit
    /// sans qu'on sache laquelle est fautive.
    pub fn summing_arity(&self, key: &str) -> Option<usize> {
        self.links
            .get(key)?
            .iter()
            .find(|n| n.contains("somme_") || n.contains("decoupage") || *n == "total_enonce")
            .and_then(|n| self.relations.get(n))
            .map(FxHashSet::len)
    }

    /// Combien de cases la relation `src` met en jeu autour de celle-ci : deux
    /// pour un report ou un produit, trois pour un trio, ses termes pour une
    /// somme. C'est ce qui dit si un écart désigne une case ou se répartit.
    pub fn arity_of(&self, key: &str, src: &str) -> usize {
        if src.starts_with("somme des ") {
            self.summing_arity(key).unwrap_or(99)
        } else if src.contains('+') {
            3
        } else {
            2
        }
    }

    /// Ce que la lecture a écrit, par opposition à ce qu'une relation a posé.
    pub fn source(&self, key: &str) -> Option<&str> {
        self.src.get(key).map(String::as_str)
    }

    /// Every determined cell with its value, in no particular order.
    pub fn cells(&self) -> impl Iterator<Item = (&str, f64)> {
        self.val.iter().map(|(k, v)| (k.as_str(), *v))
    }

    pub fn proof(&self, key: &str) -> Option<Proof> {
        let src = self.src.get(key)?;
        Some(if self.agreements.get(key).is_some_and(confirms) {
            Proof::Confirmed
        } else if src == "lu" {
            Proof::Read
        } else {
            Proof::Derived
        })
    }

    /// La somme des parts vaut la case, dans les deux sens : le total quand tout
    /// est connu, le terme manquant quand il n'en manque qu'un.
    pub fn sum(&mut self, key: &str, parts: &[String], what: &str) -> bool {
        let known: Vec<&String> = parts.iter().filter(|p| self.val.contains_key(*p)).collect();
        if known.len() == parts.len() {
            let total: f64 = parts.iter().map(|p| self.val[p]).sum();
            return self.set_exact(key, Some(total), &format!("somme des {what}"));
        }
        if known.len() + 1 == parts.len() {
            if let Some(&total) = self.val.get(key) {
                let seen: f64 = known.iter().map(|p| self.val[*p]).sum();
                let missing = parts
                    .iter()
                    .find(|p| !self.val.contains_key(*p))
                    .expect("un terme manque puisque le compte des connus le dit")
                    .clone();
                return self.set_exact(&missing, Some(total - seen), &format!("reste de {key}"));
            }
        }
        false
    }

    /// `a + b = c`, dans les trois sens. La case visée est posée même déjà
    /// remplie : sinon la relation se tait dès que ses trois termes sont connus,
    /// c'est-à-dire précisément quand elle vérifie quelque chose.
    pub fn trio(&mut self, a: &str, b: &str, c: &str, what: &str) -> bool {
        let mut moved = false;
        for (x, y, z, sum) in [(a, b, c, true), (c, b, a, false), (c, a, b, false)] {
            if let (Some(vx), Some(vy)) = (self.get(x), self.get(y)) {
                let v = if sum { vx + vy } else { vx - vy };
                moved |= self.set(z, Some(v), what, 0.01);
            }
        }
        moved
    }

    pub fn verdict(&self) -> Verdict {
        Verdict {
            cells: self.val.len(),
            confirmed: self.agreements.values().filter(|a| confirms(a)).count(),
            conflicts: self.conflicts.clone(),
            roundings: self.roundings.clone(),
            breaches: self.breaches.clone(),
        }
    }

    /// Combien de cases chaque famille de relation confirme. Une case confirmée
    /// compte pour chacune de ses sources : le chiffre dit ce qu'on perdrait en
    /// retirant la relation, pas une partition.
    pub fn contributions(&self) -> Vec<(String, usize)> {
        let mut by_relation: FxHashMap<&str, usize> = FxHashMap::default();
        for sources in self.agreements.values().filter(|a| a.len() > 1) {
            for s in sources.iter() {
                *by_relation.entry(family(s)).or_default() += 1;
            }
        }
        let mut out: Vec<(String, usize)> = by_relation
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }
}

/// What determines a cell: two informative paths agreeing, the reading alone,
/// or a single relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proof {
    Confirmed,
    Read,
    Derived,
}

impl Proof {
    pub fn name(self) -> &'static str {
        match self {
            Proof::Confirmed => "confirmee",
            Proof::Read => "lue",
            Proof::Derived => "derivee",
        }
    }
}

/// A cell is confirmed when two paths **carrying information** reach it. A
/// sharing rate of 1 copies the value, and counting it would inflate the
/// confirmed total.
fn confirms(sources: &FxHashSet<String>) -> bool {
    sources.iter().filter(|s| !is_copy(s)).count() > 1
}

fn is_copy(src: &str) -> bool {
    src.ends_with("x 1.000000") || src.ends_with("/ 1.000000")
}

/// La famille d'une source, pour compter ce que chaque relation rapporte. Les
/// libellés portent des montants et des taux, donc le regroupement est un
/// préfixe.
fn family(src: &str) -> &'static str {
    match src {
        "lu" => "lu dans l'arrêt",
        "forcé" => "forcé par un arbitrage",
        "poste rejeté" | "créance nulle" | "net nul" => "zéro propagé",
        "net + creance" => "net + créance = assiette",
        "part victime + part tiers" => "part victime + part tiers",
        s if s.starts_with("somme des ") => match &s["somme des ".len()..] {
            "postes" => "somme des postes = total victime",
            "composantes" => "somme des composantes = poste",
            "dus des composantes" => "somme des dus des composantes = dû du poste",
            "tranches" => "somme des tranches = tarif unitaire",
            "parts victime" => "somme des parts victime = net",
            "parts tiers" => "somme des parts tiers = créance",
            _ => "somme, autre",
        },
        s if s.starts_with("reste de ") => "terme manquant d'une somme",
        s if is_copy(s) => "recopie (partage à 100 %)",
        s if s.starts_with("pct ") => "pourcentage annoncé x total",
        s if s.starts_with("total x ") || s.contains(" / ") => "partage de responsabilité",
        s if s.starts_with("poste x ") => "partage appliqué au poste",
        s if s.starts_with("composante x ") => "partage appliqué à la composante",
        s if s.contains(" x ") => "produit tarif x quantité",
        _ => "autre",
    }
}

fn head_cell(vi: usize, pi: usize, path: &str) -> String {
    format!("v{vi}.p{pi}.{path}")
}

/// The rate owed, and whether it comes from the owed total over the full one, in
/// which case the written percentage is still to check. That ratio wins where it
/// holds: a percentage cannot write a third, and 66,67 % misses 14,82 on an
/// assiette of 296 374,20. It stops holding once a poste is spared.
fn rate_of(v: &Victime, t: &Spreadsheet, vi: usize) -> (f64, bool) {
    // A ratio derived under a written 100 % would spread one aggregate's gap
    // over every poste and hide the only wrong cell.
    let pct = v.owed_pct();
    if pct == Some(100.0) {
        return (1.0, false);
    }
    let written = pct.map_or(1.0, |p| p / 100.0);
    if v.postes.iter().any(Poste::spared) {
        return (written, false);
    }
    let (tot, ass) = (
        t.get(&format!("v{vi}.total")),
        t.get(&format!("v{vi}.assiette")),
    );
    match (tot, ass) {
        (Some(tot), Some(ass)) if tot != 0.0 => (ass / tot, true),
        _ => (written, false),
    }
}

/// `forcages` remplace la valeur du record sur les cases nommées, ce qui sert à
/// demander « et si c'était l'autre chiffre ? ».
fn load(r: &Quantum, t: &mut Spreadsheet, forced: &FxHashMap<String, f64>) {
    for (vi, v) in r.victimes.iter().enumerate() {
        for (val, key) in [
            (v.total_alloue, "total"),
            (v.assiette_reduite, "assiette"),
            (v.net_victime, "net"),
            (v.creance_tiers_payeurs, "creance"),
        ] {
            let name = format!("v{vi}.{key}");
            match forced.get(&name) {
                Some(&f) => t.set_exact(&name, Some(f), "forcé"),
                None => t.set_exact(&name, val, "lu"),
            };
        }
        for (pi, p) in v.postes.iter().enumerate() {
            for (val, key) in [
                (p.montant_alloue, "plein"),
                (p.part_victime, "pv"),
                (p.part_tiers_payeurs, "pt"),
            ] {
                let name = head_cell(vi, pi, key);
                match forced.get(&name) {
                    Some(&f) => t.set_exact(&name, Some(f), "forcé"),
                    None => t.set_exact(&name, val, "lu"),
                };
            }
            for (ci, c) in p.composantes.iter().enumerate() {
                t.set_exact(&head_cell(vi, pi, &format!("c{ci}")), c.montant, "lu");
            }
        }
    }
    match forced.get("total_enonce") {
        Some(&f) => t.set_exact("total_enonce", Some(f), "forcé"),
        None => t.set_exact("total_enonce", r.total_enonce, "lu"),
    };
}

/// Les bandes horaires d'un tarif composé, qui le somment. Une aide humaine à
/// 404 euros par jour vaut « 10 h x 25 + 14 h x 11 » : chaque bande se multiplie,
/// et leur somme repose le tarif déjà lu.
fn bands(t: &mut Spreadsheet, tranches: &[super::Tranche], vu: Option<f64>, at: &str) -> bool {
    if tranches.is_empty() {
        return false;
    }
    let head = format!("{at}.vu");
    let mut moved = t.set_exact(&head, vu, "lu");
    let mut keys = Vec::with_capacity(tranches.len());
    for (bi, b) in tranches.iter().enumerate() {
        let key = format!("{at}.t{bi}");
        if let Some((v, src, tol)) = product(b.valeur_unitaire, b.quantite, None) {
            moved |= t.set(&key, Some(v), &src, tol);
        }
        keys.push(key);
    }
    let mut with_head = keys.clone();
    with_head.push(head.clone());
    t.relation(&format!("{at}.tranches"), &with_head);
    moved | t.sum(&head, &keys, "tranches")
}

/// Les composantes d'un poste, qui le somment au centime. Chacune est un poste
/// en miniature : quand la cour écrit son tarif et sa quantité sans son
/// sous-total, le produit le pose, et c'est une équation de plus.
fn breakdown(t: &mut Spreadsheet, p: &Poste, vi: usize, pi: usize, pl: &str, taux: f64) -> bool {
    if p.composantes.is_empty() {
        return false;
    }
    let mut moved = false;
    let mut keys = Vec::with_capacity(p.composantes.len());
    for (ci, c) in p.composantes.iter().enumerate() {
        let key = head_cell(vi, pi, &format!("c{ci}"));
        moved |= bands(t, &c.tranches, c.valeur_unitaire, &key);
        // Une composante servie en rente n'est pas du capital : le découpage est
        // incomplet et ne se somme pas.
        if c.montant.is_none() && c.rente.is_some() {
            return moved;
        }
        let vu = c.valeur_unitaire.or_else(|| t.get(&format!("{key}.vu")));
        if let Some((v, src, tol)) = product(vu, c.quantite, c.taux_pct) {
            t.relation(&format!("{key}.produit"), std::slice::from_ref(&key));
            moved |= t.set(&key, Some(v), &src, tol);
        }
        keys.push(key);
    }
    let mut with_head = keys.clone();
    with_head.push(pl.to_string());
    t.relation(&format!("v{vi}.p{pi}.decoupage"), &with_head);
    moved |= t.sum(pl, &keys, "composantes");
    if p.composantes.iter().any(|c| c.montant_du.is_some()) {
        moved |= owed_breakdown(t, p, &keys, &head_cell(vi, pi, "du"), taux);
    }
    moved
}

/// What each component owes once the court writes it on one of them: a
/// component the reduction spares owes its full amount, the others that amount
/// at the victim's rate, and together they owe the poste's due.
fn owed_breakdown(t: &mut Spreadsheet, p: &Poste, keys: &[String], du: &str, taux: f64) -> bool {
    let mut moved = false;
    let mut dues = Vec::with_capacity(keys.len());
    for (c, key) in p.composantes.iter().zip(keys) {
        let owed = format!("{key}.du");
        moved |= t.set_exact(&owed, c.montant_du, "lu");
        if !(p.left_whole() || c.left_whole()) {
            if let Some(full) = t.get(key) {
                let src = format!("composante x {taux:.6}");
                moved |= t.set(&owed, Some(full * taux), &src, 0.01);
            }
            if let (Some(d), true) = (t.get(&owed), taux != 0.0) {
                let src = format!("du / {taux:.6}");
                moved |= t.set(key, Some(d / taux), &src, 0.01 / taux);
            }
        }
        dues.push(owed);
    }
    let mut with_head = dues.clone();
    with_head.push(du.to_string());
    t.relation(&format!("{du}.decoupage"), &with_head);
    moved | t.sum(du, &dues, "dus des composantes")
}

fn pass_once(r: &Quantum, t: &mut Spreadsheet) -> bool {
    let mut moved = false;
    let (mut totals, mut nets) = (Vec::new(), Vec::new());
    for (vi, v) in r.victimes.iter().enumerate() {
        let n = v.postes.len();
        let (taux, pct_to_check) = rate_of(v, t, vi);
        for (pi, p) in v.postes.iter().enumerate() {
            let (pl, pv, pt) = (
                head_cell(vi, pi, "plein"),
                head_cell(vi, pi, "pv"),
                head_cell(vi, pi, "pt"),
            );
            let du = head_cell(vi, pi, "du");
            let qt = super::derivations::points_of(v, p);
            let at = format!("v{vi}.p{pi}");
            moved |= bands(t, &p.tranches, p.valeur_unitaire, &at);
            moved |= t.set_exact(&du, p.montant_du, "lu");
            let vu = p.valeur_unitaire.or_else(|| t.get(&format!("{at}.vu")));
            if let Some((val, src, tol)) = product(vu, qt, p.taux_pct) {
                t.relation(&format!("v{vi}.p{pi}.produit"), std::slice::from_ref(&pl));
                moved |= t.set(&pl, Some(val), &src, tol);
            }
            if let Some(pr) = &p.prorata_survie {
                t.relation(&format!("v{vi}.p{pi}.prorata"), std::slice::from_ref(&pl));
                moved |= t.set(
                    &pl,
                    Some(pr.montant_viager / pr.euro_de_rente * pr.duree_ans),
                    &format!(
                        "{} / {} x {} ans",
                        pr.montant_viager, pr.euro_de_rente, pr.duree_ans
                    ),
                    0.01,
                );
            }
            moved |= breakdown(t, p, vi, pi, &pl, taux);
            t.relation(
                &format!("v{vi}.p{pi}.repartition"),
                &[pl.clone(), pv.clone(), pt.clone()],
            );
            let uniform = !p.spared();
            if let (Some(plein), true) = (t.get(&pl), uniform) {
                moved |= t.set(&du, Some(plein * taux), &format!("poste x {taux:.6}"), 0.01);
            }
            moved |= t.trio(&pv, &pt, &du, "part victime + part tiers");
            // The written due is rounded to the cent: dividing it by the rate
            // spreads that rounding by 1 / rate.
            if let (Some(d), true) = (t.get(&du), uniform && taux != 0.0) {
                moved |= t.set(&pl, Some(d / taux), &format!("du / {taux:.6}"), 0.01 / taux);
            }
        }
        // Les montants sont positifs : un poste rejeté ne laisse rien ni à la
        // victime ni au tiers payeur, et un agrégat nul détermine sa colonne
        // entière.
        for pi in 0..n {
            if t.get(&head_cell(vi, pi, "plein")) == Some(0.0) {
                moved |= t.set_exact(&head_cell(vi, pi, "pv"), Some(0.0), "poste rejeté");
                moved |= t.set_exact(&head_cell(vi, pi, "pt"), Some(0.0), "poste rejeté");
            }
        }
        for (aggregate, path, what) in
            [("creance", "pt", "créance nulle"), ("net", "pv", "net nul")]
        {
            if t.get(&format!("v{vi}.{aggregate}")) == Some(0.0) {
                for pi in 0..n {
                    moved |= t.set_exact(&head_cell(vi, pi, path), Some(0.0), what);
                }
            }
        }
        for (key, path, what) in [
            (format!("v{vi}.total"), "plein", "postes"),
            (format!("v{vi}.assiette"), "du", "indemnités dues"),
            (format!("v{vi}.net"), "pv", "parts victime"),
            (format!("v{vi}.creance"), "pt", "parts tiers"),
        ] {
            let parts: Vec<String> = (0..n).map(|i| head_cell(vi, i, path)).collect();
            let mut with_cells = parts.clone();
            with_cells.push(key.clone());
            t.relation(&format!("v{vi}.somme_{path}"), &with_cells);
            moved |= t.sum(&key, &parts, what);
        }
        let (net, creance, assiette) = (
            format!("v{vi}.net"),
            format!("v{vi}.creance"),
            format!("v{vi}.assiette"),
        );
        t.relation(
            &format!("v{vi}.assiette"),
            &[net.clone(), creance.clone(), assiette.clone()],
        );
        moved |= t.trio(&net, &creance, &assiette, "net + creance");
        let total = format!("v{vi}.total");
        let uniform = !v.postes.iter().any(Poste::spared);
        if uniform {
            t.relation(
                &format!("v{vi}.reduction"),
                &[total.clone(), assiette.clone()],
            );
        }
        if let (Some(tot), true) = (t.get(&total), uniform) {
            moved |= t.set(
                &assiette,
                Some(tot * taux),
                &format!("total x {taux:.6}"),
                EPS + 0.005 * n as f64,
            );
            // Le taux opérant est ce rapport, qui ne vérifie donc rien du
            // pourcentage écrit : le confronter demande une seconde pose. Budget
            // d'un centième de point, ce qu'un « 66,67 % » arrondit déjà.
            if let (true, Some(pct)) = (pct_to_check, v.owed_pct()) {
                moved |= t.set(
                    &assiette,
                    Some(tot * pct / 100.0),
                    &format!("pct {pct} % x total"),
                    EPS + tot.abs() * 1e-4,
                );
            }
        }
        totals.push(total);
        nets.push(net);
    }
    if let Some(base @ ("postes_pleins" | "parts_victime")) = r.total_enonce_base.as_deref() {
        let postes_pleins = base == "postes_pleins";
        let parts = if postes_pleins { totals } else { nets };
        let mut with_cells = parts.clone();
        with_cells.push("total_enonce".to_string());
        t.relation("total_enonce", &with_cells);
        let what = if postes_pleins {
            "totaux des victimes"
        } else {
            "nets des victimes"
        };
        moved |= t.sum("total_enonce", &parts, what);
    }
    moved
}

/// The court rules within what the parties ask. What the victim receives on a
/// poste stays under the higher of her claim and the offer: the bound holds her
/// share, not the full poste, because a third-party payer claims the rest and the
/// claim read is hers. The full poste stays over the offer, which values the
/// poste before any split.
fn bounds(r: &Quantum, t: &mut Spreadsheet) {
    for (vi, v) in r.victimes.iter().enumerate() {
        for (pi, p) in v.postes.iter().enumerate() {
            let cell = |key: &str| t.get(&head_cell(vi, pi, key));
            let full = cell("plein");
            let owed = cell("du").or(full);
            let share = cell("pv").or_else(|| Some(owed? - cell("pt")?)).or(owed);
            let asked = p
                .montant_demande
                .map(|d| p.montant_offert.map_or(d, |o| d.max(o)));
            if let (Some(share), Some(asked)) = (share, asked) {
                if share > asked + EPS {
                    t.breaches.push(Breach {
                        cell: head_cell(vi, pi, "demande"),
                        value: asked,
                        award: share,
                    });
                }
            }
            if let (Some(full), Some(offer)) = (full, p.montant_offert) {
                if full < offer - EPS {
                    t.breaches.push(Breach {
                        cell: head_cell(vi, pi, "offre"),
                        value: offer,
                        award: full,
                    });
                }
            }
        }
    }
}

/// Charge les signaux et propage jusqu'au point fixe. Rend le tableur, dont les
/// cases portent aussi ce que la lecture n'écrivait pas.
pub fn compute(r: &Quantum) -> Spreadsheet {
    compute_with(r, &FxHashMap::default())
}

/// Le tableur tel qu'il serait si ces cases portaient ces valeurs.
pub fn compute_with(r: &Quantum, forced: &FxHashMap<String, f64>) -> Spreadsheet {
    let mut t = Spreadsheet::new();
    load(r, &mut t, forced);
    for _ in 0..PASSES {
        if !pass_once(r, &mut t) {
            break;
        }
    }
    bounds(r, &mut t);
    t
}

/// Ce que la lecture prouve.
pub fn validate(r: &Quantum) -> Verdict {
    compute(r).verdict()
}
