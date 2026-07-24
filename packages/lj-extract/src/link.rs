//! Linker in-pass (ADR 0145) : résout chaque occurrence de citation vers le
//! catalogue (`ref_text_uid`, `ref_num_key`) **pendant** la passe d'extraction.
//! PUR (règle #1) : opère sur un [`LinkSnapshot`] bâti de lignes catalogue
//! plates hydratées par l'appelant (`lj-ingest` depuis `lj-store`).
//!
//! Règles, dans l'ordre d'autorité :
//!
//! 1. **Alias embarqués** (`data/link_aliases.tsv`) — la connaissance curée de
//!    l'ancienne table d'overrides, devenue du code : une correction de masse
//!    est un commit sur ce fichier. Validés contre le catalogue à l'hydratation.
//!    1bis. **NOR porté par la forme brute** (« circulaire NOR JUSK1140023C
//!    du 14 avril 2011 ») : identifiant interministériel unique en colonne
//!    `legal_text.nor` — même autorité qu'un alias, avant le gate.
//! 2. **Gate de citabilité** (`key_signals`) : acte local, norme privée,
//!    fragment → jamais lié.
//! 3. **Acte daté par numéro** (Voie B, ADR 0137) : le numéro survit dans la
//!    forme brute capturée et dans le `title` catalogue ; unique par
//!    (nature, numéro) seulement. Prime sur le titre (le numéro tranche les
//!    frères collapsés sur la même date).
//! 4. **Titre exact** : `lower(text_key) == lower(title_key)`, désambiguïsé
//!    « texte vivant » (max articles VIGUEUR, ex æquo → abstention, ADR 0102).
//! 5. **Droit dérivé UE** par (directive|règlement, année/séquence) unique.
//! 6. **Accord bilatéral** par (gentilé, date), piloté depuis le catalogue.
//! 7. **CCN** : gazetteer par squelette de tokens (ADR 0123), cible validée
//!    présente au catalogue.
//! 8. **Code étranger** par (ISO du gentilé, famille sans gentilé) unique —
//!    « Code civil suisse » → l'entrée CH dont le `title_key` est « Code
//!    civil » nu.
//! 9. **Acte daté par date** (nature, date) unique — la forme courte « loi du
//!    13 juillet 1983 » sans numéro ni titre exact.
//!
//! `ref_num_key` : posé par **existence** de l'article au catalogue (ou via la
//! table préfixe-agnostique des codes territoriaux, migration 0087 §7.4) ;
//! jamais inventé.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use jiff::civil::Date;
use regex::Regex;

use crate::data::LINK_ALIASES_TSV;
use crate::extract::common::FOREIGN_NATIONALITY_STEMS;
use crate::extract::key_signals::{key_signals, Citability, KeyNature, KeySignals};
use crate::gazetteer::gazetteer;
use lj_core::text::fold;

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
    /// Codes territoriaux à préfixe d'article variable (migration 0087 §7.4).
    pub num_prefix_agnostic: bool,
    /// Nombre d'articles VIGUEUR (désambiguïsation « texte vivant », ADR 0102).
    pub n_vigueur: i64,
    /// Date de signature (`legal_text.date_texte`) — l'identité datée des
    /// familles à titre libre (circulaires : le titre du fond ne porte
    /// presque jamais « Circulaire du <date> », la date vit en colonne).
    pub date_texte: Option<Date>,
    /// NOR (`legal_text.nor`) — identifiant interministériel unique, la clé
    /// de résolution la plus forte quand la décision le cite (visas).
    pub nor: Option<String>,
}

impl CatalogText {
    /// Depuis la ligne plate de `lj-store::link_catalog_texts` (même ordre de
    /// colonnes) — la conversion vit ici pour que chaque hydrateur (`lj-ingest`,
    /// `lj-bench`) ne la réécrive pas. `date_texte` en ISO.
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

/// Cible d'une occurrence, prête pour `legal_citation`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkTarget {
    pub ref_text_uid: Option<String>,
    pub ref_num_key: Option<String>,
}

/// Juridiction ÉMETTRICE de la décision extraite — le contexte qui résout les
/// instruments nus qu'une cour cite sans les nommer : « du règlement de
/// procédure » dans un arrêt CJUE désigne le règlement de la formation qui
/// parle, « de la Convention » dans un arrêt CEDH désigne la CESDH. Dérivé de
/// `jurisdiction_type` + ECLI (`EU:C`/`EU:T`/`EU:F`), passé à `doc_extract`.
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
/// uid. « statut » est exclu — dans les affaires de fonction publique UE
/// (EU:F, pourvois EU:T/EU:C), « du statut » désigne le statut des
/// fonctionnaires, pas celui de la Cour. « du règlement » nu CJUE aussi :
/// c'est le règlement matériel sous interprétation. Validées au catalogue à
/// l'hydratation, comme les alias.
const FORUM_DEFAULTS: &[(Forum, &str, &str)] = &[
    (Forum::Cedh, "convention", "JORFTEXT000000886019"),
    (Forum::CjueCour, "reglement de procedure", "EU/RPROC/CJUE"),
    (
        Forum::CjueTribunal,
        "reglement de procedure",
        "EU/RPROC/TRIBUNAL",
    ),
    (Forum::CjueTfp, "reglement de procedure", "EU/RPROC/TFP"),
];

/// Index mémoire du catalogue pour la passe. ~155 k textes + ~2 M articles :
/// se rebâtit au début de chaque run d'ingest / passe intégrale.
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
    /// avec leur titre plié : « règlement du 17 décembre 2013 établissant les
    /// règles relatives aux paiements directs » se départage par tokens dans
    /// les six règlements PAC du même jour, comme les traités (6bis).
    eu_date: HashMap<(bool, Date), Vec<(String, String)>>,
    /// Cibles des instruments nus par (forum, nature) — [`FORUM_DEFAULTS`]
    /// filtré aux uids présents au catalogue.
    forum_defaults: HashMap<(Forum, &'static str), String>,
    /// (nature pliée, date de l'acte) → uid, uniques seulement.
    nature_date: HashMap<(String, Date), String>,
    /// NOR (majuscules) → uid, uniques seulement — identifiant
    /// interministériel, même autorité qu'un alias quand la mention le porte.
    nor: HashMap<String, String>,
    /// Traités par date — TOUTES les dates du titre de l'acte de publication
    /// (« faite à La Haye le 25 octobre 1980 », « signée le 9 septembre
    /// 1966 »…) indexent le texte. TOUS les candidats d'une date (une
    /// conférence signe plusieurs conventions le même jour ; les décrets
    /// d'adhésion réembarquent le titre de base) : `(uid, titre plié)` —
    /// les tokens cités puis les mots-instruments départagent.
    treaty_date: HashMap<Date, Vec<(String, String)>>,
    /// Accords bilatéraux : (stem gentilé plié, date) → uid, uniques seulement.
    accords: HashMap<(String, Date), String>,
    /// (ISO-2, famille de code sans gentilé) → uid, uniques seulement.
    foreign_code: HashMap<(String, String), String>,
    /// Cibles CCN présentes (valide le snap gazetteer).
    kalicont: HashSet<String>,
    /// uid → `num_key` des articles au catalogue (existence).
    articles: HashMap<String, HashSet<String>>,
    /// uid → forme citée pointée → `num_key` officiel étoilé (« R. 771-5 » →
    /// « R*771-5 », décrets en Conseil d'État).
    star_articles: HashMap<String, HashMap<String, String>>,
    /// uid préfixe-agnostique → cœur numérique → `num_key` officiel.
    prefix_agnostic: HashMap<String, HashMap<String, String>>,
}

impl LinkSnapshot {
    /// Bâtit le snapshot depuis les lignes catalogue. `articles` = paires
    /// `(text_uid, num_key)` distinctes. Pur et déterministe.
    pub fn build(
        texts: Vec<CatalogText>,
        articles: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let mut article_sets: HashMap<String, HashSet<String>> = HashMap::new();
        for (uid, num_key) in articles {
            article_sets.entry(uid).or_default().insert(num_key);
        }

        // Articles « étoilés » (décrets en Conseil d'État : « r*771-5 »,
        // variantes « *r », « r** », « d* », « l* ») : les décisions les
        // citent en forme pointée (« R. 771-5 »). Index forme citée (repliée
        // en clé publique) → num_key officiel étoilé.
        let mut star_articles: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (uid, nums) in &article_sets {
            for nk in nums {
                if !nk.contains('*') {
                    continue;
                }
                let cited = lj_core::article_key::article_key(&nk.replace('*', ""));
                if !cited.is_empty() && !nums.contains(&cited) {
                    star_articles
                        .entry(uid.clone())
                        .or_default()
                        .insert(cited, nk.clone());
                }
            }
        }

        // Texte vivant par title_key (ADR 0102) : max VIGUEUR, ex æquo → skip.
        // Clés pliées (`fold`) : le vocabulaire cité et le catalogue divergent
        // sur les apostrophes typographiques — `lower()` seul les ratait.
        let mut best: HashMap<String, (usize, i64, bool)> = HashMap::new();
        for (i, t) in texts.iter().enumerate() {
            let key = fold_link(&t.title_key);
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
                .entry(fold_link(&t.title_key))
                .or_default()
                .push(t.text_uid.clone());
        }
        title_brothers.retain(|_, v| v.len() >= 2);

        // Index structurés, uniques seulement : un doublon retire la clé
        // (abstention plutôt que lien hasardeux, #12).
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

        let mut nature_num = HashMap::new();
        let mut nature_num_dead = HashSet::new();
        let mut eu_num = HashMap::new();
        let mut eu_num_dead = HashSet::new();
        let mut eu_date: HashMap<(bool, Date), Vec<(String, String)>> = HashMap::new();
        let mut nature_date = HashMap::new();
        let mut nature_date_dead = HashSet::new();
        let mut nor_index = HashMap::new();
        let mut nor_dead = HashSet::new();
        let mut treaty_date: HashMap<Date, Vec<(String, String)>> = HashMap::new();
        // Candidats accords par (stem gentilé, date) : les avenants et lois
        // d'autorisation réembarquent la date de la convention de base dans
        // leur titre — l'unicité brute tuerait la clé, l'acte de base
        // départage (voir `accord_is_base`).
        let mut accord_cands: HashMap<(String, Date), Vec<(String, bool)>> = HashMap::new();
        let mut foreign_code = HashMap::new();
        let mut foreign_code_dead = HashSet::new();
        let mut kalicont = HashSet::new();
        let mut prefix_agnostic: HashMap<String, HashMap<String, String>> = HashMap::new();

        for t in &texts {
            let folded_title = fold_link(&t.title);
            let folded_tk = fold_link(&t.title_key);

            // NOR en colonne — le fond CIRCULAIRES le porte à ~92 % ; un
            // doublon (rééditions) tue la clé, comme partout.
            if let Some(nor) = &t.nor {
                insert_unique(
                    &mut nor_index,
                    &mut nor_dead,
                    nor.to_uppercase(),
                    &t.text_uid,
                );
            }

            // Circulaires (ADR 0196) : identité datée en colonne — les titres
            // du fond sont libres (« Modalités d'attribution… ») et ne
            // portent presque jamais « Circulaire du <date> ».
            if t.nature == "CIRCULAIRE" {
                if let Some(date) = t.date_texte {
                    insert_unique(
                        &mut nature_date,
                        &mut nature_date_dead,
                        ("circulaire".to_string(), date),
                        &t.text_uid,
                    );
                }
            }

            // Actes datés FR : nature + numéro depuis le title brut (le
            // title_key rabote le numéro), nature + date depuis le title_key.
            if let Some(nat) = head_act_nature(&folded_title) {
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

            // Droit dérivé UE : (directive?, année/séquence) depuis le title,
            // et TOUTES les dates du title pour la forme citée sans numéro
            // (« règlement du 17 décembre 2013 établissant… »).
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
            // « … entre la France et <pays> … » (conventions fiscales, accords
            // de coopération) : le pays + chaque date du titre — la
            // jurisprudence cite « convention franco-suisse du 9 septembre
            // 1966 » là où le décret de publication écrit « convention entre
            // la France et la Suisse …et du protocole additionnel du
            // 9 septembre 1966 ». Le stem (5 chars) fait converger pays et
            // gentilé (« portugal » ↔ « portugais », « italie » ↔
            // « italienne »).
            if matches!(t.nature.as_str(), "TRAITE" | "TI") {
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

            // Traités : chaque date du titre de publication indexe le texte
            // (« faite à La Haye le 25 octobre 1980 », « signée le 9 septembre
            // 1966 », « LE 25-08-1924 » — le title_key raboté ne garde que le
            // décret, la matière conventionnelle vit dans la queue du titre).
            if matches!(t.nature.as_str(), "TRAITE" | "TI") {
                for date in all_dates(&folded_title) {
                    let cands = treaty_date.entry(date).or_default();
                    if !cands.iter().any(|(uid, _)| uid == &t.text_uid) {
                        cands.push((t.text_uid.clone(), folded_title.clone()));
                    }
                }
            }

            // Codes étrangers : (ISO, famille sans gentilé) — le title_key
            // porte parfois le gentilé (« code civil belge »), parfois non
            // (« Code civil » suisse) ; la famille nue les réunit.
            if let Some(iso) = t.jurisdiction.as_deref() {
                if !matches!(iso, "FR" | "UE" | "INTL") && folded_tk.starts_with("code") {
                    let base = strip_gentile_words(&folded_tk);
                    insert_unique(
                        &mut foreign_code,
                        &mut foreign_code_dead,
                        (iso.to_string(), base),
                        &t.text_uid,
                    );
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

        // Résolution des accords : clé unique → l'uid ; collision → l'acte
        // de base s'il est seul, sinon clé morte (jamais un pari).
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
        // BOFiP (ADR 0196) : le code BOI cité (« BOI-IR-BASE-10-10 ») EST le
        // `text_uid` catalogue — alias direct, même autorité que le TSV.
        for t in &texts {
            if t.nature == "BOFIP" {
                alias_text.insert(fold_link(&t.text_uid), t.text_uid.clone());
            }
        }
        let mut alias_article = HashMap::new();
        for line in LINK_ALIASES_TSV.lines() {
            // 4e colonne (ref_num_key forcé) optionnelle : les text-fixers git
            // strippent les tabs finaux, l'absence vaut vide.
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
            // Clés pliées (`fold`) : le vocabulaire prod mélange apostrophes
            // droites/typographiques et casses — le lookup plie des deux côtés.
            if ak.is_empty() {
                alias_text.insert(fold_link(tk), uid.to_string());
            } else {
                // La 4e colonne est curée en forme citée ; la clé stockée est
                // la clé publique (ADR 0209).
                let forced = (!num.is_empty()).then(|| lj_core::article_key::article_key(num));
                alias_article.insert((fold_link(tk), ak.to_string()), (uid.to_string(), forced));
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
            nor: nor_index,
            treaty_date,
            accords,
            foreign_code,
            kalicont,
            articles: article_sets,
            star_articles,
            prefix_agnostic,
        }
    }

    /// Cardinalités des index — pour le log d'hydratation (diagnostic d'un
    /// snapshot appauvri sans fouiller les champs privés).
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
            self.foreign_code.len(),
            self.articles.len(),
        )
    }

    /// L'article existe-t-il au catalogue pour ce texte ? (validation des
    /// rattachements du moteur compilé — antécédents, anaphores.) Même
    /// frontière d'alphabet que [`Self::num_key_for`] : clé citée → clé
    /// publique avant consultation du catalogue.
    pub fn has_article(&self, uid: &str, num_key: &str) -> bool {
        let ak = &lj_core::article_key::article_key(num_key);
        self.articles.get(uid).is_some_and(|s| s.contains(ak))
            || self
                .star_articles
                .get(uid)
                .is_some_and(|m| m.contains_key(ak))
            || !self.prefixed_variants(uid, ak).is_empty()
    }

    /// Article cité sans son préfixe de partie (« 1233-62 » pour
    /// « L. 1233-62 ») : les variantes préfixées présentes au catalogue.
    /// Le `num_key` ne se pose que si la variante est UNIQUE (L. 521-1 et
    /// R. 521-1 coexistent dans le CJA : lien texte, jamais un pari).
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

    /// Le catalogue connaît-il la STRUCTURE de ce texte ? Faux pour les
    /// traités JORF sans structure — l'existence d'un article n'y est alors
    /// pas réfutable. Les décrets JORF de publication de traités portent 1–2
    /// articles boilerplate (« sera publiée au Journal officiel ») qui ne
    /// disent rien des articles du traité lui-même : ≤ 2 = structure inconnue.
    pub fn has_article_info(&self, uid: &str) -> bool {
        self.articles.get(uid).is_some_and(|s| s.len() > 2)
    }

    /// Itère `(uid, num_keys)` — pour bâtir des index dérivés (index inversé
    /// `num_key` → textes du moteur compilé).
    pub fn article_sets(&self) -> impl Iterator<Item = (&str, &HashSet<String>)> {
        self.articles.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Cible de l'instrument NU d'un forum (« du règlement de procédure »
    /// dans un arrêt CJUE) — présente seulement si le uid est au catalogue.
    pub fn forum_default<'a>(&'a self, forum: Forum, nature: &str) -> Option<&'a str> {
        self.forum_defaults
            .iter()
            .find(|((f, n), _)| *f == forum && *n == nature)
            .map(|(_, uid)| uid.as_str())
    }

    /// `ref_num_key` par existence au catalogue, sinon via la table
    /// préfixe-agnostique du texte. Jamais inventé. Frontière d'alphabet : la
    /// clé citée arrive en forme `normalize_article` (« L. 761-1 »), le
    /// catalogue est en clé publique slug (ADR 0209) — conversion ici.
    pub fn num_key_for(&self, uid: &str, article_key: Option<&str>) -> Option<String> {
        let ak = &lj_core::article_key::article_key(article_key?);
        if ak.is_empty() {
            return None;
        }
        if self.articles.get(uid).is_some_and(|s| s.contains(ak)) {
            return Some(ak.to_string());
        }
        if let Some(official) = self.star_articles.get(uid).and_then(|m| m.get(ak)) {
            return Some(official.clone());
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
/// la résolution (cascade regex), qui ne dépend QUE de `text_key`. À mémoïser
/// par l'appelant quand la même clé résout plusieurs articles (compose du
/// moteur citations : une clé revient avec des dizaines d'articles distincts).
pub struct KeyAnalysis {
    lower: String,
    folded: String,
    sig: KeySignals,
}

impl KeyAnalysis {
    pub fn new(text_key: &str) -> Self {
        Self {
            lower: text_key.to_lowercase(),
            folded: fold_link(text_key),
            sig: key_signals(text_key),
        }
    }

    pub fn signals(&self) -> &KeySignals {
        &self.sig
    }
}

/// Résout une occurrence : `instrument` = forme brute capturée (porte le
/// numéro d'acte que `text_key` rabote), `text_key`/`article_key` = clés
/// canoniques. Déterministe, total.
pub fn link_citation(
    snap: &LinkSnapshot,
    instrument: &str,
    text_key: &str,
    article_key: Option<&str>,
) -> LinkTarget {
    link_citation_analyzed(snap, instrument, &KeyAnalysis::new(text_key), article_key)
}

/// Variante à analyse fournie (voir [`KeyAnalysis`]).
pub fn link_citation_analyzed(
    snap: &LinkSnapshot,
    instrument: &str,
    analysis: &KeyAnalysis,
    article_key: Option<&str>,
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
    if let Some(uid) = snap.alias_text.get(folded) {
        return LinkTarget {
            ref_text_uid: Some(uid.clone()),
            ref_num_key: snap.num_key_for(uid, article_key),
        };
    }

    // 1bis. NOR porté par la forme brute (« circulaire NOR JUSK1140023C du
    // 14 avril 2011 », « décret NOR : DEVT0766271D du 26 octobre 2007 ») :
    // identifiant interministériel unique — même autorité qu'un alias, avant
    // le gate (une mention qui porte un NOR est un acte réel, pas de la
    // prose).
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
    );
    match uid {
        Some(uid) => {
            let num = snap.num_key_for(&uid, article_key);
            LinkTarget {
                ref_text_uid: Some(uid),
                ref_num_key: num,
            }
        }
        None => LinkTarget::default(),
    }
}

/// Cœur de la résolution texte, règles 3-9 (cf. doc de module).
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
) -> Option<String> {
    // 3. Acte daté par numéro — prime sur le titre (les frères d'une même date
    // collapsent sur le même title_key, le numéro tranche).
    let dated_shape = RE_DATED_ACT_SHAPE.is_match(folded);
    let head_num = dated_shape
        .then(|| head_act_num(&fold_link(instrument)))
        .flatten();
    if let (Some(nat), Some(num)) = (head_act_nature(folded), head_num.as_deref()) {
        if let Some(uid) = snap.nature_num.get(&(nat.to_string(), num.to_string())) {
            return Some(uid.clone());
        }
    }

    // 4. Titre exact, texte vivant (ADR 0102) — y compris pour les clés datées
    // dont le rabotage collapse des frères d'un même jour : le texte le plus
    // vivant est presque toujours celui que la jurisprudence vise (« loi du
    // 10 juillet 1991 » = l'aide juridique, pas les 8 autres du même JO) ; les
    // contre-exemples avérés se corrigent par alias embarqué. Le marqueur
    // éditorial « modifié(e) » de la forme citée (« arrêté du 18 juin 1991
    // modifié relatif à… ») n'est pas une identité : retenté sans lui.
    // L'article cité tranche entre frères d'un même title_key raboté
    // (« décret du 19 décembre 1991 » + « article 108 » = le 91-1266, pas le
    // plus vivant 91-1267) : si le vivant ignore l'article et qu'EXACTEMENT
    // un frère le porte, le frère l'emporte.
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
        return Some(brother_pick(uid));
    }
    if folded.contains(" modifie") {
        let sans_modifie = strip_word(folded, |w| {
            matches!(w, "modifie" | "modifiee" | "modifies" | "modifiees")
        });
        if let Some(uid) = snap.title_to_uid.get(&sans_modifie) {
            return Some(uid.clone());
        }
    }

    // 5. Droit dérivé UE.
    if let (KeyNature::DirectiveUe | KeyNature::ReglementUe, Some(num)) = (nature, act_num) {
        let key = (*nature == KeyNature::DirectiveUe, num.to_string());
        if let Some(uid) = snap.eu_num.get(&key) {
            return Some(uid.clone());
        }
    }

    // 5bis. Droit dérivé UE par date + tokens — « règlement du 17 décembre
    // 2013 établissant les règles relatives aux paiements directs » : six
    // règlements PAC portent cette date, la queue de titre citée départage
    // (même discipline que les traités en 6bis : UN survivant ou abstention).
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
    // 27 décembre 1968 », « convention fiscale franco-suisse du 9 septembre
    // 1966 », « convention conclue le 29 mars 1974 entre la France et le
    // Sénégal ».
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
    // sur les contrats de vente… » ↔ « …portant publication de la convention
    // …, faite à Vienne le 11 avril 1980 ». Une conférence signe plusieurs
    // conventions le même jour : les tokens distinctifs de la clé citée
    // (lieu, matière) départagent — lié seulement si UN candidat de la date
    // contient tous les tokens (paraphrase ⇒ abstention, jamais un pari).
    if *nature == KeyNature::TraiteAccord {
        if let Some(cands) = act_date.and_then(|d| snap.treaty_date.get(&d)) {
            // Tokens distinctifs : mois et nombres exclus (déjà encodés dans
            // le pivot date — un titre JORF historique écrit « 25-08-1924 »).
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
                // Tie-break : les actes accessoires réembarquent le titre de
                // base — décret publiant le « protocole additionnel à la
                // convention X », décret d'adhésion / d'échange de lettres,
                // loi « autorisant la ratification » de la convention X.
                // Éliminé si le titre porte un mot-instrument absent de la
                // clé citée ; le décret de publication de base survit. Les
                // parenthèses (« (ensemble un protocole…) » — annexes
                // embarquées, pas l'instrument publié) sont retirées avant
                // le test.
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

    // 7. CCN via gazetteer (squelette de tokens, ADR 0123).
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
            if let Some(uid) = snap.foreign_code.get(&(iso.to_string(), base)) {
                return Some(uid.clone());
            }
        }
    }

    // 9. Acte daté par date — forme courte sans numéro ; jamais quand la forme
    // brute porte un numéro (il aurait dû trancher en 3, un désaccord
    // numéro/date serait un mislink).
    if dated_shape && head_num.is_none() {
        if let (Some(nat), Some(date)) = (head_act_nature(folded), act_date) {
            if let Some(uid) = snap.nature_date.get(&(nat.to_string(), date)) {
                return Some(uid.clone());
            }
        }
    }

    None
}

/// NOR dans la forme brute capturée : « NOR JUSK1140023C », « NOR :
/// INTK9700174C », « NORINTK1207286C » (graphie collée). Majuscules exigées —
/// la graphie officielle, et le pli casse rendrait « nor » indistinguable
/// d'un fragment. Préfiltre `contains` : la regex ne court que sur les rares
/// mentions porteuses.
fn nor_in_raw(instrument: &str) -> Option<String> {
    if !instrument.contains("NOR") {
        return None;
    }
    RE_NOR_IN_RAW.captures(instrument).map(|c| c[1].to_string())
}

static RE_NOR_IN_RAW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bNOR\s*:?\s*([A-Z]{4}\d{7}[A-Z])\b").unwrap());

/// Nature d'acte daté en tête de chaîne pliée (mêmes familles que la Voie B ;
/// « circulaire » depuis l'ingest du fond DILA CIRCULAIRES, ADR 0196).
fn head_act_nature(folded: &str) -> Option<&'static str> {
    [
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

/// Numéro d'acte FR ancré en tête (port du regex SQL Voie B) : nature,
/// déterminants tolérés, borne `[^0-9]{0,40}` avant le « n° » — un numéro
/// imbriqué après une date interposée n'est pas capté.
fn head_act_num(folded: &str) -> Option<String> {
    RE_HEAD_ACT_NUM.captures(folded).map(|c| c[1].to_string())
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
/// (« LE 25-08-1924 » → « le 25-08-1924 »). Dédupliquées, ordre stable.
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
fn first_date(folded: &str) -> Option<Date> {
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

/// Retire les mots-gentilés d'une clé pliée (« code civil suisse » → « code
/// civil ») — la famille nue, comparable entre clé citée et title_key.
fn strip_gentile_words(folded: &str) -> String {
    folded
        .split_whitespace()
        .filter(|w| !is_gentile_word(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `fold` + retrait des caractères de format Unicode (LRM/RLM, ZWSP, BOM,
/// soft hyphen) : les titres JORF historiques en sont truffés (« signés à
/// Paris, le 14 janvier ‎‎1971‎ ») et ils cassent l'extraction de dates comme
/// l'égalité de clés.
fn fold_link(s: &str) -> String {
    let folded = fold(s);
    if folded.chars().any(is_format_char) {
        folded.chars().filter(|c| !is_format_char(*c)).collect()
    } else {
        folded
    }
}

fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{FEFF}'
    )
}

/// Cœur numérique d'un `num_key` (préfixe d'instrument retiré jusqu'au premier
/// chiffre) — port du `regexp_replace(…, '^[^0-9]*', '')` de la migration 0087.
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
static RE_SLASHNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{1,4}/\d{1,4})").unwrap());
/// Marqueurs d'instrument conventionnel — pour le tie-break traité (un titre
/// en portant un ABSENT de la clé citée publie un instrument accessoire :
/// protocole additionnel, adhésion d'un État tiers, échange de lettres, loi
/// d'autorisation de ratification…).
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
/// elle-même — ni un avenant/protocole modificatif, ni une loi
/// d'autorisation. Départage les collisions (stem, date) : « convention
/// franco-suisse du 9 septembre 1966 » cible le décret de publication de
/// 1967, pas les cinq avenants qui réembarquent la date.
fn accord_is_base(folded_title: &str) -> bool {
    !["avenant", "modifiant", "autorisant", "approbation"]
        .iter()
        .any(|w| folded_title.contains(w))
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
                    Some("UE"),
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
                // Traité à actes accessoires : le décret de publication de base
                // (parenthèse d'annexes) doit battre la loi d'autorisation de
                // ratification qui réembarque le même titre et la même date.
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
                // Frères d'un même jour dont les title_keys collapsent (rabotage
                // total) : le « vivant » ne doit PAS trancher une clé datée nue.
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
        // Homonyme « code civil » (FR vs CH) : le texte vivant (FR) gagne.
        let t = link_citation(&s, "le code civil", "Code civil", Some("1240"));
        assert_eq!(t.ref_text_uid.as_deref(), Some("LEGITEXT_CC"));
        assert_eq!(t.ref_num_key.as_deref(), Some("1240"));
        // Article hors catalogue : texte lié, num NULL (existence).
        let t = link_citation(&s, "le code civil", "Code civil", Some("9999"));
        assert_eq!(t.ref_text_uid.as_deref(), Some("LEGITEXT_CC"));
        assert_eq!(t.ref_num_key, None);
    }

    #[test]
    fn foreign_code_links_by_gentile_and_family() {
        let s = snap();
        let t = link_citation(&s, "le code civil suisse", "Code civil suisse", None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("fedlex/cc"));
        let t = link_citation(&s, "le code civil belge", "Code civil belge", None);
        assert_eq!(t.ref_text_uid.as_deref(), Some("belgian/cc"));
    }

    #[test]
    fn dated_act_number_beats_collapsed_title() {
        let s = snap();
        // Les deux décrets du 5 juillet 1973 collapsent sur des title_keys
        // distincts ici, mais la clé citée courte ne matche aucun : le numéro
        // de la forme brute tranche.
        let t = link_citation(
            &s,
            "le décret n° 73-609 du 5 juillet 1973",
            "Décret du 5 juillet 1973",
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_D609"));
        // Sans numéro : deux frères à la même date → abstention (unicité).
        let t = link_citation(
            &s,
            "le décret du 5 juillet 1973",
            "Décret du 5 juillet 1973",
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
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_FONC"));
        // Forme courte d'un acte unique par date + article existant.
        let t = link_citation(
            &s,
            "la loi du 10 juillet 1991",
            "Loi du 10 juillet 1991",
            Some("37"),
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
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("CELEX_DUBLIN"));
        assert_eq!(t.ref_num_key.as_deref(), Some("17"));
        // Variante sans le boilerplate : le slashnum suffit.
        let t = link_citation(
            &s,
            "le règlement Dublin III n° 604/2013",
            "Règlement n° 604/2013",
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
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_ACCDZ"));
        // Forme longue : le gentilé apparaît hors « franco- », la date matche.
        let t = link_citation(
            &s,
            "l'accord entre la France et l'Algérie relatif à la circulation des ressortissants algériens du 27 décembre 1968",
            "Accord entre la France et l'Algérie relatif à la circulation des ressortissants algériens du 27 décembre 1968",
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_ACCDZ"));
    }

    #[test]
    fn treaty_links_by_place_and_date() {
        let s = snap();
        // Titre catalogue = décret de publication ; la matière et le lieu de
        // signature vivent dans la queue (« faite à La Haye le 25 octobre 1980 »).
        let t = link_citation(
            &s,
            "la convention de La Haye du 25 octobre 1980 sur les aspects civils de l'enlèvement international d'enfants",
            "Convention de La Haye du 25 octobre 1980 sur les aspects civils de l'enlèvement international d'enfants",
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_HAYE80"));
        // Date numérique du titre JORF historique (« SIGNEE A BRUXELLES LE 25-08-1924 »).
        let t = link_citation(
            &s,
            "la convention de Bruxelles du 25 août 1924",
            "Convention de Bruxelles du 25 août 1924",
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_BRUX24"));
        // Actes accessoires : la loi « autorisant la ratification » réembarque
        // titre et date — éliminée par ses mots-instruments ; la parenthèse
        // d'annexes du décret de base ne l'élimine pas, lui.
        let t = link_citation(
            &s,
            "la convention de Rome du 19 juin 1980 sur la loi applicable aux obligations contractuelles",
            "Convention de Rome du 19 juin 1980 sur la loi applicable aux obligations contractuelles",
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_ROME80"));
    }

    #[test]
    fn collapsed_dated_title_key_follows_living_text_unless_numbered() {
        let s = snap();
        // Deux ordonnances du même jour rabotées sur le MÊME title_key : le
        // texte vivant (max VIGUEUR) gagne (ADR 0102) — un contre-exemple
        // avéré se corrige par alias embarqué, pas par abstention globale.
        let t = link_citation(
            &s,
            "l'ordonnance du 2 novembre 1945",
            "Ordonnance du 2 novembre 1945",
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("JORFTEXT_ONOTAIRE"));
        // Avec le numéro dans la forme brute, la règle 3 tranche.
        let t = link_citation(
            &s,
            "l'ordonnance n° 45-2658 du 2 novembre 1945",
            "Ordonnance du 2 novembre 1945",
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
        );
        assert_eq!(t, LinkTarget::default());
        let t = link_citation(
            &s,
            "le règlement de copropriété",
            "Règlement de copropriété",
            None,
        );
        assert_eq!(t, LinkTarget::default());
    }

    /// NOR : identifiant plus fort que le gate (1bis), mais un doublon au
    /// catalogue (rééditions partageant le NOR) tue la clé — abstention.
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
        // Unique → lié, y compris à travers le gate (l'acte préfectoral
        // non-citable par la forme reste identifié par son NOR).
        let t = link_citation(
            &s,
            "l'arrêté préfectoral NOR INTV1234567C du 3 mai 2019",
            "Arrêté préfectoral du 3 mai 2019",
            None,
        );
        assert_eq!(t.ref_text_uid.as_deref(), Some("cir_1"));
        // Doublon → clé morte, jamais un pari.
        let t = link_citation(
            &s,
            "la circulaire NOR PRMX7654321J du 4 juin 2020",
            "Circulaire du 4 juin 2020",
            None,
        );
        assert_eq!(t, LinkTarget::default());
    }

    #[test]
    fn embedded_aliases_resolve_famous_instruments() {
        // Le snapshot de test contient-il les cibles des alias ? Non — on
        // vérifie seulement que le TSV embarqué parse et que ses clés sont
        // bien pliées en minuscules (contrat du fichier).
        for line in LINK_ALIASES_TSV.lines().take(50) {
            let cols: Vec<&str> = line.split('\t').collect();
            // 3 ou 4 colonnes : la 4e (ref_num_key forcé) est optionnelle,
            // les text-fixers git strippent les tabs finaux.
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
