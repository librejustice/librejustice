//! Cache référentiel process-local : `facet_value` + `jurisdiction` chargés
//! depuis Postgres via `lj-store`, servis aux handlers pour résoudre labels de
//! facettes, tags des hits et arbre juridiction.
//!
//! Cache moka mono-entrée, TTL 1 h (aligné sur les caches search/rerank) : le
//! référentiel ne bouge qu'aux migrations/backfills ; un UPDATE de label est
//! visible au pire une heure plus tard, sans redéploiement.

use std::collections::HashMap;
use std::sync::Arc;

use lj_dtos::FacetTag;
use lj_store::repository::{DecisionRepository, FacetValueRow, JurisdictionRow};

use crate::error::{ApiError, Result};
use crate::state::AppState;

/// Une valeur de facette du référentiel (`facet_value`), indexée par uid.
#[derive(Debug, Clone)]
pub struct FacetValueEntry {
    pub facet: String,
    pub label: String,
    pub abbr: Option<String>,
    pub parent_uid: Option<String>,
    pub hint: Option<String>,
    /// Rang de lecture voulu au sein de la facette. Ne sert qu'aux facettes qui
    /// forment une échelle, où le compteur classerait le moins important devant.
    pub sort: i32,
}

/// Une unité juridictionnelle (`jurisdiction`), indexée par code.
/// `source_code` = code de la source (location Judilibre) — sert aux suggestions
/// MCP quand un client envoie un ancien code.
#[derive(Debug, Clone)]
pub struct JurisdictionEntry {
    pub jurisdiction_type: String,
    pub source_code: String,
    pub city: Option<String>,
    pub label: String,
}

/// Vue en mémoire des référentiels de facettes. Immutable après chargement,
/// partagée en `Arc` via le cache de l'[`AppState`].
#[derive(Debug, Default)]
pub struct Referential {
    values: HashMap<String, FacetValueEntry>,
    jurisdictions: HashMap<String, JurisdictionEntry>,
    /// Nodes of the seat axis the nightly snapshot counted, sorted HERE: the
    /// database orders under `en_US.utf8`, which ignores the dot and puts `CAA`
    /// before `CA.AGEN`, so a binary search over its order misses.
    seats: Vec<String>,
    /// Court label under every slug that designates its unit: code, source
    /// location, folded label. The three key spaces do not overlap, so the map
    /// stays flat. Only reader: [`crate::chronology`].
    courts: HashMap<String, String>,
    /// Court label under its seat path, derived by the same rule the extraction
    /// uses, so a node of the tree and an indexed path can never disagree.
    court_paths: HashMap<String, String>,
    /// Every case name the fonds carries. A content query offers many candidate
    /// keys and most name nothing, too many to ask Postgres on every search.
    case_names: std::collections::HashSet<String>,
    /// The keys of every named decision, and the decisions under each word of them.
    names: lj_core::case_names::NameIndex,
    name_ids: Vec<i64>,
    seat_words: std::collections::HashSet<String>,
}

impl Referential {
    /// Construit la vue depuis les lignes DB (ou des fixtures de test).
    pub fn new(
        values: Vec<FacetValueRow>,
        jurisdictions: Vec<JurisdictionRow>,
        mut seats: Vec<String>,
    ) -> Self {
        seats.sort_unstable();
        Self {
            case_names: Default::default(),
            names: Default::default(),
            name_ids: Default::default(),
            seat_words: jurisdictions
                .iter()
                .flat_map(|j| {
                    j.city
                        .iter()
                        .chain(std::iter::once(&j.label))
                        .flat_map(|s| folded_words(s))
                        .collect::<Vec<_>>()
                })
                .collect(),
            courts: jurisdictions.iter().flat_map(court_keys).collect(),
            court_paths: jurisdictions
                .iter()
                .map(|j| {
                    (
                        lj_core::chamber::jurisdiction_trunk(&j.jurisdiction_type, Some(&j.code))
                            .join("."),
                        j.label.clone(),
                    )
                })
                .collect(),
            values: values
                .into_iter()
                .map(|r| {
                    (
                        r.uid,
                        FacetValueEntry {
                            facet: r.facet,
                            label: r.label,
                            abbr: r.abbr,
                            parent_uid: r.parent_uid,
                            hint: r.hint,
                            sort: r.sort,
                        },
                    )
                })
                .collect(),
            jurisdictions: jurisdictions
                .into_iter()
                .map(|r| {
                    (
                        r.code,
                        JurisdictionEntry {
                            jurisdiction_type: r.jurisdiction_type,
                            source_code: r.source_code,
                            city: r.city,
                            label: r.label,
                        },
                    )
                })
                .collect(),
            seats,
        }
    }

    /// Libellé d'une cour sous son chemin de siège.
    pub fn court_label_of_path(&self, path: &str) -> Option<&str> {
        self.court_paths.get(path).map(String::as_str)
    }

    /// Le fonds porte-t-il ce nœud de siège ?
    pub fn has_seat(&self, path: &str) -> bool {
        self.seats
            .binary_search_by(|n| n.as_str().cmp(path))
            .is_ok()
    }

    pub fn seat_nodes(&self) -> &[String] {
        &self.seats
    }

    /// Entrée `facet_value` d'un uid complet (`solution:REJET`).
    pub fn value(&self, uid: &str) -> Option<&FacetValueEntry> {
        self.values.get(uid)
    }

    /// Label FR d'un uid complet ; repli transparent sur le **suffixe** d'uid
    /// si l'entrée manque.
    pub fn label<'a>(&'a self, uid: &'a str) -> &'a str {
        self.values
            .get(uid)
            .map(|e| e.label.as_str())
            .unwrap_or_else(|| uid_suffix(uid))
    }

    /// Ce que la valeur recouvre quand elle en regroupe plusieurs, `None` sinon.
    pub fn hint(&self, uid: &str) -> Option<String> {
        self.values.get(uid).and_then(|e| e.hint.clone())
    }

    /// Rang de lecture d'un uid, `i32::MAX` si l'entrée manque : un uid hors
    /// référentiel se lit après ceux qui y sont.
    pub fn rank(&self, uid: &str) -> i32 {
        self.values.get(uid).map_or(i32::MAX, |e| e.sort)
    }

    /// Tag référentiel d'un uid complet : `key` = suffixe d'uid, `label` résolu.
    pub fn tag(&self, uid: &str) -> FacetTag {
        FacetTag {
            key: uid_suffix(uid).to_string(),
            label: self.label(uid).to_string(),
        }
    }

    /// Label d'un type de juridiction (`TJ` → « Tribunal judiciaire »), depuis
    /// les lignes `jurisdiction_type:*`.
    pub fn jurisdiction_type_label(&self, code: &str) -> Option<&str> {
        self.values
            .get(&format!("jurisdiction_type:{code}"))
            .map(|e| e.label.as_str())
    }

    pub fn with_case_names(mut self, keys: Vec<String>) -> Self {
        self.case_names = keys.into_iter().collect();
        self
    }

    pub fn with_named_decisions(mut self, named: Vec<(i64, Vec<String>)>) -> Self {
        let (ids, keys): (Vec<i64>, Vec<Vec<String>>) = named.into_iter().unzip();
        self.names = lj_core::case_names::NameIndex::new(keys);
        self.name_ids = ids;
        self
    }

    /// The named decisions a name's words designate, as [`lj_core::case_names::NameIndex::bearers`].
    pub fn name_bearers(&self, words: &[&str], max: usize) -> Vec<i64> {
        self.names
            .bearers(words, max)
            .into_iter()
            .map(|rank| self.name_ids[rank])
            .collect()
    }

    /// Whether these candidate keys can name a case. An unloaded sieve answers
    /// yes and leaves the question to Postgres: emptied, it would report every
    /// query as naming nothing, which silently kills the name leg.
    pub fn names_a_case(&self, keys: &[String]) -> bool {
        !keys.is_empty()
            && (self.case_names.is_empty() || keys.iter().any(|k| self.case_names.contains(k)))
    }

    /// Entrée `jurisdiction` d'un code (`tj_le_havre`).
    pub fn jurisdiction(&self, code: &str) -> Option<&JurisdictionEntry> {
        self.jurisdictions.get(code)
    }

    pub fn seat_words(&self) -> &std::collections::HashSet<String> {
        &self.seat_words
    }

    /// The court of type `kind` the folded `words` seat: the only court of its
    /// type, or the one whose city they write, its first words at least
    /// (« Saint-Denis » for « Saint-Denis de la Réunion »). `None` when two
    /// courts of the type fit as well, or none.
    pub fn court_code(&self, kind: &str, words: &[&str]) -> Option<String> {
        let of_kind: Vec<(&String, &JurisdictionEntry)> = self
            .jurisdictions
            .iter()
            .filter(|(_, e)| e.jurisdiction_type == kind)
            .collect();
        if let [(code, e)] = of_kind[..] {
            if e.city.is_none() {
                return Some(code.clone());
            }
        }
        let article = |w: &&String| SEAT_ARTICLES.contains(&w.as_str());
        let mut scored: Vec<(usize, &String)> = of_kind
            .iter()
            .filter_map(|(code, e)| {
                let city = folded_words(e.city.as_deref()?);
                let written = |w: &String| words.contains(&w.as_str());
                let mut core = city.iter().skip_while(article).take_while(|w| !article(w));
                let first = core.next()?;
                (written(first) && core.all(written))
                    .then(|| (city.iter().filter(|w| written(w)).count(), *code))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        match scored[..] {
            [(best, code), (second, _), ..] if best > second => Some(code.clone()),
            [(_, code)] => Some(code.clone()),
            _ => None,
        }
    }

    /// Whether `words` write the city of the court of type `kind` they name: « paris »
    /// after « caa », never « béziers » after « conseil d'État ».
    pub fn seats_court(&self, kind: &str, words: &[&str]) -> bool {
        self.court_code(kind, words)
            .and_then(|code| self.jurisdictions.get(&code))
            .is_some_and(|e| e.city.is_some())
    }

    /// Court label under any writing of its unit: code (`tj_le_havre`), source
    /// location (`tj76351`, `7501`) or name (`tribunal judiciaire du havre`).
    pub fn court_by_key(&self, key: &str) -> Option<&str> {
        self.courts
            .get(&lj_fold::slug(key, '-'))
            .map(String::as_str)
    }

    /// Itère les unités juridictionnelles `(code, entrée)`.
    pub fn jurisdictions(&self) -> impl Iterator<Item = (&str, &JurisdictionEntry)> {
        self.jurisdictions.iter().map(|(c, e)| (c.as_str(), e))
    }

    /// Tokens (suffixes d'uid) d'une facette, triés — le vocabulaire valide
    /// d'un axe à catégorie contrôlée (`publication`).
    pub fn facet_tokens(&self, facet: &str) -> Vec<&str> {
        let mut tokens: Vec<&str> = self
            .values
            .iter()
            .filter(|(_, e)| e.facet == facet)
            .map(|(uid, _)| uid_suffix(uid))
            .collect();
        tokens.sort_unstable();
        tokens
    }

    /// Premier token hors vocabulaire d'un axe à catégorie contrôlée.
    pub fn find_unknown_token<'a>(&self, facet: &str, tokens: &'a [String]) -> Option<&'a str> {
        tokens
            .iter()
            .map(String::as_str)
            .find(|t| !self.values.contains_key(&format!("{facet}:{t}")))
    }
}

/// Message correctif pour un `jurisdiction_code` hors référentiel, avec
/// suggestions : code source exact (« tj75056 » → « tj_paris »), segment de
/// libellé matchant le lieu demandé — graphies ultramarines comprises via
/// [`lj_core::outremer`], sans quoi « ta_cayenne » ne trouve jamais
/// « ta_guyane » —, et codes proches en distance d'édition. Les unités du
/// **type** demandé passent devant : `ta_cayenne` cherche un tribunal
/// administratif, pas la cour d'appel homonyme.
pub fn unknown_jurisdiction_code_msg(code: &str, refs: &Referential) -> String {
    // Préfixe de type effectivement porté par des codes du référentiel
    // (« ta_ », « tcom_ ») — sinon le code entier est du texte libre.
    let kind = code
        .split_once('_')
        .map(|(k, _)| format!("{k}_"))
        .filter(|k| refs.jurisdictions().any(|(c, _)| c.starts_with(k)));
    let place = kind.as_deref().map_or(code, |k| &code[k.len()..]);
    let mut needles = vec![lj_fold::slug(place, '-')];
    // Dernier segment : les codes devinés portent souvent la ville en queue
    // (« tribunal_paris », « conseil_etat »).
    if let Some(last) = place.rsplit(['_', '-']).next() {
        needles.push(lj_fold::slug(last, '-'));
    }
    for n in needles.clone() {
        needles.extend(
            lj_core::outremer::equivalents(&n)
                .iter()
                .map(|s| (*s).into()),
        );
    }

    // Tri : lieu reconnu d'abord (le seul signal fiable), puis type demandé,
    // puis distance.
    let mut scored: Vec<(bool, bool, usize, String, String)> = refs
        .jurisdictions()
        .filter_map(|(c, e)| {
            let label = lj_fold::slug(&e.label, '-');
            let distance = levenshtein(code, c);
            let place_hit = e.source_code == code || needles.iter().any(|n| segment_of(&label, n));
            let same_kind = kind.as_deref().is_some_and(|k| c.starts_with(k));
            (place_hit || distance <= 3).then(|| {
                (
                    !place_hit,
                    !same_kind,
                    distance,
                    c.to_string(),
                    e.label.clone(),
                )
            })
        })
        .collect();
    scored.sort();
    let matches: Vec<String> = scored
        .iter()
        .take(5)
        .map(|(_, _, _, c, label)| format!("\"{c}\" ({label})"))
        .collect();
    let hint = if matches.is_empty() {
        String::new()
    } else {
        format!("; did you mean: {}", matches.join(", "))
    };
    format!("unknown jurisdiction_code '{code}'{hint}")
}

/// The correction for an unknown seat path, which the fund carries no node of
/// or which is misspelt: the neighbouring nodes under the same parent.
pub fn unknown_seat_msg(path: &str, nodes: &[String]) -> String {
    let parent = path.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
    let voisins: Vec<&str> = nodes
        .iter()
        .filter(|n| n.rsplit_once('.').map(|(p, _)| p).unwrap_or("") == parent)
        .map(String::as_str)
        .take(8)
        .collect();
    if voisins.is_empty() {
        return format!(
            "Chemin de siège inconnu : « {path} ». Les chemins servis sont dans facets.seat."
        );
    }
    format!(
        "Chemin de siège inconnu : « {path} ». Sous « {parent} », le fonds porte : {}.",
        voisins.join(", ")
    )
}

/// The refusal of a path outside the « Formation » tree.
pub fn unknown_formation_msg(path: &str) -> String {
    format!(
        "Chemin de formation inconnu : « {path} ». Les chemins servis sont dans \
         facets.formation, par exemple « FR.TJ.JEX » ou « FR.CC.RESTREINTE »."
    )
}

/// The node a path written without its legal order names, when a single order
/// holds it: `CA.PARIS` for `FR.CA.PARIS`. Links and bookmarks written before
/// the order led the path still carry the short form.
pub fn seat_under_its_order<'a>(path: &str, nodes: &'a [String]) -> Option<&'a str> {
    let mut rooted = nodes
        .iter()
        .filter(|n| n.split_once('.').is_some_and(|(_, rest)| rest == path));
    match (rooted.next(), rooted.next()) {
        (Some(node), None) => Some(node),
        _ => None,
    }
}

/// Folds the two spellings callers write into the one the fund carries: none of
/// its nodes holds a hyphen, and a comma means a list joined into one string.
/// The seat whose last segment the caller abbreviated, `CA.AIX` for
/// `CA.AIX_EN_PROVENCE`: unique target or nothing, `TJ.SAINT` prefixing several.
/// The bound is the underscore, a dot being the hierarchy a prefix already covers.
pub fn seat_completion<'a>(path: &str, nodes: &'a [String]) -> Option<&'a str> {
    let mut hit = None;
    for n in nodes {
        if n.len() > path.len() && n.starts_with(path) && n.as_bytes()[path.len()] == b'_' {
            if hit.is_some() {
                return None;
            }
            hit = Some(n.as_str());
        }
    }
    hit
}

pub fn canonicalize_seats(paths: &mut Vec<String>) {
    *paths = paths
        .iter()
        .flat_map(|p| p.split(','))
        .map(|p| p.trim().replace('-', "_"))
        .filter(|p| !p.is_empty())
        .collect();
}

/// `needle` présent dans `hay` en segment entier (bornes `-` ou extrémités) —
/// une sous-chaîne libre fait matcher « ta_basse_terre » sur Nanterre.
fn segment_of(hay: &str, needle: &str) -> bool {
    needle.len() >= 3
        && hay.match_indices(needle).any(|(i, _)| {
            let end = i + needle.len();
            (i == 0 || hay.as_bytes()[i - 1] == b'-')
                && (end == hay.len() || hay.as_bytes()[end] == b'-')
        })
}

/// Distance de Levenshtein (DP à une ligne), pour les suggestions de codes.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Every slug under which a chronology key may name this unit. The Judilibre
/// location of a `TCOM` is bare (`7501`) where its source code is prefixed
/// (`tcom7501`), so both are indexed.
fn court_keys(row: &JurisdictionRow) -> Vec<(String, String)> {
    let mut keys = vec![
        lj_fold::slug(&row.code, '-'),
        lj_fold::slug(&row.source_code, '-'),
        lj_fold::slug(&row.label, '-'),
    ];
    if let Some(bare) = row.source_code.strip_prefix("tcom") {
        keys.push(lj_fold::slug(bare, '-'));
    }
    keys.into_iter().map(|k| (k, row.label.clone())).collect()
}

const SEAT_ARTICLES: &[&str] = &[
    "de", "du", "des", "la", "le", "les", "l", "d", "sur", "en", "a", "et",
];

fn folded_words(s: &str) -> Vec<String> {
    lj_fold::fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Suffixe d'un uid namespacé (`solution:REJET` → `REJET`).
pub fn uid_suffix(uid: &str) -> &str {
    uid.split_once(':').map(|(_, s)| s).unwrap_or(uid)
}

/// Référentiel depuis le cache de l'état (TTL 1 h), chargé au premier accès.
pub async fn referential(state: &AppState) -> Result<Arc<Referential>> {
    state
        .referential_cache
        .try_get_with(String::new(), load(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("referential load: {e}")))
}

async fn load(state: &AppState) -> Result<Arc<Referential>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let values = repo.load_facet_values().await.map_err(ApiError::Store)?;
    let jurisdictions = repo.load_jurisdictions().await.map_err(ApiError::Store)?;
    let seats = repo.load_seat_paths().await.map_err(ApiError::Store)?;
    let case_names = repo.case_name_key_set().await.map_err(ApiError::Store)?;
    let named = repo.named_decisions().await.map_err(ApiError::Store)?;
    Ok(Arc::new(
        Referential::new(values, jurisdictions, seats)
            .with_case_names(case_names)
            .with_named_decisions(named),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_crible_des_noms_laisse_passer_le_doute_jamais_le_silence() {
        let cle = |s: &str| vec![s.to_string()];
        let charge = Referential::default().with_case_names(cle("danthony"));
        assert!(charge.names_a_case(&cle("danthony")));
        assert!(!charge.names_a_case(&cle("congés payés")));
        assert!(!charge.names_a_case(&[]));
        assert!(Referential::default().names_a_case(&cle("congés payés")));
    }

    fn jr(code: &str, jurisdiction_type: &str, label: &str) -> JurisdictionRow {
        JurisdictionRow {
            code: code.to_string(),
            source_code: code.to_string(),
            jurisdiction_type: jurisdiction_type.to_string(),
            city: None,
            label: label.to_string(),
            place: None,
        }
    }

    fn fv(uid: &str, label: &str, parent: Option<&str>) -> FacetValueRow {
        FacetValueRow {
            uid: uid.to_string(),
            facet: uid.split(':').next().unwrap().to_string(),
            label: label.to_string(),
            abbr: None,
            parent_uid: parent.map(str::to_string),
            sort: 0,
            hint: None,
        }
    }

    fn jrv(code: &str, jurisdiction_type: &str, city: &str, label: &str) -> JurisdictionRow {
        JurisdictionRow {
            city: Some(city.to_string()),
            ..jr(code, jurisdiction_type, label)
        }
    }

    #[test]
    fn la_cour_citee_se_lit_par_son_type_et_son_siege() {
        let refs = Referential::new(
            vec![],
            vec![
                jr("ce", "CE", "Conseil d'État"),
                jrv(
                    "caa_paris",
                    "CAA",
                    "Paris",
                    "Cour administrative d'appel de Paris",
                ),
                jrv(
                    "caa_lyon",
                    "CAA",
                    "Lyon",
                    "Cour administrative d'appel de Lyon",
                ),
                jrv("tj_paris", "TJ", "Paris", "Tribunal judiciaire de Paris"),
                jrv(
                    "tj_le_havre",
                    "TJ",
                    "Le Havre",
                    "Tribunal judiciaire du Havre",
                ),
                jrv(
                    "tj_saint_denis_reunion",
                    "TJ",
                    "Saint-Denis de la Réunion",
                    "Tribunal judiciaire de Saint-Denis de la Réunion",
                ),
                jrv(
                    "tj_saint_malo",
                    "TJ",
                    "Saint-Malo",
                    "Tribunal judiciaire de Saint-Malo",
                ),
            ],
            Vec::new(),
        );
        let code = |kind: &str, q: &str| {
            let folded = lj_core::case_names::case_name_key(q);
            let words: Vec<&str> = folded.split(' ').collect();
            refs.court_code(kind, &words)
        };
        assert_eq!(code("CE", "").as_deref(), Some("ce"));
        assert_eq!(code("CAA", "paris").as_deref(), Some("caa_paris"));
        assert_eq!(code("TJ", "de paris").as_deref(), Some("tj_paris"));
        assert_eq!(code("TJ", "du havre").as_deref(), Some("tj_le_havre"));
        assert_eq!(
            code("TJ", "saint denis").as_deref(),
            Some("tj_saint_denis_reunion")
        );
        assert_eq!(code("TJ", "saint").as_deref(), None);
        assert_eq!(code("CAA", "23 avril 1998").as_deref(), None);
        assert!(refs.seat_words().contains("havre"));
        assert!(refs.seat_words().contains("reunion"));
    }

    #[test]
    fn tag_resolves_label_and_falls_back_to_suffix() {
        let refs = Referential::new(
            vec![
                fv("solution:REJET", "Rejet", None),
                fv("jurisdiction_type:TJ", "Tribunal judiciaire", None),
            ],
            vec![JurisdictionRow {
                code: "tj_le_havre".to_string(),
                source_code: "tj76351".to_string(),
                jurisdiction_type: "TJ".to_string(),
                city: Some("Le Havre".to_string()),
                label: "Tribunal judiciaire du Havre".to_string(),
                place: None,
            }],
            Vec::new(),
        );
        let tag = refs.tag("solution:REJET");
        assert_eq!((tag.key.as_str(), tag.label.as_str()), ("REJET", "Rejet"));
        let tag = refs.tag("solution:INCONNU");
        assert_eq!(tag.label, "INCONNU");
        assert_eq!(
            refs.jurisdiction_type_label("TJ"),
            Some("Tribunal judiciaire")
        );
        assert_eq!(
            refs.jurisdiction("tj_le_havre").map(|j| j.label.as_str()),
            Some("Tribunal judiciaire du Havre")
        );
    }

    /// An unknown seat path names its siblings under the same parent.
    #[test]
    fn unknown_seat_nomme_les_voisins_du_parent() {
        let noeuds = vec![
            "FR".to_string(),
            "FR.CA".to_string(),
            "FR.CA.PARIS".to_string(),
            "FR.CA.PARIS.C3".to_string(),
            "FR.CA.PARIS.P6".to_string(),
        ];

        let inconnu = unknown_seat_msg("FR.CA.PARIS.C9", &noeuds);
        assert!(inconnu.contains("FR.CA.PARIS.C3"), "{inconnu}");
        assert!(inconnu.contains("FR.CA.PARIS.P6"), "{inconnu}");

        let orphelin = unknown_seat_msg("FR.CA.PARSI.C3", &noeuds);
        assert!(orphelin.contains("facets.seat"), "{orphelin}");

        assert_eq!(
            seat_under_its_order("CA.PARIS", &noeuds),
            Some("FR.CA.PARIS")
        );
        assert_eq!(seat_under_its_order("FR.CA.PARIS", &noeuds), None);
    }

    /// `CA.AIX` for `CA.AIX_EN_PROVENCE`: the completion holds only when it
    /// designates one seat, and never across the dot.
    #[test]
    fn le_segment_abrege_se_complete_s_il_designe_un_seul_siege() {
        let noeuds = vec![
            "FR.CA".to_string(),
            "FR.CA.AIX_EN_PROVENCE".to_string(),
            "FR.CA.PARIS".to_string(),
            "FR.CA.PARIS.C3".to_string(),
            "FR.TJ.SAINT_DENIS".to_string(),
            "FR.TJ.SAINT_ETIENNE".to_string(),
        ];
        assert_eq!(
            seat_completion("FR.CA.AIX", &noeuds),
            Some("FR.CA.AIX_EN_PROVENCE")
        );
        // Two targets: the question stands.
        assert_eq!(seat_completion("FR.TJ.SAINT", &noeuds), None);
        // The dot is the hierarchy, which a prefix already covers.
        assert_eq!(seat_completion("FR.CA.PARIS", &noeuds), None);
        assert_eq!(seat_completion("FR.CA.NULLE", &noeuds), None);
    }

    /// A hyphenated city and two paths joined in one string fold to the fund's spelling.
    #[test]
    fn les_graphies_d_appelant_se_replient_sur_celle_du_fonds() {
        let mut un = vec!["CA.AIX-EN-PROVENCE".to_string()];
        canonicalize_seats(&mut un);
        assert_eq!(un, vec!["CA.AIX_EN_PROVENCE"]);

        let mut jointe = vec!["CC.SOCIALE, CC.COMMERCIALE".to_string()];
        canonicalize_seats(&mut jointe);
        assert_eq!(jointe, vec!["CC.SOCIALE", "CC.COMMERCIALE"]);

        let mut vide = vec!["CC,".to_string(), String::new()];
        canonicalize_seats(&mut vide);
        assert_eq!(vide, vec!["CC"]);
    }

    #[test]
    fn unknown_code_suggests_accented_label_from_ascii_needle() {
        let refs = Referential::new(
            vec![],
            vec![JurisdictionRow {
                code: "ce".to_string(),
                source_code: "CONSETAT".to_string(),
                jurisdiction_type: "CE".to_string(),
                city: None,
                label: "Conseil d'État".to_string(),
                place: None,
            }],
            Vec::new(),
        );
        assert_eq!(
            unknown_jurisdiction_code_msg("conseil_etat", &refs),
            "unknown jurisdiction_code 'conseil_etat'; did you mean: \"ce\" (Conseil d'État)"
        );
    }

    #[test]
    fn unknown_code_bridges_outremer_naming_and_ranks_the_asked_type_first() {
        let refs = Referential::new(
            vec![],
            vec![
                jr("ta_guyane", "TA", "Tribunal administratif de la Guyane"),
                jr("ca_cayenne", "CA", "Cour d'appel de Cayenne"),
                jr("ta_caen", "TA", "Tribunal administratif de Caen"),
                jr("tcom_nanterre", "TCOM", "Tribunal de commerce de Nanterre"),
                jr(
                    "ta_guadeloupe",
                    "TA",
                    "Tribunal administratif de la Guadeloupe",
                ),
            ],
            Vec::new(),
        );
        assert_eq!(
            unknown_jurisdiction_code_msg("ta_cayenne", &refs),
            "unknown jurisdiction_code 'ta_cayenne'; did you mean: \
             \"ta_guyane\" (Tribunal administratif de la Guyane), \
             \"ca_cayenne\" (Cour d'appel de Cayenne), \
             \"ta_caen\" (Tribunal administratif de Caen)"
        );
        let msg = unknown_jurisdiction_code_msg("ta_basse_terre", &refs);
        assert!(msg.contains("ta_guadeloupe"), "{msg}");
        assert!(!msg.contains("nanterre"), "{msg}");
    }

    #[test]
    fn facet_tokens_and_unknown_token_lookup() {
        let refs = Referential::new(
            vec![
                fv("matiere:SOCIALE", "Chambre sociale", None),
                fv("matiere:CIVILE", "Chambre civile", None),
                fv("publication:B", "Bulletin", None),
            ],
            vec![],
            Vec::new(),
        );
        assert_eq!(refs.facet_tokens("matiere"), vec!["CIVILE", "SOCIALE"]);
        let values = vec!["SOCIALE".to_string(), "JAF".to_string()];
        assert_eq!(refs.find_unknown_token("matiere", &values), Some("JAF"));
        assert_eq!(
            refs.find_unknown_token("publication", &["B".to_string()]),
            None
        );
    }
}
