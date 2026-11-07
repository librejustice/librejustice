//! Linker in-pass : résout chaque occurrence de citation vers le catalogue
//! (`ref_text_uid`, `ref_num_key`) pendant la passe d'extraction, sur un
//! [`LinkSnapshot`] hydraté par l'appelant.
//!
//! Jambes de résolution, **dans l'ordre d'autorité** :
//!
//! 1. alias embarqués (`data/link_aliases.tsv`), validés à l'hydratation ;
//! 2. gate de citabilité (`key_signals`) : acte local, norme privée, fragment →
//!    jamais lié ;
//! 3. acte daté par numéro — unique par (nature, numéro) seulement ;
//! 4. titre exact, désambiguïsé « texte vivant » (ex æquo → abstention) ;
//! 5. droit dérivé UE par (directive|règlement, année/séquence) ;
//! 6. accord bilatéral par (gentilé, date) ;
//! 7. CCN par squelette de tokens ;
//! 8. code étranger par (ISO du gentilé, famille sans gentilé) ;
//! 9. acte daté par date — la forme courte sans numéro.
//!
//! `ref_num_key` est posé par existence de l'article au catalogue, jamais
//! inventé.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use jiff::civil::Date;
use regex::Regex;

use crate::data::LINK_ALIASES_TSV;
use crate::extract::common::FOREIGN_NATIONALITY_STEMS;
use crate::extract::key_signals::{key_signals, Citability, KeyNature, KeySignals};
use crate::gazetteer::gazetteer;
use lj_fold::fold;

/// Ligne texte du catalogue, hydratée par l'appelant (une par `legal_text`).
#[derive(Debug, Clone)]
pub struct CatalogText {
    pub text_uid: String,
    /// Titre catalogue brut (porte le numéro d'acte que `title_key` rabote).
    pub title: String,
    /// `normalize_instrument(title)` tel que stocké.
    pub title_key: String,
    /// Nature catalogue (`CODE`, `LOI`, `DIRECTIVE_EURO`, `code_civil`…).
    pub nature: String,
    pub jurisdiction: Option<String>,
    /// Codes territoriaux à préfixe d'article variable.
    pub num_prefix_agnostic: bool,
    /// Nombre d'articles VIGUEUR (désambiguïsation « texte vivant »).
    pub n_vigueur: i64,
    /// Date de signature (`legal_text.date_texte`) — l'identité datée des
    /// familles à titre libre, dont le titre ne la porte pas.
    pub date_texte: Option<Date>,
    /// NOR (`legal_text.nor`) — identifiant interministériel unique.
    pub nor: Option<String>,
}

impl CatalogText {
    /// Depuis la ligne plate de `lj-store::link_catalog_texts` (même ordre de
    /// colonnes). `date_texte` en ISO.
    #[allow(clippy::type_complexity)]
    pub fn from_row(
        row: (
            String,
            String,
            String,
            String,
            Option<String>,
            bool,
            i64,
            Option<String>,
            Option<String>,
        ),
    ) -> Self {
        let (
            text_uid,
            title,
            title_key,
            nature,
            jurisdiction,
            num_prefix_agnostic,
            n_vigueur,
            date_texte,
            nor,
        ) = row;
        Self {
            text_uid,
            title,
            title_key,
            nature,
            jurisdiction,
            num_prefix_agnostic,
            n_vigueur,
            date_texte: date_texte.and_then(|d| d.parse().ok()),
            nor,
        }
    }
}

/// Ce que le for du corps émetteur dit d'une famille citée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HomeText<'a> {
    /// Le pays ne porte pas cette famille — la cascade continue.
    Absent,
    /// Le for interdit la cible : le pays porte la famille en plusieurs textes
    /// sans que l'article tranche, ou ne la porte pas du tout.
    Abstain,
    Found(&'a str),
}

/// Cible d'une occurrence, prête pour `legal_citation`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkTarget {
    pub ref_text_uid: Option<String>,
    pub ref_num_key: Option<String>,
}

/// Juridiction ÉMETTRICE de la décision extraite — le contexte qui résout les
/// instruments nus qu'une cour cite sans les nommer : « de la Convention » dans
/// un arrêt CEDH désigne la CESDH. Dérivé de `jurisdiction_type` + ECLI
/// (`EU:C`/`EU:T`/`EU:F`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Forum {
    Cedh,
    CjueCour,
    CjueTribunal,
    CjueTfp,
}

impl Forum {
    pub fn of(jurisdiction_type: Option<&str>, ecli: Option<&str>) -> Option<Forum> {
        match jurisdiction_type? {
            "CEDH" => Some(Forum::Cedh),
            "CJUE" => match ecli?.split(':').nth(2)? {
                "C" => Some(Forum::CjueCour),
                "T" => Some(Forum::CjueTribunal),
                "F" => Some(Forum::CjueTfp),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Cibles des instruments NUS par forum : (forum, nature génitive pliée) →
/// uid. Validées au catalogue à l'hydratation, comme les alias.
const FORUM_DEFAULTS: &[(Forum, &str, &str)] = &[
    (Forum::Cedh, "convention", "INTL/CEDH"),
    (Forum::CjueCour, "reglement de procedure", "EU/RPROC/CJUE"),
    (
        Forum::CjueTribunal,
        "reglement de procedure",
        "EU/RPROC/TRIBUNAL",
    ),
    (Forum::CjueTfp, "reglement de procedure", "EU/RPROC/TFP"),
];

/// Index mémoire du catalogue pour la passe : se rebâtit au début de chaque
/// run d'ingest ou passe intégrale.
pub struct LinkSnapshot {
    /// Alias embarqués niveau texte : `lower(text_key)` → uid.
    alias_text: HashMap<String, String>,
    /// Alias embarqués niveau article : `(lower(text_key), article_key)` →
    /// (uid, `ref_num_key` forcé éventuel).
    alias_article: HashMap<(String, String), (String, Option<String>)>,
    /// `fold(title_key)` → uid du texte vivant (ex æquo exclu).
    title_to_uid: HashMap<String, String>,
    /// `fold(title_key)` → TOUS les uids frères (clés à ≥ 2 porteurs
    /// seulement) : l'article cité tranche quand le vivant l'ignore.
    title_brothers: HashMap<String, Vec<String>>,
    /// (nature pliée, « NN-NNN ») → uid, uniques seulement.
    nature_num: HashMap<(String, String), String>,
    /// (directive?, « NNNN/NNN ») → uid, uniques seulement.
    eu_num: HashMap<(bool, String), String>,
    /// Droit dérivé UE par date — TOUS les candidats d'une (directive?, date)
    /// avec leur titre plié, que les tokens de la clé citée départagent.
    eu_date: HashMap<(bool, Date), Vec<(String, String)>>,
    /// Cibles des instruments nus par (forum, nature) — [`FORUM_DEFAULTS`]
    /// filtré aux uids présents au catalogue.
    forum_defaults: HashMap<(Forum, &'static str), String>,
    /// (nature pliée, date de l'acte) → uid, uniques seulement.
    nature_date: HashMap<(String, Date), String>,
    /// Circulaires par date — TOUS les candidats avec leur titre plié : la
    /// queue-objet de la citation (« relative aux conditions d'examen… »)
    /// départage.
    circ_date: HashMap<Date, Vec<(String, String)>>,
    /// NOR (majuscules) → uid, uniques seulement.
    nor: HashMap<String, String>,
    /// Traités par date — TOUTES les dates du titre de l'acte de publication
    /// (« faite à La Haye le 25 octobre 1980 ») indexent le texte, et TOUS les
    /// candidats d'une date : `(uid, titre plié)`, que les tokens cités puis les
    /// mots-instruments départagent.
    treaty_date: HashMap<Date, Vec<(String, String)>>,
    /// Accords bilatéraux : (stem gentilé plié, date) → uid, uniques seulement.
    accords: HashMap<(String, Date), String>,
    /// (ISO-2, famille de titre sans gentilé) → uids porteurs.
    foreign_title: HashMap<(String, String), Vec<String>>,
    /// (ISO-2, TÊTE de famille) → uids porteurs — le catalogue nomme un code
    /// par sa matière complète (« code de procédure civile, commerciale,
    /// administrative et financière »), le corps le cite par sa tête. Second
    /// rang : la famille exacte prime.
    foreign_head: HashMap<(String, String), Vec<String>>,
    /// uid → identité d'acte daté lue dans le TITRE catalogue (numéro tel
    /// qu'écrit, date de signature).
    act_identity: HashMap<String, (Option<String>, Option<Date>)>,
    /// uid → ISO-2, pour les seuls textes du référentiel étranger (ni FR, ni
    /// UE, ni INTL) : le for du corps émetteur.
    foreign_owner: HashMap<String, String>,
    /// Cibles CCN présentes (valide le snap gazetteer).
    kalicont: HashSet<String>,
    /// uid → `num_key` des articles au catalogue (existence).
    articles: HashMap<String, HashSet<String>>,
    /// uid préfixe-agnostique → cœur numérique → `num_key` officiel.
    prefix_agnostic: HashMap<String, HashMap<String, String>>,
}

impl LinkSnapshot {
    /// Bâtit le snapshot depuis les lignes catalogue. `articles` = paires
    /// `(text_uid, num_key)` distinctes. Pur et déterministe.
    pub fn build(texts: Vec<CatalogText>, articles: Vec<(String, String)>) -> Self {
        let mut article_sets: HashMap<String, HashSet<String>> = HashMap::new();
        for (uid, num_key) in articles {
            article_sets.entry(uid).or_default().insert(num_key);
        }

        // Texte vivant par title_key : max VIGUEUR, ex æquo → skip. Clés pliées
        // (`fold`) : le vocabulaire cité et le catalogue divergent sur les
        // apostrophes typographiques.
        let mut best: HashMap<String, (usize, i64, bool)> = HashMap::new();
        for (i, t) in texts.iter().enumerate() {
            let key = fold(&t.title_key);
            if !title_names_text(&key) || is_rectificatif(&fold(&t.title)) {
                continue;
            }
            match best.get_mut(&key) {
                None => {
                    best.insert(key, (i, t.n_vigueur, false));
                }
                Some(cur) => {
                    if t.n_vigueur > cur.1 {
                        *cur = (i, t.n_vigueur, false);
                    } else if t.n_vigueur == cur.1 {
                        cur.2 = true;
                    }
                }
            }
        }
        let title_to_uid: HashMap<String, String> = best
            .into_iter()
            .filter_map(|(key, (i, _, tie))| (!tie).then(|| (key, texts[i].text_uid.clone())))
            .collect();
        let mut title_brothers: HashMap<String, Vec<String>> = HashMap::new();
        for t in &texts {
            title_brothers
                .entry(fold(&t.title_key))
                .or_default()
                .push(t.text_uid.clone());
        }
        title_brothers.retain(|_, v| v.len() >= 2);

        // Index structurés, uniques seulement : un doublon retire la clé.
        fn insert_unique<K: std::hash::Hash + Eq + Clone>(
            map: &mut HashMap<K, String>,
            dead: &mut HashSet<K>,
            key: K,
            uid: &str,
        ) {
            if dead.contains(&key) {
                return;
            }
            match map.get(&key) {
                Some(prev) if prev != uid => {
                    map.remove(&key);
                    dead.insert(key);
                }
                Some(_) => {}
                None => {
                    map.insert(key, uid.to_string());
                }
            }
        }

        // Index à porteurs multiples : la clé garde TOUS ses candidats, et
        // l'article cité tranche à la résolution.
        fn push_candidate(
            map: &mut HashMap<(String, String), Vec<String>>,
            key: (String, String),
            uid: &str,
        ) {
            let cands = map.entry(key).or_default();
            if !cands.iter().any(|u| u == uid) {
                cands.push(uid.to_string());
            }
        }

        let mut nature_num = HashMap::new();
        let mut nature_num_dead = HashSet::new();
        let mut eu_num = HashMap::new();
        let mut eu_num_dead = HashSet::new();
        let mut eu_date: HashMap<(bool, Date), Vec<(String, String)>> = HashMap::new();
        let mut nature_date = HashMap::new();
        let mut nature_date_dead = HashSet::new();
        let mut circ_date: HashMap<Date, Vec<(String, String)>> = HashMap::new();
        let mut nor_index = HashMap::new();
        let mut nor_dead = HashSet::new();
        let mut treaty_date: HashMap<Date, Vec<(String, String)>> = HashMap::new();
        // Candidats accords par (stem gentilé, date) : les avenants et lois
        // d'autorisation réembarquent la date de la convention de base dans leur
        // titre, l'acte de base départage (voir `accord_is_base`).
        let mut accord_cands: HashMap<(String, Date), Vec<(String, bool)>> = HashMap::new();
        let mut foreign_title: HashMap<(String, String), Vec<String>> = HashMap::new();
        let mut foreign_head: HashMap<(String, String), Vec<String>> = HashMap::new();
        let mut act_identity = HashMap::new();
        let mut foreign_owner = HashMap::new();
        let mut kalicont = HashSet::new();
        let mut prefix_agnostic: HashMap<String, HashMap<String, String>> = HashMap::new();

        for t in &texts {
            let folded_title = fold(&t.title);
            let folded_tk = fold(&t.title_key);

            if is_rectificatif(&folded_title) {
                continue;
            }

            if let Some(nor) = &t.nor {
                insert_unique(
                    &mut nor_index,
                    &mut nor_dead,
                    nor.to_uppercase(),
                    &t.text_uid,
                );
            }

            // Circulaires : identité datée en colonne, les titres du fond étant
            // libres (« Modalités d'attribution… »).
            if t.nature == "CIRCULAIRE" {
                if let Some(date) = t.date_texte {
                    insert_unique(
                        &mut nature_date,
                        &mut nature_date_dead,
                        ("circulaire".to_string(), date),
                        &t.text_uid,
                    );
                    circ_date
                        .entry(date)
                        .or_default()
                        .push((t.text_uid.clone(), folded_title.clone()));
                }
            }

            // Actes datés FR : nature + numéro depuis le title brut (le
            // title_key rabote le numéro), nature + date depuis le title_key.
            if let Some(nat) = head_act_nature(&folded_title) {
                act_identity.insert(
                    t.text_uid.clone(),
                    (head_act_num_loose(&folded_title), first_date(&folded_title)),
                );
                if let Some(num) = head_act_num(&folded_title) {
                    insert_unique(
                        &mut nature_num,
                        &mut nature_num_dead,
                        (nat.to_string(), num),
                        &t.text_uid,
                    );
                }
                if let Some(date) = first_date(&folded_title) {
                    insert_unique(
                        &mut nature_date,
                        &mut nature_date_dead,
                        (nat.to_string(), date),
                        &t.text_uid,
                    );
                }
            }

            // Droit dérivé UE : (directive?, année/séquence) depuis le title, et
            // TOUTES les dates du title pour la forme citée sans numéro.
            match t.nature.as_str() {
                "DIRECTIVE_EURO" | "REGLEMENT" => {
                    let dir = t.nature == "DIRECTIVE_EURO";
                    if let Some(m) = RE_SLASHNUM.captures(&folded_title) {
                        let key = (dir, m[1].to_string());
                        insert_unique(&mut eu_num, &mut eu_num_dead, key, &t.text_uid);
                    }
                    for date in all_dates(&folded_title) {
                        let cands = eu_date.entry((dir, date)).or_default();
                        if !cands.iter().any(|(uid, _)| uid == &t.text_uid) {
                            cands.push((t.text_uid.clone(), folded_title.clone()));
                        }
                    }
                }
                _ => {}
            }

            // Accords bilatéraux « Accord franco-<gentilé> du <date> ».
            if let Some(m) = RE_ACCORD_FRANCO.captures(&folded_title) {
                if let Some(date) = first_date(&folded_title) {
                    let stem = gentile_stem(&m[1]);
                    accord_cands
                        .entry((stem, date))
                        .or_default()
                        .push((t.text_uid.clone(), accord_is_base(&folded_title)));
                }
            }
            // « … entre la France et <pays> … » : le pays + chaque date du titre. Le
            // stem (5 chars) fait converger pays et gentilé (« portugal » ↔
            // « portugais », « italie » ↔ « italienne »).
            if treaty_catalog_text(t) {
                if let Some(m) = RE_ENTRE_PAYS.captures(&folded_title) {
                    let stem = gentile_stem(&m[1]);
                    for date in all_dates(&folded_title) {
                        accord_cands
                            .entry((stem.clone(), date))
                            .or_default()
                            .push((t.text_uid.clone(), accord_is_base(&folded_title)));
                    }
                }
            }

            // Traités : chaque date du titre de publication indexe le texte (« faite
            // à La Haye le 25 octobre 1980 », « LE 25-08-1924 »).
            if treaty_catalog_text(t) {
                for date in all_dates(&folded_title) {
                    let cands = treaty_date.entry(date).or_default();
                    if !cands.iter().any(|(uid, _)| uid == &t.text_uid) {
                        cands.push((t.text_uid.clone(), folded_title.clone()));
                    }
                }
            }

            // Textes étrangers : (ISO, famille sans gentilé) — le title_key porte
            // parfois le gentilé (« code civil belge »), parfois non (« Code civil »
            // suisse) ; la famille nue les réunit. Toutes les familles, pas seulement
            // les codes.
            if let Some(iso) = t.jurisdiction.as_deref() {
                if !matches!(iso, "FR" | "EU" | "INTL") {
                    foreign_owner.insert(t.text_uid.clone(), iso.to_string());
                    let fams = home_families(&folded_tk, iso);
                    for head in family_heads(&fams) {
                        push_candidate(&mut foreign_head, (iso.to_string(), head), &t.text_uid);
                    }
                    for base in fams {
                        push_candidate(&mut foreign_title, (iso.to_string(), base), &t.text_uid);
                    }
                }
            }

            if t.text_uid.starts_with("KALICONT") {
                kalicont.insert(t.text_uid.clone());
            }

            if t.num_prefix_agnostic {
                let cores = prefix_agnostic.entry(t.text_uid.clone()).or_default();
                if let Some(nums) = article_sets.get(&t.text_uid) {
                    for num_key in nums {
                        let core = digit_core(num_key);
                        if !core.is_empty() {
                            cores.insert(core, num_key.clone());
                        }
                    }
                }
            }
        }

        // Résolution des accords : clé unique → l'uid ; collision → l'acte de
        // base s'il est seul, sinon clé morte.
        let mut accords: HashMap<(String, Date), String> = HashMap::new();
        for (key, mut cands) in accord_cands {
            cands.sort();
            cands.dedup();
            let uid = match &cands[..] {
                [(uid, _)] => Some(uid),
                _ => {
                    let mut bases = cands.iter().filter(|(_, base)| *base);
                    match (bases.next(), bases.next()) {
                        (Some((uid, _)), None) => Some(uid),
                        _ => None,
                    }
                }
            };
            if let Some(uid) = uid {
                accords.insert(key, uid.clone());
            }
        }

        // Alias embarqués, validés par existence de la cible au catalogue.
        let uids: HashSet<&str> = texts.iter().map(|t| t.text_uid.as_str()).collect();
        let forum_defaults: HashMap<(Forum, &'static str), String> = FORUM_DEFAULTS
            .iter()
            .filter(|(_, _, uid)| uids.contains(uid))
            .map(|(forum, nature, uid)| ((*forum, *nature), uid.to_string()))
            .collect();
        let mut alias_text = HashMap::new();
        // BOFiP : le code BOI cité (« BOI-IR-BASE-10-10 ») EST le `text_uid`
        // catalogue — alias direct, même autorité que le TSV.
        for t in &texts {
            if t.nature == "BOFIP" {
                alias_text.insert(fold(&t.text_uid), t.text_uid.clone());
            }
        }
        let mut alias_article = HashMap::new();
        for line in LINK_ALIASES_TSV.lines() {
            // 4e colonne (`ref_num_key` forcé) optionnelle : l'absence vaut vide.
            let mut cols = line.split('\t');
            let (tk, ak, uid, num) = (
                cols.next().expect("link_aliases.tsv : text_key"),
                cols.next().expect("link_aliases.tsv : article_key"),
                cols.next().expect("link_aliases.tsv : ref_text_uid"),
                cols.next().unwrap_or(""),
            );
            if !uids.contains(uid) {
                continue;
            }
            // Clés pliées (`fold`) des deux côtés du lookup.
            if ak.is_empty() {
                alias_text.insert(fold(tk), uid.to_string());
            } else {
                // La 4e colonne est curée en forme citée ; la clé stockée est la clé
                // publique.
                let forced = (!num.is_empty()).then(|| lj_norm::article_key::article_key(num));
                alias_article.insert((fold(tk), ak.to_string()), (uid.to_string(), forced));
            }
        }

        Self {
            alias_text,
            alias_article,
            title_to_uid,
            title_brothers,
            nature_num,
            eu_num,
            eu_date,
            forum_defaults,
            nature_date,
            circ_date,
            nor: nor_index,
            treaty_date,
            accords,
            foreign_title,
            foreign_head,
            act_identity,
            foreign_owner,
            kalicont,
            articles: article_sets,
            prefix_agnostic,
        }
    }

    /// Cardinalités des index, pour le log d'hydratation.
    pub fn stats(&self) -> String {
        format!(
            "titres={} alias={} nature_num={} eu={} dates={} nor={} traites={} accords={} \
             etrangers={} textes_articles={}",
            self.title_to_uid.len(),
            self.alias_text.len(),
            self.nature_num.len(),
            self.eu_num.len(),
            self.nature_date.len(),
            self.nor.len(),
            self.treaty_date.len(),
            self.accords.len(),
            self.foreign_title.len() + self.foreign_head.len(),
            self.articles.len(),
        )
    }

    /// ISO-2 du texte quand il appartient au référentiel étranger — le for du
    /// corps émetteur, que le composeur passe au résolveur.
    ///
    /// Les collectivités françaises d'outre-mer en sortent : le code des impôts
    /// calédonien porte la juridiction `NC` mais l'uid `FR-NC/…`, et le droit
    /// métropolitain s'y applique — il cite le code civil FRANÇAIS.
    pub fn foreign_iso(&self, text_uid: &str) -> Option<&str> {
        if text_uid.starts_with("FR-") {
            return None;
        }
        self.foreign_owner.get(text_uid).map(String::as_str)
    }

    /// Texte d'un pays donné portant cette famille : famille EXACTE d'abord,
    /// TÊTE de titre ensuite (« code de procédure civile » pour le « code de
    /// procédure civile, commerciale, administrative et financière »
    /// congolais). À plusieurs porteurs, l'article cité tranche.
    fn home_text(&self, iso: &str, family: &str, article_key: Option<&str>) -> HomeText<'_> {
        let key = (iso.to_string(), family.to_string());
        let Some(cands) = self
            .foreign_title
            .get(&key)
            .or_else(|| self.foreign_head.get(&key))
        else {
            return HomeText::Absent;
        };
        if let [uid] = &cands[..] {
            return HomeText::Found(uid);
        }
        let carrier = article_key.and_then(|ak| {
            let mut carriers = cands.iter().filter(|u| self.has_article(u, ak));
            match (carriers.next(), carriers.next()) {
                (Some(uid), None) => Some(uid.as_str()),
                _ => None,
            }
        });
        carrier.map_or(HomeText::Abstain, HomeText::Found)
    }

    /// L'acte candidat contredit-il l'identité citée ? Un acte daté s'identifie
    /// par (nature, numéro, date) : dans le corps d'un texte ÉTRANGER, une
    /// jambe qui matche sur une composante ne peut pas contredire l'autre —
    /// « loi n° 69-5 du 18 février 1969 » (Niger) n'est pas la loi française
    /// n° 69-5 du 3 janvier 1969.
    ///
    /// Hors de ce for, la composante qui matche reste l'identité et absorbe la
    /// coquille de l'autre — la prose française écrit « loi n° 91-647 du
    /// 10 août 1991 » pour celle du 10 juillet.
    ///
    /// Composantes connues des DEUX côtés seulement : la forme courte « la loi
    /// du 10 juillet 1991 » ne réfute aucun numéro.
    fn act_identity_conflicts(&self, uid: &str, num: Option<&str>, date: Option<Date>) -> bool {
        let Some((cat_num, cat_date)) = self.act_identity.get(uid) else {
            return false;
        };
        matches!((num, cat_num.as_deref()), (Some(a), Some(b)) if a != b)
            || matches!((date, *cat_date), (Some(a), Some(b)) if a != b)
    }

    /// L'article existe-t-il au catalogue pour ce texte ? Même frontière
    /// d'alphabet que [`Self::num_key_for`] : clé citée → clé publique avant
    /// consultation du catalogue.
    pub fn has_article(&self, uid: &str, num_key: &str) -> bool {
        let ak = &lj_norm::article_key::article_key(num_key);
        self.articles.get(uid).is_some_and(|s| s.contains(ak))
            || !self.prefixed_variants(uid, ak).is_empty()
    }

    /// Article cité sans son préfixe de partie (« 1233-62 » pour
    /// « L. 1233-62 ») : les variantes préfixées présentes au catalogue. Le
    /// `num_key` ne se pose que si la variante est UNIQUE (L. 521-1 et
    /// R. 521-1 coexistent dans le CJA).
    fn prefixed_variants(&self, uid: &str, num_key: &str) -> Vec<String> {
        if !num_key.starts_with(|c: char| c.is_ascii_digit()) || !num_key.contains('-') {
            return Vec::new();
        }
        let Some(set) = self.articles.get(uid) else {
            return Vec::new();
        };
        ["l", "r", "d", "a"]
            .iter()
            .map(|p| format!("{p}{num_key}"))
            .filter(|cand| set.contains(cand))
            .collect()
    }

    /// Le catalogue connaît-il la STRUCTURE de ce texte ? Faux pour les traités
    /// JORF sans structure : leur décret de publication porte 1–2 articles
    /// boilerplate qui ne disent rien des articles du traité — ≤ 2 = structure
    /// inconnue.
    pub fn has_article_info(&self, uid: &str) -> bool {
        self.articles.get(uid).is_some_and(|s| s.len() > 2)
    }

    pub fn article_sets(&self) -> impl Iterator<Item = (&str, &HashSet<String>)> {
        self.articles.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Cible de l'instrument NU d'un forum (« du règlement de procédure » dans
    /// un arrêt CJUE) — présente seulement si le uid est au catalogue.
    pub fn forum_default<'a>(&'a self, forum: Forum, nature: &str) -> Option<&'a str> {
        self.forum_defaults
            .iter()
            .find(|((f, n), _)| *f == forum && *n == nature)
            .map(|(_, uid)| uid.as_str())
    }

    /// Départage une circulaire homonyme de date par la queue-objet citée
    /// (« relative aux conditions d'examen… ») : les tokens distinctifs de la
    /// queue tous présents dans le titre LIBRE d'UN seul candidat de la date,
    /// sinon abstention.
    pub(crate) fn circulaire_by_object_tail(&self, date: Date, tail: &str) -> Option<String> {
        let cands = self.circ_date.get(&date)?;
        let tokens: Vec<&str> = tail
            .split_whitespace()
            .filter(|w| {
                w.chars().count() >= 4
                    && !w.chars().all(|c| c.is_ascii_digit())
                    && !matches!(
                        *w,
                        "relative"
                            | "relatives"
                            | "relatif"
                            | "relatifs"
                            | "portant"
                            | "concernant"
                    )
            })
            .collect();
        if tokens.is_empty() {
            return None;
        }
        let survivors: Vec<&(String, String)> = cands
            .iter()
            .filter(|(_, title)| tokens.iter().all(|tok| title.contains(tok)))
            .collect();
        match survivors.as_slice() {
            [(uid, _)] => Some(uid.clone()),
            _ => None,
        }
    }

    /// La clé est un alias texte embarqué : une forme datée nue ainsi curée
    /// passe le gate d'acte individuel. La forme canonique compte aussi
    /// (qualificatif de circulaire raboté) : c'est le même acte.
    pub(crate) fn has_text_alias(&self, text_key: &str) -> bool {
        self.alias_text.contains_key(&fold(text_key))
            || strip_circulaire_qualifier(text_key)
                .is_some_and(|k| self.alias_text.contains_key(&fold(&k)))
    }

    /// `ref_num_key` par existence au catalogue, sinon via la table
    /// préfixe-agnostique du texte. Jamais inventé. Frontière d'alphabet : la
    /// clé citée arrive en forme `normalize_article` (« L. 761-1 »), le
    /// catalogue est en clé publique slug — conversion ici.
    pub fn num_key_for(&self, uid: &str, article_key: Option<&str>) -> Option<String> {
        let ak = &lj_norm::article_key::article_key(article_key?);
        if ak.is_empty() {
            return None;
        }
        if self.articles.get(uid).is_some_and(|s| s.contains(ak)) {
            return Some(ak.to_string());
        }
        if let [unique] = &self.prefixed_variants(uid, ak)[..] {
            return Some(unique.clone());
        }
        let cores = self.prefix_agnostic.get(uid)?;
        let core = digit_core(ak);
        if core.is_empty() {
            return None;
        }
        cores.get(&core).cloned()
    }
}

/// Analyse par clé (pliages + signaux `key_signals`) — la partie coûteuse de
/// la résolution, qui ne dépend QUE de `text_key`. À mémoïser par l'appelant
/// quand la même clé résout plusieurs articles.
pub struct KeyAnalysis {
    lower: String,
    folded: String,
    sig: KeySignals,
}

impl KeyAnalysis {
    pub fn new(text_key: &str) -> Self {
        Self {
            lower: text_key.to_lowercase(),
            folded: fold(text_key),
            sig: key_signals(text_key),
        }
    }

    pub fn signals(&self) -> &KeySignals {
        &self.sig
    }
}

/// Résout une occurrence : `instrument` = forme brute capturée (porte le
/// numéro d'acte que `text_key` rabote), `text_key`/`article_key` = clés
/// canoniques. `owner_iso` = ISO-2 du for du corps émetteur quand il vient du
/// référentiel étranger, `None` pour une décision. Déterministe, total.
pub fn link_citation(
    snap: &LinkSnapshot,
    instrument: &str,
    text_key: &str,
    article_key: Option<&str>,
    owner_iso: Option<&str>,
) -> LinkTarget {
    link_citation_analyzed(
        snap,
        instrument,
        &KeyAnalysis::new(text_key),
        article_key,
        owner_iso,
    )
}

/// Variante à analyse fournie (voir [`KeyAnalysis`]).
pub fn link_citation_analyzed(
    snap: &LinkSnapshot,
    instrument: &str,
    analysis: &KeyAnalysis,
    article_key: Option<&str>,
    owner_iso: Option<&str>,
) -> LinkTarget {
    let tk = &analysis.lower;
    let folded = &analysis.folded;

    // 1. Alias embarqués — autorité curée, article-niveau d'abord.
    if let Some(ak) = article_key {
        if let Some((uid, forced)) = snap.alias_article.get(&(folded.clone(), ak.to_string())) {
            let num = forced
                .clone()
                .or_else(|| snap.num_key_for(uid, article_key));
            return LinkTarget {
                ref_text_uid: Some(uid.clone()),
                ref_num_key: num,
            };
        }
    }
    // Le for du texte émetteur prime l'alias de TEXTE : « constitution » est
    // aliasé sur celle du 4 octobre 1958, ce qui est vrai dans un document
    // français et faux dans la constitution ukrainienne.
    let mut home = owner_iso.map_or(HomeText::Absent, |iso| {
        snap.home_text(iso, folded, article_key)
    });
    // 2ter. Famille de code NUE que le pays émetteur ne porte pas au
    // catalogue : abstention.
    if home == HomeText::Absent && owner_iso.is_some() && analysis.sig.nature == KeyNature::Code {
        home = HomeText::Abstain;
    }
    if home == HomeText::Absent {
        if let Some(uid) = snap.alias_text.get(folded) {
            return LinkTarget {
                ref_text_uid: Some(uid.clone()),
                ref_num_key: snap.num_key_for(uid, article_key),
            };
        }
    }

    // 1bis. NOR porté par la forme brute (« circulaire NOR JUSK1140023C du
    // 14 avril 2011 ») : identifiant interministériel unique, avant le gate.
    if let Some(uid) = nor_in_raw(instrument).and_then(|nor| snap.nor.get(&nor)) {
        return LinkTarget {
            ref_text_uid: Some(uid.clone()),
            ref_num_key: snap.num_key_for(uid, article_key),
        };
    }

    // 2. Gate de citabilité.
    let sig = &analysis.sig;
    if sig.citability != Citability::Citable {
        return LinkTarget::default();
    }
    let uid = linked_text_uid(
        snap,
        instrument,
        tk,
        folded,
        &sig.nature,
        sig.jurisdiction,
        sig.act_num.as_deref(),
        sig.act_date,
        article_key,
        home,
        owner_iso,
    );
    match uid {
        Some(uid) => {
            let num = snap.num_key_for(&uid, article_key);
            LinkTarget {
                ref_text_uid: Some(uid),
                ref_num_key: num,
            }
        }
        // 10. Qualificatif de rédaction NON distinctif au catalogue : « circulaire
        // ministérielle du <date> » cite le même acte que « circulaire du
        // <date> ». On retente la clé sans lui, avec la même discipline (alias,
        // nature+date unique, queue-objet). Restreint aux circulaires : « loi
        // organique » est une nature à part au catalogue.
        None => match strip_circulaire_qualifier(tk) {
            Some(stripped) => link_citation_analyzed(
                snap,
                &stripped,
                &KeyAnalysis::new(&stripped),
                article_key,
                owner_iso,
            ),
            None => LinkTarget::default(),
        },
    }
}

/// « Circulaire ministérielle du 28 novembre 2012 » → « Circulaire du 28
/// novembre 2012 ». Découpe au mot (le pli ne décale aucun offset), casse et
/// accents du reste de la clé préservés. `None` quand la clé ne porte pas de
/// qualificatif de circulaire.
///
/// Partagé avec les départages doc-contextuels du composeur (queue-objet,
/// adoption intra-document, gate d'acte individuel) : tous voient la forme
/// canonique.
pub(crate) fn strip_circulaire_qualifier(text_key: &str) -> Option<String> {
    let words: Vec<&str> = text_key.split_whitespace().collect();
    let (head, rest) = words.split_first()?;
    if fold(head) != "circulaire" {
        return None;
    }
    let qualifier = rest
        .first()
        .is_some_and(|w| matches!(fold(w).as_str(), "ministerielle" | "interministerielle"));
    let rest = &rest[if qualifier { 1 } else { author_len(rest)? }..];
    if rest.is_empty() {
        return None;
    }
    Some(format!("{head} {}", rest.join(" ")))
}

/// Words of the author a circulaire names before its date, « du ministre de
/// l'intérieur », up to the « du » the date opens with.
fn author_len(words: &[&str]) -> Option<usize> {
    let folded: Vec<String> = words.iter().map(|w| fold(w)).collect();
    let mut at = match folded.first()?.as_str() {
        "du" | "des" => 1,
        "de" if folded.get(1).is_some_and(|w| w == "la") => 2,
        _ => return None,
    };
    if folded
        .get(at)
        .is_some_and(|w| w == "premier" || w == "premiere")
    {
        at += 1;
    }
    if !matches!(
        folded.get(at)?.as_str(),
        "ministre" | "ministres" | "ministere"
    ) {
        return None;
    }
    (at + 1..folded.len().saturating_sub(1))
        .find(|&i| folded[i] == "du" && folded[i + 1].starts_with(|c: char| c.is_ascii_digit()))
}

/// Cœur de la résolution texte, jambes 3 à 9.
#[allow(clippy::too_many_arguments)]
fn linked_text_uid(
    snap: &LinkSnapshot,
    instrument: &str,
    tk: &str,
    folded: &str,
    nature: &KeyNature,
    jurisdiction: Option<&str>,
    act_num: Option<&str>,
    act_date: Option<Date>,
    article_key: Option<&str>,
    home: HomeText<'_>,
    owner_iso: Option<&str>,
) -> Option<String> {
    // 2bis. Texte du pays émetteur : le corps d'un texte étranger qui cite une
    // famille NUE (« du Code de procédure pénale », « la Constitution ») vise le
    // texte de son propre pays. Une clé qui NOMME un pays (« code pénal
    // marocain ») ne coïncide avec aucune famille nue et suit sa route normale.
    match home {
        HomeText::Found(uid) => return Some(uid.to_string()),
        HomeText::Abstain => return None,
        HomeText::Absent => {}
    }

    // 3. Acte daté par numéro — prime sur le titre (les frères d'une même date
    // collapsent sur le même title_key, le numéro tranche).
    let dated_shape = RE_DATED_ACT_SHAPE.is_match(folded);
    let raw = dated_shape.then(|| fold(instrument));
    let head_num = raw.as_deref().and_then(head_act_num);
    // Composantes d'identité opposables — seulement sous le for d'un texte
    // étranger. Le numéro cité est lu en graphie LIBRE : les numérotations non
    // françaises (« loi n° 5982 » turque) échappent au format `NN-NNN`.
    let (cited_num, cited_date) = match owner_iso {
        Some(_) => (raw.as_deref().and_then(head_act_num_loose), act_date),
        None => (None, None),
    };
    if let (Some(nat), Some(num)) = (head_act_nature(folded), head_num.as_deref()) {
        if let Some(uid) = snap.nature_num.get(&(nat.to_string(), num.to_string())) {
            if !snap.act_identity_conflicts(uid, cited_num.as_deref(), cited_date) {
                return Some(uid.clone());
            }
        }
    }

    // 4. Titre exact, texte vivant — y compris pour les clés datées dont le
    // rabotage collapse des frères d'un même jour. Le marqueur éditorial
    // « modifié(e) » de la forme citée n'est pas une identité : retenté sans
    // lui. L'article cité tranche entre frères d'un même title_key raboté : si
    // le vivant ignore l'article et qu'EXACTEMENT un frère le porte, le frère
    // l'emporte.
    let brother_pick = |uid: &String| -> String {
        if let Some(ak) = article_key {
            if !snap.has_article(uid, ak) {
                if let Some(bros) = snap.title_brothers.get(folded) {
                    let mut carriers = bros.iter().filter(|u| snap.has_article(u, ak));
                    if let (Some(b), None) = (carriers.next(), carriers.next()) {
                        return b.clone();
                    }
                }
            }
        }
        uid.clone()
    };
    if let Some(uid) = snap.title_to_uid.get(folded) {
        let pick = brother_pick(uid);
        if !snap.act_identity_conflicts(&pick, cited_num.as_deref(), cited_date) {
            return Some(pick);
        }
    }
    if folded.contains(" modifie") {
        let sans_modifie = strip_word(folded, |w| {
            matches!(w, "modifie" | "modifiee" | "modifies" | "modifiees")
        });
        if let Some(uid) = snap.title_to_uid.get(&sans_modifie) {
            if !snap.act_identity_conflicts(uid, cited_num.as_deref(), cited_date) {
                return Some(uid.clone());
            }
        }
    }

    // 5. Droit dérivé UE.
    if let (KeyNature::DirectiveUe | KeyNature::ReglementUe, Some(num)) = (nature, act_num) {
        let key = (*nature == KeyNature::DirectiveUe, num.to_string());
        if let Some(uid) = snap.eu_num.get(&key) {
            return Some(uid.clone());
        }
    }

    // 5bis. Droit dérivé UE par date + tokens — « règlement du 17 décembre 2013
    // établissant les règles relatives aux paiements directs » : la queue de
    // titre citée départage les règlements de la même date (UN survivant ou
    // abstention).
    if act_num.is_none() && (folded.starts_with("reglement") || folded.starts_with("directive")) {
        if let Some(cands) =
            act_date.and_then(|d| snap.eu_date.get(&(folded.starts_with("directive"), d)))
        {
            let tokens: Vec<&str> = folded
                .split_whitespace()
                .filter(|w| {
                    w.chars().count() >= 4
                        && month_num(w).is_none()
                        && !w.chars().all(|c| c.is_ascii_digit())
                })
                .collect();
            let survivors: Vec<&(String, String)> = cands
                .iter()
                .filter(|(_, title)| tokens.iter().all(|tok| title.contains(tok)))
                .collect();
            if let [(uid, _)] = survivors.as_slice() {
                return Some(uid.clone());
            }
        }
    }

    // 6. Accord bilatéral par (gentilé, date) — « accord franco-algérien du
    // 27 décembre 1968 ».
    if *nature == KeyNature::TraiteAccord
        && (folded.starts_with("accord") || folded.starts_with("convention"))
    {
        if let Some(date) = act_date {
            for w in folded.split_whitespace() {
                let w = w.trim_start_matches("franco-");
                if is_gentile_word(w) {
                    if let Some(uid) = snap.accords.get(&(gentile_stem(w), date)) {
                        return Some(uid.clone());
                    }
                }
            }
            if let Some(m) = RE_ENTRE_PAYS.captures(folded) {
                if let Some(uid) = snap.accords.get(&(gentile_stem(&m[1]), date)) {
                    return Some(uid.clone());
                }
            }
        }
    }

    // 6bis. Traité par date + tokens : « convention de Vienne du 11 avril 1980
    // sur les contrats de vente… » ↔ « …portant publication de la convention…,
    // faite à Vienne le 11 avril 1980 ». Les tokens distinctifs de la clé citée
    // (lieu, matière) départagent — lié seulement si UN candidat de la date les
    // contient tous.
    if *nature == KeyNature::TraiteAccord {
        if let Some(cands) = act_date.and_then(|d| snap.treaty_date.get(&d)) {
            // Tokens distinctifs : mois et nombres exclus, déjà encodés dans le
            // pivot date.
            let tokens: Vec<&str> = folded
                .split_whitespace()
                .filter(|w| {
                    w.chars().count() >= 4
                        && month_num(w).is_none()
                        && !w.chars().all(|c| c.is_ascii_digit())
                })
                .collect();
            let mut survivors: Vec<&(String, String)> = cands
                .iter()
                .filter(|(_, title)| tokens.iter().all(|tok| title.contains(tok)))
                .collect();
            if survivors.len() > 1 {
                // Tie-break : les actes accessoires réembarquent le titre de base
                // (protocole additionnel, adhésion, loi autorisant la ratification).
                // Éliminé si le titre porte un mot-instrument absent de la clé citée ;
                // le décret de publication de base survit. Les parenthèses (annexes
                // embarquées) sont retirées avant le test.
                let narrowed: Vec<&(String, String)> = survivors
                    .iter()
                    .filter(|(_, title)| {
                        let bare = strip_parens(title);
                        !TREATY_WORDS
                            .iter()
                            .any(|w| bare.contains(w) && !folded.contains(w))
                    })
                    .copied()
                    .collect();
                if !narrowed.is_empty() {
                    survivors = narrowed;
                }
            }
            if let [(uid, _)] = survivors.as_slice() {
                return Some(uid.clone());
            }
        }
    }

    // 7. CCN via gazetteer (squelette de tokens).
    if *nature == KeyNature::Ccn {
        if let Some(entry) = gazetteer().snap(tk) {
            if snap.kalicont.contains(&entry.kalicont) {
                return Some(entry.kalicont.clone());
            }
        }
    }

    // 8. Code étranger par (ISO, famille sans gentilé).
    if *nature == KeyNature::CodeEtranger {
        if let Some(iso) = jurisdiction {
            let base = strip_gentile_words(folded);
            match snap.home_text(iso, &base, article_key) {
                HomeText::Found(uid) => return Some(uid.to_string()),
                HomeText::Abstain => return None,
                HomeText::Absent => {}
            }
        }
    }

    // 9. Acte daté par date — forme courte sans numéro ; jamais quand la forme
    // brute porte un numéro.
    if dated_shape && head_num.is_none() {
        if let (Some(nat), Some(date)) = (head_act_nature(folded), act_date) {
            if let Some(uid) = snap
                .nature_date
                .get(&(nat.to_string(), date))
                .filter(|uid| !snap.act_identity_conflicts(uid, cited_num.as_deref(), cited_date))
            {
                return Some(uid.clone());
            }
            // 9bis. Circulaires homonymes de date : la queue-objet de la clé, quand
            // elle en porte une, départage contre les titres LIBRES des candidates.
            // Le cas de la queue en TEXTE après la mention passe par
            // [`LinkSnapshot::circulaire_by_object_tail`].
            if nat == "circulaire" {
                if let Some(tail) = RE_CIRC_TAIL
                    .captures(folded)
                    .map(|c| c.get(1).expect("groupe queue").as_str())
                {
                    if let Some(uid) = snap.circulaire_by_object_tail(date, tail) {
                        return Some(uid);
                    }
                }
            }
        }
    }

    None
}

/// Queue-objet d'une clé de circulaire datée : ce qui suit « circulaire du
/// <date> ».
static RE_CIRC_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^circulaire du \d{1,2}(?:er)? (?:janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre) \d{4}\s+(.+)$",
    )
    .unwrap()
});

/// NOR dans la forme brute capturée : « NOR JUSK1140023C », « NOR :
/// INTK9700174C », « NORINTK1207286C » (graphie collée). Majuscules exigées —
/// la graphie officielle. Préfiltre `contains` avant la regex.
fn nor_in_raw(instrument: &str) -> Option<String> {
    if !instrument.contains("NOR") {
        return None;
    }
    RE_NOR_IN_RAW.captures(instrument).map(|c| c[1].to_string())
}

static RE_NOR_IN_RAW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bNOR\s*:?\s*([A-Z]{4}\d{7}[A-Z])\b").unwrap());

/// Nature d'acte daté en tête de chaîne pliée.
///
/// « Loi du pays » est une nature à part, testée avant « loi » : la
/// délibération du congrès calédonien partage la numérotation
/// `année-séquence` du JORF sans partager ses actes.
fn head_act_nature(folded: &str) -> Option<&'static str> {
    [
        "loi du pays",
        "decret",
        "loi",
        "arrete",
        "ordonnance",
        "decision",
        "deliberation",
        "circulaire",
    ]
    .into_iter()
    .find(|nat| folded.starts_with(nat))
}

/// Numéro d'acte FR ancré en tête : nature, déterminants tolérés, borne
/// `[^0-9]{0,40}` avant le « n° » — un numéro imbriqué après une date
/// interposée n'est pas capté.
fn head_act_num(folded: &str) -> Option<String> {
    RE_HEAD_ACT_NUM.captures(folded).map(|c| c[1].to_string())
}

/// Numéro d'acte ancré en tête, graphie LIBRE : ce que la mention écrit après
/// « n° », hors format FR (« n° 5982 » d'une loi turque, « n° 073/84 » d'une
/// loi congolaise). Sert l'identité, pas la résolution.
fn head_act_num_loose(folded: &str) -> Option<String> {
    RE_HEAD_ACT_NUM_LOOSE
        .captures(folded)
        .map(|c| c[1].to_string())
}

/// Retire d'une chaîne pliée les mots vérifiant `drop`, en re-joignant par
/// espaces simples.
fn strip_word(folded: &str, drop: impl Fn(&str) -> bool) -> String {
    folded
        .split_whitespace()
        .filter(|w| !drop(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Toutes les dates d'une chaîne pliée : en toutes lettres après « du »/« le »
/// (« le 25 octobre 1980 ») et numériques des titres JORF historiques
/// (« LE 25-08-1924 »). Dédupliquées, ordre stable.
fn all_dates(folded: &str) -> Vec<Date> {
    let mut out: Vec<Date> = Vec::new();
    let mut push = |d: Option<Date>| {
        if let Some(d) = d {
            if !out.contains(&d) {
                out.push(d);
            }
        }
    };
    for c in RE_DATE.captures_iter(folded) {
        push(date_from_words(&c[1], &c[2], &c[3]));
    }
    for c in RE_DATE_NUM.captures_iter(folded) {
        let d = || -> Option<Date> {
            Date::new(c[3].parse().ok()?, c[2].parse().ok()?, c[1].parse().ok()?).ok()
        };
        push(d());
    }
    out
}

fn date_from_words(day: &str, month: &str, year: &str) -> Option<Date> {
    let day: i8 = day.parse().ok()?;
    let month = month_num(month)?;
    let year: i16 = year.parse().ok()?;
    Date::new(year, month, day).ok()
}

/// Première date « du/le <jour> <mois> <année> » d'une chaîne pliée.
pub(crate) fn first_date(folded: &str) -> Option<Date> {
    let c = RE_DATE.captures(folded)?;
    date_from_words(&c[1], &c[2], &c[3])
}

fn month_num(m: &str) -> Option<i8> {
    Some(match m {
        "janvier" => 1,
        "fevrier" => 2,
        "mars" => 3,
        "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" => 7,
        "aout" => 8,
        "septembre" => 9,
        "octobre" => 10,
        "novembre" => 11,
        "decembre" => 12,
        _ => return None,
    })
}

/// Un mot plié porte-t-il un gentilé étranger ?
fn is_gentile_word(w: &str) -> bool {
    FOREIGN_NATIONALITY_STEMS.iter().any(|st| w.starts_with(st))
}

/// Stem d'appariement d'un gentilé (5 premiers chars pliés) : « algerien » et
/// « algerienne » convergent, « maroc » ↔ « marocain » aussi.
fn gentile_stem(gentile: &str) -> String {
    gentile.chars().take(5).collect()
}

/// Familles nues sous lesquelles indexer le titre d'un texte étranger : ce
/// qu'une citation du pays lui-même peut écrire. Le titre nomme son pays de
/// deux façons — par le gentilé (« code civil belge ») ou par le nom de
/// l'État (« Code civil de la République de Djibouti ») ; seules les
/// désignations du pays du texte LUI-MÊME tombent.
fn home_families(folded_tk: &str, iso: &str) -> Vec<String> {
    let mut out = vec![strip_gentile_words(folded_tk)];
    if let Some((codes, stripped)) = lj_norm::jurisdictions::strip_query_jurisdictions(folded_tk) {
        if codes.contains(&iso) {
            out.push(drop_dangling_prepositions(&stripped));
        }
    }
    out.retain(|v| !v.is_empty());
    out.sort();
    out.dedup();
    out
}

/// Têtes de famille sous lesquelles indexer AUSSI un titre étranger. Le
/// catalogue nomme un code par sa matière complète (« code de procédure
/// civile, commerciale, administrative et financière ») quand le corps le
/// cite par sa tête (« le code de procédure civile »). Préfixes stricts,
/// bornés : tête NOMMANTE, jamais un préfixe qui s'arrête sur un mot-outil
/// (« code de »), ≥ 2 mots hors « constitution » — qui se cite seul.
fn family_heads(families: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for fam in families {
        let words: Vec<&str> = fam.split_whitespace().collect();
        let Some(head) = words.first() else { continue };
        if !matches!(*head, "code" | "livre" | "constitution") {
            continue;
        }
        let min = if *head == "constitution" { 1 } else { 2 };
        for n in min..words.len() {
            if is_link_word(words[n - 1]) {
                continue;
            }
            let prefix = words[..n].join(" ");
            if !out.contains(&prefix) {
                out.push(prefix);
            }
        }
    }
    out
}

/// Mot-outil : ne termine ni une tête de famille (« code de », « … civile
/// et ») ni un titre amputé de son pays.
fn is_link_word(w: &str) -> bool {
    matches!(
        w,
        "de" | "du" | "des" | "d" | "la" | "le" | "les" | "l" | "et" | "ou" | "a" | "au" | "aux"
    )
}

/// Préposition orpheline laissée par le retrait du nom de pays (« code civil
/// de djibouti » → « code civil de » → « code civil »).
fn drop_dangling_prepositions(folded: &str) -> String {
    let mut words: Vec<&str> = folded.split_whitespace().collect();
    while words.last().is_some_and(|w| is_link_word(w)) {
        words.pop();
    }
    words.join(" ")
}

/// Retire les mots-gentilés d'une clé pliée (« code civil suisse » → « code
/// civil ») — la famille nue, comparable entre clé citée et title_key.
fn strip_gentile_words(folded: &str) -> String {
    folded
        .split_whitespace()
        .filter(|w| !is_gentile_word(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rectificatif : correction d'édition du JO, jamais l'acte que la
/// jurisprudence vise. Formes sûres seulement : suffixe « (rectificatif) » et
/// titre en tête « Rectificatif … » — un acte numéroté « décret rectificatif
/// du décret X » garde son identité propre.
fn is_rectificatif(folded_title: &str) -> bool {
    folded_title.contains("(rectificatif)") || folded_title.starts_with("rectificatif")
}

fn title_names_text(folded_tk: &str) -> bool {
    let mut words = folded_tk.split_whitespace();
    let head = words.next().unwrap_or("");
    matches!(head, "code" | "livre" | "constitution")
        || words.nth(1).is_some()
        || folded_tk.bytes().any(|b| b.is_ascii_digit())
}

/// Cœur numérique d'un `num_key` : préfixe d'instrument retiré jusqu'au
/// premier chiffre.
fn digit_core(num_key: &str) -> String {
    match num_key.find(|c: char| c.is_ascii_digit()) {
        Some(i) => num_key[i..].to_string(),
        None => String::new(),
    }
}

static RE_DATED_ACT_SHAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:decret|loi|arrete|ordonnance|decision|deliberation|circulaire)(?: organique)?(?: du pays)? du \d").unwrap()
});
static RE_HEAD_ACT_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:le |la |les |l' ?|du |de la |de l' ?)?\s*(?:decret|loi|arrete|ordonnance|decision|deliberation|circulaire)[^0-9]{0,40}n[o°]? ?(\d{2,4}-\d+)").unwrap()
});
static RE_HEAD_ACT_NUM_LOOSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:le |la |les |l' ?|du |de la |de l' ?)?\s*(?:decret|loi|arrete|ordonnance|decision|deliberation|circulaire)[^0-9]{0,40}n[o°]? ?(\d[\d/-]*)").unwrap()
});
static RE_SLASHNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{1,4}/\d{1,4})").unwrap());
/// Marqueurs d'instrument conventionnel — pour le tie-break traité : un titre
/// en portant un ABSENT de la clé citée publie un instrument accessoire
/// (protocole additionnel, adhésion d'un État tiers, loi d'autorisation de
/// ratification…).
const TREATY_WORDS: &[&str] = &[
    "protocole",
    "avenant",
    "amendement",
    "convention",
    "accord",
    "charte",
    "pacte",
    "traite",
    "declaration",
    "adhesion",
    "echange de lettres",
    "reserves",
    "denonciation",
    "arrangement",
    "autorisant",
    "approbation",
];

/// Retire les segments parenthésés d'un titre plié — « (ensemble un protocole
/// et deux déclarations communes) » décrit des annexes embarquées, pas
/// l'instrument publié.
fn strip_parens(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut depth = 0usize;
    for c in title.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Acte de BASE d'un accord bilatéral : la publication de la convention
/// elle-même, ni un avenant ou protocole modificatif, ni une loi
/// d'autorisation. Départage les collisions (stem, date).
fn accord_is_base(folded_title: &str) -> bool {
    !["avenant", "modifiant", "autorisant", "approbation"]
        .iter()
        .any(|w| folded_title.contains(w))
}

/// Fiches des jambes traité : nature native `TRAITE` (DILA) ou `TI` (KALI,
/// accords collectifs cités « accord du <date> »), ou juridiction `INTL` —
/// une fiche instrument porte sa nature honnête (DUDH `DECLARATION`), son
/// appartenance aux jambes se lit sur la juridiction. Les natures JORF
/// `CONVENTION`/`AVENANT`/`ACCORD`/`PROTOCOLE` restent dehors : du
/// conventionnel administratif domestique.
fn treaty_catalog_text(t: &CatalogText) -> bool {
    matches!(t.nature.as_str(), "TRAITE" | "TI") || t.jurisdiction.as_deref() == Some("INTL")
}

static RE_ACCORD_FRANCO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^accord franco-([a-z]+)").unwrap());
/// « entre la France et <pays> » — côté catalogue (« entre le Gouvernement de
/// la République française et le Gouvernement de la République du Sénégal »)
/// comme côté cité (« entre la France et le Portugal »).
static RE_ENTRE_PAYS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"entre (?:la france|le gouvernement de la republique francaise) et (?:le gouvernement |la |le |l' ?)?(?:de la republique |du royaume |de l'etat )?(?:du |de la |de |d' ?)?([a-z][a-z-]+)",
    )
    .unwrap()
});
static RE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:du|le) (\d{1,2})(?:er)? (janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre) (\d{4})").unwrap()
});
static RE_DATE_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{1,2})-(\d{2})-(\d{4})\b").unwrap());

#[cfg(test)]
mod tests {
    use super::*;

    fn text(
        uid: &str,
        title: &str,
        title_key: &str,
        nature: &str,
        jur: Option<&str>,
        n_vigueur: i64,
    ) -> CatalogText {
        CatalogText {
            text_uid: uid.to_string(),
            title: title.to_string(),
            title_key: title_key.to_string(),
            nature: nature.to_string(),
            jurisdiction: jur.map(str::to_string),
            num_prefix_agnostic: false,
            n_vigueur,
            date_texte: None,
            nor: None,
        }
    }

    fn snap() -> LinkSnapshot {
        LinkSnapshot::build(
            vec![
                text("LEGITEXT_CC", "Code civil", "Code civil", "CODE", Some("FR"), 2800),
                text("fedlex/cc", "Code civil (Suisse), RS 210", "Code civil", "code_civil", Some("CH"), 0),
                text("belgian/cc", "Code civil belge (Ancien Code civil)", "code civil belge", "code_civil", Some("BE"), 0),
                text("JORF_CONST58", "Constitution du 4 octobre 1958", "Constitution du 4 octobre 1958", "CONSTITUTION", Some("FR"), 108),
                text("ukr/const", "Constitution ukrainienne", "Constitution ukrainienne", "CONSTITUTION", Some("UA"), 0),
                text(
                    "JORFTEXT_AJ",
                    "Loi n° 91-647 du 10 juillet 1991 relative à l'aide juridique",
                    "Loi du 10 juillet 1991 relative à l'aide juridique",
                    "LOI",
                    Some("FR"),
                    60,
                ),
                text(
                    "JORFTEXT_FONC",
                    "Loi n° 83-634 du 13 juillet 1983 portant droits et obligations des fonctionnaires",
                    "Loi du 13 juillet 1983 portant droits et obligations des fonctionnaires",
                    "LOI",
                    Some("FR"),
                    40,
                ),
                text(
                    "JORFTEXT_D609",
                    "Décret n° 73-609 du 5 juillet 1973 relatif à la formation",
                    "Décret du 5 juillet 1973 relatif à la formation",
                    "DECRET",
                    Some("FR"),
                    10,
                ),
                text(
                    "JORFTEXT_D611",
                    "Décret n° 73-611 du 5 juillet 1973 relatif aux concours",
                    "Décret du 5 juillet 1973 relatif aux concours",
                    "DECRET",
                    Some("FR"),
                    30,
                ),
                text(
                    "CELEX_DUBLIN",
                    "Règlement (UE) n° 604/2013 du Parlement européen et du Conseil",
                    "Règlement (UE) n° 604/2013",
                    "REGLEMENT",
                    Some("EU"),
                    50,
                ),
                text(
                    "JORFTEXT_ACCDZ",
                    "Accord franco-algérien du 27 décembre 1968 relatif à la circulation",
                    "Accord franco-algérien du 27 décembre 1968",
                    "TRAITE",
                    Some("INTL"),
                    12,
                ),
                text(
                    "JORFTEXT_DUDH",
                    "Déclaration universelle des droits de l'homme du 10 décembre 1948",
                    "Déclaration universelle des droits de l'homme du 10 décembre 1948",
                    "DECLARATION",
                    Some("INTL"),
                    0,
                ),
                text(
                    "JORFTEXT_HAYE80",
                    "Décret n° 83-1021 du 29 novembre 1983 portant publication de la convention \
                     sur les aspects civils de l'enlèvement international d'enfants, faite à La \
                     Haye le 25 octobre 1980 (1)",
                    "Décret du 29 novembre 1983",
                    "TRAITE",
                    Some("INTL"),
                    45,
                ),
                text(
                    "JORFTEXT_BRUX24",
                    "Décret du 25 mars 1937 PORTANT PROMULGATION DE LA CONVENTION INTERNATIONALE \
                     POUR L'UNIFICATION DE CERTAINES REGLES EN MATIERE DE CONNAISSEMENT SIGNEE A \
                     BRUXELLES LE 25-08-1924",
                    "Décret du 25 mars 1937",
                    "TRAITE",
                    Some("INTL"),
                    17,
                ),
                text(
                    "JORFTEXT_ROME80",
                    "Décret n°91-242 du 28 février 1991 portant publication de la convention \
                     sur la loi applicable aux obligations contractuelles (ensemble un \
                     protocole et deux déclarations communes), ouverte à la signature à Rome \
                     le 19 juin 1980",
                    "Décret du 28 février 1991",
                    "TRAITE",
                    Some("INTL"),
                    0,
                ),
                text(
                    "JORFTEXT_LROME80",
                    "Loi n°82-523 du 21 juin 1982 AUTORISANT LA RATIFICATION DE LA CONVENTION \
                     SUR LA LOI APPLICABLE AUX OBLIGATIONS CONTRACTUELLES, SIGNEE A ROME LE \
                     19-06-1980",
                    "Loi du 21 juin 1982",
                    "TRAITE",
                    Some("INTL"),
                    2,
                ),
                text(
                    "JORFTEXT_ONOTAIRE",
                    "Ordonnance n° 45-2590 du 2 novembre 1945 relative au statut du notariat",
                    "Ordonnance du 2 novembre 1945",
                    "ORDONNANCE",
                    Some("FR"),
                    80,
                ),
                text(
                    "JORFTEXT_OETRANGERS",
                    "Ordonnance n°45-2658 du 2 novembre 1945 RELATIVE A L'ENTREE ET AU SEJOUR \
                     DES ETRANGERS EN FRANCE",
                    "Ordonnance du 2 novembre 1945",
                    "ORDONNANCE",
                    Some("FR"),
                    20,
                ),
            ],
            vec![
                ("LEGITEXT_CC".to_string(), "1240".to_string()),
                ("JORFTEXT_AJ".to_string(), "37".to_string()),
                ("CELEX_DUBLIN".to_string(), "17".to_string()),
            ],
        )
    }

    #[test]
    fn exact_title_links_living_text_and_validates_article() {
        let s = snap();
        let t = link_citation(&s, "le code civil", "Code civil", Some("1240"), None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("LEGITEXT_CC"));
        assert_eq!(t.ref_num_key.as_deref(), Some("1240"));
        let t = link_citation(&s, "le code civil", "Code civil", Some("9999"), None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("LEGITEXT_CC"));
        assert_eq!(t.ref_num_key, None);
    }

    #[test]
    fn short_descriptive_title_never_links() {
        let s = LinkSnapshot::build(
            vec![
                text(
                    "JORF_DECL",
                    "Déclaration",
                    "Déclaration",
                    "DELIBERATION",
                    Some("FR"),
                    0,
                ),
                text(
                    "JORF_TRIP",
                    "Accord tripartite",
                    "Accord tripartite",
                    "ACCORD",
                    Some("FR"),
                    0,
                ),
                text(
                    "LEGI_MINIER",
                    "Code minier",
                    "Code minier",
                    "CODE",
                    Some("FR"),
                    100,
                ),
            ],
            vec![],
        );
        let t = link_citation(&s, "la déclaration", "Déclaration", None, None);
        assert_eq!(t.ref_text_uid, None);
        let t = link_citation(&s, "l'accord tripartite", "Accord tripartite", None, None);
        assert_eq!(t.ref_text_uid, None);
        let t = link_citation(&s, "le code minier", "Code minier", None, None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("LEGI_MINIER"));
    }

    #[test]
    fn circulaire_date_ambigue_departagee_par_queue_objet() {
        let mk = |uid: &str, title: &str| {
            let mut t = text(uid, title, title, "CIRCULAIRE", Some("FR"), 0);
            t.date_texte = "2012-11-28".parse().ok();
            t
        };
        let s = LinkSnapshot::build(
            vec![
                mk(
                    "cir_11111",
                    "Conditions d'examen des demandes d'admission au séjour déposées par \
                     des ressortissants étrangers en situation irrégulière",
                ),
                mk(
                    "cir_22222",
                    "Examen professionnel d'accès au corps des secrétaires administratifs",
                ),
            ],
            vec![],
        );
        let t = link_citation(
            &s,
            "la circulaire du 28 novembre 2012 relative aux conditions d'examen des \
             demandes d'admission au séjour",
            "Circulaire du 28 novembre 2012 relative aux conditions d'examen des \
             demandes d'admission au séjour",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("cir_11111"));

        let t = link_citation(
            &s,
            "la circulaire du 28 novembre 2012",
            "Circulaire du 28 novembre 2012",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid, None);

        let t = link_citation(
            &s,
            "la circulaire du 28 novembre 2012 portant examen",
            "Circulaire du 28 novembre 2012 portant examen",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid, None);
    }

    #[test]
    fn rectificatif_never_wins_a_leg() {
        let s = LinkSnapshot::build(
            vec![
                text(
                    "JORF_LO_REAL",
                    "Loi n° 99-209 organique du 19 mars 1999 relative à la Nouvelle-Calédonie (1)",
                    "Loi n° 99-209 organique du 19 mars 1999 relative à la nouvelle-Calédonie (1)",
                    "LOI",
                    Some("FR"),
                    400,
                ),
                text(
                    "JORF_LO_RECTIF",
                    "LOI organique no  99-209 du 19 mars 1999  relative à la Nouvelle-Calédonie (rectificatif)",
                    "LOI organique du 19 mars 1999",
                    "LOI_ORGANIQUE",
                    Some("FR"),
                    0,
                ),
            ],
            vec![],
        );
        let t = link_citation(
            &s,
            "la loi organique du 19 mars 1999",
            "LOI organique du 19 mars 1999",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORF_LO_REAL"));
    }

    #[test]
    fn foreign_title_links_by_gentile_and_family() {
        let s = snap();
        let t = link_citation(&s, "le code civil suisse", "Code civil suisse", None, None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("fedlex/cc"));
        let t = link_citation(&s, "le code civil belge", "Code civil belge", None, None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("belgian/cc"));
    }

    #[test]
    fn bare_code_family_follows_the_owner_country() {
        let s = snap();
        let t = link_citation(&s, "le code civil", "Code civil", None, None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("LEGITEXT_CC"));
        let t = link_citation(&s, "le code civil", "Code civil", None, Some("CH"));
        assert_eq!(t.ref_text_uid.as_deref(), Some("fedlex/cc"));
        let t = link_citation(&s, "le code civil", "Code civil", None, Some("BE"));
        assert_eq!(t.ref_text_uid.as_deref(), Some("belgian/cc"));
        let t = link_citation(
            &s,
            "le code civil belge",
            "Code civil belge",
            None,
            Some("CH"),
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("belgian/cc"));
    }

    #[test]
    fn bare_constitution_follows_the_owner_country() {
        let s = snap();
        let t = link_citation(&s, "la Constitution", "Constitution", None, None);
        assert_eq!(t.ref_text_uid, None);
        let t = link_citation(&s, "la Constitution", "Constitution", None, Some("UA"));
        assert_eq!(t.ref_text_uid.as_deref(), Some("ukr/const"));
    }

    #[test]
    fn bare_family_head_follows_the_owner_country() {
        let s = LinkSnapshot::build(
            vec![
                text(
                    "JORF_CONST58",
                    "Constitution du 4 octobre 1958",
                    "Constitution du 4 octobre 1958",
                    "CONSTITUTION",
                    Some("FR"),
                    108,
                ),
                text(
                    "CH/CONSTITUTION-FEDERALE",
                    "Constitution fédérale de la Confédération suisse (RS 101)",
                    "Constitution fédérale",
                    "CONSTITUTION",
                    Some("CH"),
                    0,
                ),
                text("LEGITEXT_CPC", "Code de procédure civile", "Code de procédure civile", "CODE", Some("FR"), 1500),
                text(
                    "CG/CPCCAF",
                    "Code de procédure civile, commerciale, administrative et financière (Congo)",
                    "code de procédure civile commerciale administrative et financière congolais",
                    "code_procedure_civile",
                    Some("CG"),
                    0,
                ),
                text(
                    "BJ/CPCCSAC",
                    "Code de procédure civile, commerciale, sociale, administrative et des comptes (Bénin)",
                    "code de procédure civile commerciale sociale administrative et des comptes béninois",
                    "code_procedure_civile",
                    Some("BJ"),
                    0,
                ),
                text("LEGITEXT_CC", "Code civil", "Code civil", "CODE", Some("FR"), 2800),
                text(
                    "GA/CC-P1",
                    "Code civil gabonais, loi n° 15/72 du 29 juillet 1972 (première partie)",
                    "Code civil gabonais",
                    "code_civil",
                    Some("GA"),
                    0,
                ),
                text(
                    "GA/CC-P2",
                    "Code civil (Gabon), deuxième partie : successions et libéralités",
                    "Code civil",
                    "code_civil",
                    Some("GA"),
                    0,
                ),
            ],
            vec![
                ("GA/CC-P1".to_string(), "42".to_string()),
                ("GA/CC-P2".to_string(), "700".to_string()),
                ("LEGITEXT_CC".to_string(), "42".to_string()),
                ("LEGITEXT_CC".to_string(), "700".to_string()),
            ],
        );
        let t = link_citation(&s, "la Constitution", "Constitution", None, Some("CH"));
        assert_eq!(t.ref_text_uid.as_deref(), Some("CH/CONSTITUTION-FEDERALE"));
        let t = link_citation(
            &s,
            "le code de procédure civile",
            "Code de procédure civile",
            None,
            Some("CG"),
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("CG/CPCCAF"));
        let t = link_citation(
            &s,
            "le code de procédure civile béninois",
            "Code de procédure civile béninois",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("BJ/CPCCSAC"));
        let t = link_citation(&s, "le code civil", "Code civil", Some("42"), Some("GA"));
        assert_eq!(t.ref_text_uid.as_deref(), Some("GA/CC-P1"));
        let t = link_citation(&s, "le code civil", "Code civil", Some("700"), Some("GA"));
        assert_eq!(t.ref_text_uid.as_deref(), Some("GA/CC-P2"));
        let t = link_citation(&s, "le code civil", "Code civil", None, Some("GA"));
        assert_eq!(t.ref_text_uid, None);
    }

    #[test]
    fn dated_act_identity_never_contradicts_itself() {
        let s = LinkSnapshot::build(
            vec![
                text(
                    "JORF_69_5",
                    "Loi n° 69-5 du 3 janvier 1969 relative à la création des communes",
                    "Loi du 3 janvier 1969",
                    "LOI",
                    Some("FR"),
                    9,
                ),
                text(
                    "JORF_2010_463",
                    "LOI n° 2010-463 du 7 mai 2010 de finances rectificative pour 2010 (1)",
                    "LOI du 7 mai 2010",
                    "LOI",
                    Some("FR"),
                    3,
                ),
            ],
            vec![],
        );
        let t = link_citation(
            &s,
            "la loi n° 69-5 du 18 février 1969",
            "Loi du 18 février 1969",
            None,
            Some("NE"),
        );
        assert_eq!(t.ref_text_uid, None);
        let t = link_citation(
            &s,
            "la loi n° 69-5 du 3 janvier 1969",
            "Loi du 3 janvier 1969",
            None,
            Some("NE"),
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORF_69_5"));
        let t = link_citation(
            &s,
            "la loi n° 5982 du 7 mai 2010",
            "Loi du 7 mai 2010",
            None,
            Some("TR"),
        );
        assert_eq!(t.ref_text_uid, None);
        let t = link_citation(
            &s,
            "la loi du 7 mai 2010",
            "Loi du 7 mai 2010",
            None,
            Some("TR"),
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORF_2010_463"));
        let t = link_citation(
            &s,
            "la loi n° 69-5 du 18 février 1969",
            "Loi du 18 février 1969",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORF_69_5"));
        let t = link_citation(
            &s,
            "la loi du pays n° 2010-463 du 7 mai 2010",
            "Loi du pays du 7 mai 2010",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid, None);
    }

    #[test]
    fn bare_code_family_absent_from_the_owner_country_abstains() {
        let s = LinkSnapshot::build(
            vec![
                text(
                    "LEGITEXT_CP",
                    "Code pénal",
                    "Code pénal",
                    "CODE",
                    Some("FR"),
                    900,
                ),
                text(
                    "GN/CODE-PROCEDURE-PENALE-2016",
                    "Code de procédure pénale (Guinée)",
                    "code de procédure pénale",
                    "code_procedure_penale",
                    Some("GN"),
                    0,
                ),
            ],
            vec![("LEGITEXT_CP".to_string(), "64".to_string())],
        );
        let t = link_citation(&s, "le Code pénal", "Code pénal", Some("64"), Some("GN"));
        assert_eq!(t.ref_text_uid, None);
        let t = link_citation(&s, "le code pénal", "Code pénal", Some("64"), None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("LEGITEXT_CP"));
        let t = link_citation(
            &s,
            "le code de procédure pénale",
            "Code de procédure pénale",
            None,
            Some("GN"),
        );
        assert_eq!(
            t.ref_text_uid.as_deref(),
            Some("GN/CODE-PROCEDURE-PENALE-2016")
        );
    }

    #[test]
    fn dated_act_number_beats_collapsed_title() {
        let s = snap();
        let t = link_citation(
            &s,
            "le décret n° 73-609 du 5 juillet 1973",
            "Décret du 5 juillet 1973",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_D609"));
        let t = link_citation(
            &s,
            "le décret du 5 juillet 1973",
            "Décret du 5 juillet 1973",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid, None);
    }

    #[test]
    fn dated_act_by_date_links_short_form() {
        let s = snap();
        let t = link_citation(
            &s,
            "la loi du 13 juillet 1983",
            "Loi du 13 juillet 1983",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_FONC"));
        let t = link_citation(
            &s,
            "la loi du 10 juillet 1991",
            "Loi du 10 juillet 1991",
            Some("37"),
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_AJ"));
        assert_eq!(t.ref_num_key.as_deref(), Some("37"));
    }

    #[test]
    fn eu_secondary_law_by_slashnum() {
        let s = snap();
        let t = link_citation(
            &s,
            "le règlement (UE) n° 604/2013",
            "Règlement (UE) n° 604/2013",
            Some("17"),
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("CELEX_DUBLIN"));
        assert_eq!(t.ref_num_key.as_deref(), Some("17"));
        let t = link_citation(
            &s,
            "le règlement Dublin III n° 604/2013",
            "Règlement n° 604/2013",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("CELEX_DUBLIN"));
    }

    #[test]
    fn bilateral_accord_by_gentile_and_date() {
        let s = snap();
        let t = link_citation(
            &s,
            "l'accord franco-algérien du 27 décembre 1968 modifié",
            "Accord franco-algérien du 27 décembre 1968 modifié",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_ACCDZ"));
        let t = link_citation(
            &s,
            "l'accord entre la France et l'Algérie relatif à la circulation des ressortissants algériens du 27 décembre 1968",
            "Accord entre la France et l'Algérie relatif à la circulation des ressortissants algériens du 27 décembre 1968",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_ACCDZ"));
    }

    #[test]
    fn treaty_links_by_place_and_date() {
        let s = snap();
        let t = link_citation(
            &s,
            "la convention de La Haye du 25 octobre 1980 sur les aspects civils de l'enlèvement international d'enfants",
            "Convention de La Haye du 25 octobre 1980 sur les aspects civils de l'enlèvement international d'enfants",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_HAYE80"));
        let t = link_citation(
            &s,
            "la convention de Bruxelles du 25 août 1924",
            "Convention de Bruxelles du 25 août 1924",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_BRUX24"));
        let t = link_citation(
            &s,
            "la convention de Rome du 19 juin 1980 sur la loi applicable aux obligations contractuelles",
            "Convention de Rome du 19 juin 1980 sur la loi applicable aux obligations contractuelles",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_ROME80"));
        let t = link_citation(
            &s,
            "la déclaration universelle des droits de l'homme du 10 décembre 1948",
            "Déclaration universelle des droits de l'homme du 10 décembre 1948",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_DUDH"));
    }

    #[test]
    fn collapsed_dated_title_key_follows_living_text_unless_numbered() {
        let s = snap();
        let t = link_citation(
            &s,
            "l'ordonnance du 2 novembre 1945",
            "Ordonnance du 2 novembre 1945",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_ONOTAIRE"));
        let t = link_citation(
            &s,
            "l'ordonnance n° 45-2658 du 2 novembre 1945",
            "Ordonnance du 2 novembre 1945",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_OETRANGERS"));
    }

    #[test]
    fn citability_gate_never_links() {
        let s = snap();
        let t = link_citation(
            &s,
            "l'arrêté préfectoral du 3 mai 2019",
            "Arrêté préfectoral du 3 mai 2019",
            None,
            None,
        );
        assert_eq!(t, LinkTarget::default());
        let t = link_citation(
            &s,
            "le règlement de copropriété",
            "Règlement de copropriété",
            None,
            None,
        );
        assert_eq!(t, LinkTarget::default());
    }

    #[test]
    fn nor_unique_links_duplicate_dead() {
        let mut a = text(
            "cir_1",
            "Titre libre A",
            "Titre libre A",
            "CIRCULAIRE",
            Some("FR"),
            0,
        );
        a.nor = Some("INTV1234567C".to_string());
        let mut b = text(
            "cir_2",
            "Titre libre B",
            "Titre libre B",
            "CIRCULAIRE",
            Some("FR"),
            0,
        );
        b.nor = Some("PRMX7654321J".to_string());
        let mut c = text(
            "cir_3",
            "Titre libre C",
            "Titre libre C",
            "CIRCULAIRE",
            Some("FR"),
            0,
        );
        c.nor = Some("PRMX7654321J".to_string());
        let s = LinkSnapshot::build(vec![a, b, c], vec![]);
        let t = link_citation(
            &s,
            "l'arrêté préfectoral NOR INTV1234567C du 3 mai 2019",
            "Arrêté préfectoral du 3 mai 2019",
            None,
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("cir_1"));
        let t = link_citation(
            &s,
            "la circulaire NOR PRMX7654321J du 4 juin 2020",
            "Circulaire du 4 juin 2020",
            None,
            None,
        );
        assert_eq!(t, LinkTarget::default());
    }

    #[test]
    fn embedded_aliases_resolve_famous_instruments() {
        for line in LINK_ALIASES_TSV.lines().take(50) {
            let cols: Vec<&str> = line.split('\t').collect();
            assert!(
                cols.len() == 3 || cols.len() == 4,
                "ligne TSV malformée : {line}"
            );
            assert_eq!(
                cols[0],
                cols[0].to_lowercase(),
                "clé non pliée : {}",
                cols[0]
            );
            assert!(!cols[2].is_empty(), "alias sans cible : {line}");
        }
    }
}
