//! Probe du FST d'autocomplétion (ADR 0216) : vocabulaire n-grammes 1-5
//! partagé jurisprudence/textes (valeur `u64 = df_juris << 32 | df_textes`),
//! sondé par préfixe du contexte le plus long au plus court.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use fst::automaton::{Automaton, Str};
use fst::{IntoStreamer, Map, Streamer};

use lj_core::body_tok::tokenize;
use lj_core::suggest::{DISPLAY_SEP, MAX_KEY_TOKENS};
use lj_store::repository::DecisionRepository;

use crate::entities;
use crate::error::{validation, ApiError, Result};
use crate::state::AppState;

/// Suggestions retournées par requête.
const SUGGEST_LIMIT: usize = 5;
const SUGGEST_MODES: &[&str] = &["jurisprudence", "textes", "annuaire"];

/// Service `GET /suggest` : probe du FST pour jurisprudence/textes, délégation
/// à la recherche d'entités pour l'annuaire (les dénominations sont le
/// vocabulaire). `q` déjà validée (≥ 2 codepoints) par la route.
pub async fn suggest(state: &AppState, q: &str, mode: &str) -> Result<lj_dtos::SuggestResponse> {
    let domain = match mode {
        "jurisprudence" => SuggestDomain::Jurisprudence,
        "textes" => SuggestDomain::Textes,
        "annuaire" => {
            let found = entities::entity_search(state, q, None, SUGGEST_LIMIT as i64).await?;
            return Ok(lj_dtos::SuggestResponse {
                matched_tokens: q.split_whitespace().count() as u32,
                suggestions: found.items.into_iter().map(|i| i.denomination).collect(),
            });
        }
        other => {
            return Err(ApiError::Unprocessable(validation::enum_error(
                &["query", "mode"],
                other,
                SUGGEST_MODES,
            )))
        }
    };
    let index = suggest_index(state).await?;
    let (matched_tokens, suggestions) = index.suggest(q, domain, SUGGEST_LIMIT);
    Ok(lj_dtos::SuggestResponse {
        matched_tokens,
        suggestions,
    })
}

/// FST d'autocomplétion, chargé du blob `suggest_index` au premier appel
/// (cache mono-entrée, TTL 24 h). Blob absent = index vide + warn : état
/// documenté d'avant premier `build-suggest` (ADR 0216), jamais une 500.
async fn suggest_index(state: &AppState) -> Result<Arc<SuggestIndex>> {
    state
        .suggest_cache
        .try_get_with((), async {
            let conn = state
                .pool
                .get()
                .await
                .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
            let repo = DecisionRepository::new(&conn);
            match repo
                .fetch_suggest_fst(lj_store::repository::SUGGEST_FST_KEY)
                .await
                .map_err(ApiError::Store)?
            {
                Some(bytes) => SuggestIndex::from_bytes(bytes)
                    .map(Arc::new)
                    .map_err(|e| ApiError::Internal(format!("blob suggest_index corrompu: {e}"))),
                None => {
                    tracing::warn!("suggest_index vide — lancer `lj-ingest build-suggest`");
                    Ok(Arc::new(SuggestIndex::empty()))
                }
            }
        })
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("suggest load: {e}")))
}

/// Moitié du `u64` packé qui ranke le domaine interrogé.
#[derive(Debug, Clone, Copy)]
pub enum SuggestDomain {
    Jurisprudence,
    Textes,
}

impl SuggestDomain {
    fn df(self, packed: u64) -> u32 {
        match self {
            SuggestDomain::Jurisprudence => (packed >> 32) as u32,
            SuggestDomain::Textes => (packed & u64::from(u32::MAX)) as u32,
        }
    }
}

/// Garde-fou d'énumération : un préfixe de 2 chars balaye un gros sous-arbre ;
/// au-delà de ce budget le top-k courant suffit (le vocabulaire est ≤ 3 M clés,
/// l'énumération complète reste en ms — c'est une borne franche, pas un tuning).
const SCAN_CAP: usize = 500_000;

/// Sur-échantillonnage du top-k avant dédup : un candidat dont une extension
/// du pool porte au moins la moitié du df est redondant (« code de
/// procédure » quand « code de procédure civile » domine) — l'extension, plus
/// informative, prend sa place (biais Google vers les complétions longues).
/// Un candidat aux extensions toutes faibles reste (« bail » face à un
/// « bail commercial statut » marginal).
const DEDUP_POOL: usize = 4;

/// FST d'autocomplétion chargé en mémoire (blob `suggest_index`).
pub struct SuggestIndex {
    map: Map<Vec<u8>>,
}

impl SuggestIndex {
    /// Charge le blob sérialisé (erreur franche si le blob n'est pas un FST).
    pub fn from_bytes(bytes: Vec<u8>) -> std::result::Result<Self, fst::Error> {
        Ok(Self {
            map: Map::new(bytes)?,
        })
    }

    /// Index vide : état d'avant premier `build-suggest` (zéro suggestion).
    pub fn empty() -> Self {
        let bytes = fst::MapBuilder::memory()
            .into_inner()
            .expect("FST vide en mémoire");
        Self {
            map: Map::new(bytes).expect("FST vide valide"),
        }
    }

    /// Suggestions top-`k` pour `q` dans `domain`. Renvoie
    /// `(matched_tokens, suggestions)` : le nombre de mots de fin de query que
    /// chaque suggestion remplace. Sonde du contexte le plus long au plus
    /// court — premier palier non vide gagne.
    pub fn suggest(&self, q: &str, domain: SuggestDomain, k: usize) -> (u32, Vec<String>) {
        let toks = tokenize(q);
        // Fin sur un séparateur = dernier mot complet (le probe cherche la
        // suite) ; sinon le dernier token est le mot en cours de frappe.
        let mid_word = q.chars().last().is_some_and(|c| c.is_alphanumeric());
        let (completed, partial) = match (mid_word, toks.split_last()) {
            (true, Some((last, rest))) => (rest, last.as_str()),
            _ => (&toks[..], ""),
        };

        // Contexte sondé jusqu'à MAX_KEY_TOKENS-1 mots : au-delà des trigrammes,
        // seuls les titres injectés entiers peuvent matcher — un palier raté
        // coûte O(len(préfixe)), les paliers longs sont gratuits.
        for ctx in (0..=completed.len().min(MAX_KEY_TOKENS - 1)).rev() {
            let context = &completed[completed.len() - ctx..];
            if context.is_empty() && partial.is_empty() {
                break;
            }
            let mut prefix = context.join(" ");
            if !prefix.is_empty() {
                prefix.push(' ');
            }
            prefix.push_str(partial);
            let out = self.complete(&prefix, domain, k);
            if !out.is_empty() {
                return ((ctx + usize::from(mid_word)) as u32, out);
            }
        }
        (0, Vec::new())
    }

    /// Top-`k` des clés du sous-arbre `prefix`, par df décroissant du domaine,
    /// déduplication des candidats dominés par une de leurs extensions
    /// ([`DEDUP_POOL`]). Les clés portent `folded[\x00display]` : le match
    /// préfixe opère sur la part pliée, la suggestion rendue est la forme
    /// d'affichage (accentuée).
    fn complete(&self, prefix: &str, domain: SuggestDomain, k: usize) -> Vec<String> {
        let automaton = Str::new(prefix).starts_with();
        let mut stream = self.map.search(automaton).into_stream();
        // Min-heap sur-dimensionnée (k × DEDUP_POOL) : la dédup pioche ses
        // remplaçants dans ce pool.
        let mut heap: BinaryHeap<Reverse<(u32, Vec<u8>)>> = BinaryHeap::new();
        let mut scanned = 0usize;
        while let Some((key, packed)) = stream.next() {
            scanned += 1;
            let df = domain.df(packed);
            if df > 0 && folded_part(key) != prefix.as_bytes() {
                heap.push(Reverse((df, key.to_vec())));
                if heap.len() > k * DEDUP_POOL {
                    heap.pop();
                }
            }
            if scanned >= SCAN_CAP {
                break;
            }
        }
        let mut pool: Vec<(u32, Vec<u8>)> = heap.into_iter().map(|Reverse(e)| e).collect();
        pool.sort_unstable_by_key(|(df, _)| Reverse(*df));
        let dominated = |df: u32, folded: &[u8]| {
            // Une référence chiffrée est une citation complète : ses
            // extensions (« article 700 du code ») sont du remplissage,
            // jamais un remplacement. Un token d'1-2 chiffres nus est un
            // fragment de date (« 78-17 du 6 »), pas une référence — il ne
            // protège pas.
            let ends_in_ref = folded.rsplit(|b| *b == b' ').next().is_some_and(|last| {
                last.iter().any(u8::is_ascii_digit)
                    && (last.len() >= 3 || last.iter().any(|b| !b.is_ascii_digit()))
            });
            !ends_in_ref
                && pool.iter().any(|(df_ext, key_ext)| {
                    let ext = folded_part(key_ext);
                    ext.len() > folded.len()
                        && ext.starts_with(folded)
                        && ext[folded.len()] == b' '
                        && u64::from(*df_ext) * 2 >= u64::from(df)
                })
        };
        pool.iter()
            .filter(|(df, key)| !dominated(*df, folded_part(key)))
            .take(k)
            .map(|(_, key)| {
                let display = match key.iter().position(|b| *b == DISPLAY_SEP) {
                    Some(i) => key[i + 1..].to_vec(),
                    None => key.clone(),
                };
                String::from_utf8(display).expect("clé FST utf-8 (construite de tokens)")
            })
            .collect()
    }
}

/// Part pliée (avant [`DISPLAY_SEP`]) d'une clé `folded[\x00display]`.
fn folded_part(key: &[u8]) -> &[u8] {
    key.split(|b| *b == DISPLAY_SEP).next().unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FST minuscule : (clé pliée, df_juris, df_textes). La clé peut porter
    /// une forme d'affichage via `"folded\x00display"` (comme au build).
    fn index(entries: &[(&str, u32, u32)]) -> SuggestIndex {
        let mut sorted: Vec<_> = entries.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        let mut b = fst::MapBuilder::memory();
        for (key, dj, dt) in sorted {
            b.insert(key, (u64::from(dj) << 32) | u64::from(dt))
                .unwrap();
        }
        SuggestIndex::from_bytes(b.into_inner().unwrap()).unwrap()
    }

    #[test]
    fn forme_affichee_accentuee_rendue_et_matchee_pliee() {
        // La clé porte `folded\x00display` : on matche plié, on rend accentué.
        let idx = index(&[
            ("conges payes\u{0}congés payés", 800, 10),
            ("congestion", 100, 5),
        ]);
        let (matched, out) = idx.suggest("congés pa", SuggestDomain::Jurisprudence, 5);
        assert_eq!(out, vec!["congés payés".to_string()]);
        assert_eq!(matched, 2);
        // L'exclusion du préfixe exactement saisi compare la part PLIÉE.
        let (_, out) = idx.suggest("conges payes", SuggestDomain::Jurisprudence, 5);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn contexte_le_plus_long_gagne() {
        let idx = index(&[
            ("commercant", 500, 10),
            ("commerce", 900, 50),
            ("tribunal de commerce", 300, 5),
        ]);
        // Mi-mot avec contexte : le trigramme « tribunal de commerce » sort au
        // palier ctx=2, avant les unigrammes en "com".
        let (matched, out) = idx.suggest("tribunal de com", SuggestDomain::Jurisprudence, 5);
        assert_eq!(out, vec!["tribunal de commerce".to_string()]);
        // Remplace « de com » ? Non : ctx=2 (« tribunal de ») + mot en cours = 3.
        assert_eq!(matched, 3);
    }

    #[test]
    fn repli_sur_unigramme_et_tri_par_df() {
        let idx = index(&[("commercant", 500, 10), ("commerce", 900, 50)]);
        let (matched, out) = idx.suggest("bail com", SuggestDomain::Jurisprudence, 5);
        // « bail com » n'existe pas en bigramme → repli unigramme, df décroissant.
        assert_eq!(matched, 1);
        assert_eq!(out, vec!["commerce".to_string(), "commercant".to_string()]);
    }

    #[test]
    fn domaine_a_df_nul_saute() {
        let idx = index(&[("commerce", 900, 0), ("commission", 0, 40)]);
        let (_, juris) = idx.suggest("com", SuggestDomain::Jurisprudence, 5);
        assert_eq!(juris, vec!["commerce".to_string()]);
        let (_, textes) = idx.suggest("com", SuggestDomain::Textes, 5);
        assert_eq!(textes, vec!["commission".to_string()]);
    }

    #[test]
    fn fin_sur_espace_complete_la_suite() {
        let idx = index(&[("tribunal de commerce", 300, 5), ("tribunal", 800, 20)]);
        // Query close par un espace : le dernier mot est complet, on suggère la
        // suite du contexte (et la clé égale au préfixe seul est exclue).
        let (matched, out) = idx.suggest("tribunal ", SuggestDomain::Jurisprudence, 5);
        assert_eq!(out, vec!["tribunal de commerce".to_string()]);
        assert_eq!(matched, 1);
    }

    #[test]
    fn titre_plus_long_que_le_trigramme_matche_en_contexte() {
        // Un titre injecté entier (4 tokens > MAX_N) doit rester atteignable
        // en tapant son début : le contexte sondé dépasse les trigrammes.
        let idx = index(&[("code de la route", 0, 12_000), ("route", 0, 900)]);
        let (matched, out) = idx.suggest("code de la ro", SuggestDomain::Textes, 5);
        assert_eq!(out, vec!["code de la route".to_string()]);
        assert_eq!(matched, 4);
    }

    #[test]
    fn prefixe_domine_par_son_extension_deduplique() {
        // « code » et « code de procedure » sont dominés par une extension
        // (df ≥ la moitié du leur) : l'extension, plus informative, les
        // remplace. « code civil », sans extension, reste.
        let idx = index(&[
            ("code", 300, 0),
            ("code civil", 150, 0),
            ("code de procedure", 200, 0),
            ("code de procedure civile", 180, 0),
        ]);
        let (_, out) = idx.suggest("cod", SuggestDomain::Jurisprudence, 5);
        assert_eq!(
            out,
            vec![
                "code de procedure civile".to_string(),
                "code civil".to_string()
            ]
        );
    }

    #[test]
    fn reference_chiffree_jamais_evincee_par_ses_extensions() {
        // « article 700 » est une citation complète : son extension dominante
        // ne l'évince pas — les deux sortent, par df décroissant.
        let idx = index(&[("article 700", 300, 0), ("article 700 du code", 250, 0)]);
        let (_, out) = idx.suggest("article 70", SuggestDomain::Jurisprudence, 5);
        assert_eq!(
            out,
            vec!["article 700".to_string(), "article 700 du code".to_string()]
        );
    }

    #[test]
    fn fragment_de_date_non_protege() {
        // « loi du 29 » finit par 2 chiffres nus : fragment de date, dominé
        // par la citation complète comme n'importe quel préfixe.
        let idx = index(&[("loi du 29", 100, 0), ("loi du 29 juillet 1881", 90, 0)]);
        let (_, out) = idx.suggest("loi du 2", SuggestDomain::Jurisprudence, 5);
        assert_eq!(out, vec!["loi du 29 juillet 1881".to_string()]);
    }

    #[test]
    fn prefixe_aux_extensions_faibles_conserve() {
        // L'extension marginale (df < la moitié) ne chasse pas son préfixe.
        let idx = index(&[("bail", 500, 0), ("bail commercial statut", 100, 0)]);
        let (_, out) = idx.suggest("ba", SuggestDomain::Jurisprudence, 5);
        assert_eq!(
            out,
            vec!["bail".to_string(), "bail commercial statut".to_string()]
        );
    }

    #[test]
    fn index_vide_zero_suggestion() {
        let (matched, out) = SuggestIndex::empty().suggest("com", SuggestDomain::Textes, 5);
        assert_eq!(matched, 0);
        assert!(out.is_empty());
    }
}
