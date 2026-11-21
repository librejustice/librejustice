//! Ordre de lecture des articles d'un code — clé de tri naturelle de `num_key`.
//!
//! `position` (quand la source la fournit) prime ; cette clé ordonne le repli
//! quand elle est absente (codes LEGI) : préfixe de partie dans l'ordre de
//! lecture des codes français (articles non préfixés < L < LO < R < D < A <
//! autres), puis segments numériques comparés en **nombres** (« L. 2 » avant
//! « L. 10 », que le tri lexical inversait), puis chaîne brute en départage.

/// Clé de tri naturelle d'un `num_key` d'article : `(rang de partie, segments
/// numériques, chaîne brute)`.
pub fn num_key_sort_key(num_key: &str) -> (u8, Vec<u64>, String) {
    let prefix: String = num_key
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let rank = match prefix.to_ascii_uppercase().as_str() {
        // Articles non préfixés (Code civil : « 1240 »).
        "" => 0,
        "L" => 1,
        "LO" => 2,
        "R" => 3,
        "D" => 4,
        "A" => 5,
        // Annexes, tableaux, préfixes exotiques — en fin de sommaire.
        _ => 6,
    };
    let mut nums: Vec<u64> = Vec::new();
    let mut cur: Option<u64> = None;
    for c in num_key.chars() {
        if let Some(d) = c.to_digit(10) {
            cur = Some(
                cur.unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(u64::from(d)),
            );
        } else if let Some(n) = cur.take() {
            nums.push(n);
        }
    }
    if let Some(n) = cur {
        nums.push(n);
    }
    (rank, nums, num_key.to_string())
}

/// Clé naturelle d'un article.
type Key = (u8, Vec<u64>, String);

/// Trie `entries` dans l'ordre de lecture d'un code sans `position` : les
/// divisions (préfixes du `title_path`, segments « > ») sont ordonnées par la clé
/// **médiane** de leurs articles, les articles d'une division par leur propre clé.
/// La médiane rend l'ordre robuste aux numérotations aberrantes — dans le CJA, les
/// articles « L. 77-10-… » du Livre VII trient numériquement avant « L. 111-1 ».
pub fn sort_reading_order<T>(
    entries: &mut [T],
    num_key: impl Fn(&T) -> &str,
    title_path: impl Fn(&T) -> Option<&str>,
) {
    use std::collections::HashMap;

    let segments = |e: &T| -> Vec<String> {
        title_path(e)
            .map(|p| {
                p.split(" > ")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    // Clés des articles par préfixe de division (chaque article compte pour
    // tous ses ancêtres).
    let mut per_prefix: HashMap<Vec<String>, Vec<Key>> = HashMap::new();
    for e in entries.iter() {
        let key = num_key_sort_key(num_key(e));
        let segs = segments(e);
        for depth in 1..=segs.len() {
            per_prefix
                .entry(segs[..depth].to_vec())
                .or_default()
                .push(key.clone());
        }
    }
    let median: HashMap<Vec<String>, Key> = per_prefix
        .into_iter()
        .map(|(prefix, mut keys)| {
            keys.sort();
            let mid = keys.len() / 2;
            (prefix, keys.swap_remove(mid))
        })
        .collect();

    // Clé composite : médiane de chaque ancêtre, puis clé propre — les entrées
    // d'une même division restent groupées, les divisions s'ordonnent par leur
    // médiane à chaque niveau.
    entries.sort_by_cached_key(|e| {
        let segs = segments(e);
        let mut composite: Vec<Key> = (1..=segs.len())
            .filter_map(|depth| median.get(&segs[..depth]).cloned())
            .collect();
        composite.push(num_key_sort_key(num_key(e)));
        composite
    });
}

/// Ordre de lecture d'un texte entier, règle complète : quand la source fournit
/// des `position`, elles priment (`NULLS LAST`, départage par la clé naturelle) ;
/// quand elle n'en fournit aucune — le cas de tous les codes LEGI — l'ordre est
/// celui des divisions par clé médiane. Unique définition de l'ordre du corpus :
/// c'est elle que `refresh_reading_ranks` matérialise dans
/// `legal_article.reading_rank`.
pub fn sort_text_reading_order<T>(
    entries: &mut [T],
    num_key: impl Fn(&T) -> &str,
    title_path: impl Fn(&T) -> Option<&str>,
    position: impl Fn(&T) -> Option<i32>,
) {
    if entries.iter().all(|e| position(e).is_none()) {
        sort_reading_order(entries, num_key, title_path);
    } else {
        entries.sort_by_cached_key(|e| {
            let p = position(e);
            (p.is_none(), p, num_key_sort_key(num_key(e)))
        });
    }
}

/// The blocks of a text's own prose that are pieces of it, by `num_key`.
pub const BLOCKS: [&str; 4] = ["notice", "visas", "signataires", "tp"];

/// A block's place, the same in every source: before the first article or after the last.
pub fn block_position(key: &str) -> Option<i32> {
    match key {
        "notice" => Some(-2),
        "visas" => Some(-1),
        "signataires" => Some(i32::MAX - 1),
        "tp" => Some(i32::MAX),
        _ => None,
    }
}

/// Places a text's pieces, its rows with no number sorted by `position`, among
/// its `articles` already in reading order: by position when `printed`, every
/// article holding its printed place, else those below zero before the articles
/// and the others after.
pub fn place_pieces<T>(
    mut articles: Vec<T>,
    pieces: Vec<T>,
    printed: bool,
    position: impl Fn(&T) -> Option<i32>,
) -> Vec<T> {
    if printed {
        articles.extend(pieces);
        articles.sort_by_key(|e| (position(e).is_none(), position(e)));
        return articles;
    }
    let (before, after): (Vec<T>, Vec<T>) = pieces
        .into_iter()
        .partition(|e| position(e).is_some_and(|p| p < 0));
    before.into_iter().chain(articles).chain(after).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_take_their_printed_place() {
        let pos = |e: &(&str, Option<i32>)| e.1;
        let pieces = vec![
            ("visas", Some(-1)),
            ("annexe", Some(3)),
            ("signataires", Some(5)),
        ];
        let printed = vec![("1", Some(1)), ("2", Some(2)), ("3", Some(4))];
        let keys =
            |v: Vec<(&'static str, Option<i32>)>| v.into_iter().map(|e| e.0).collect::<Vec<_>>();
        assert_eq!(
            keys(place_pieces(printed, pieces.clone(), true, pos)),
            ["visas", "1", "2", "annexe", "3", "signataires"]
        );
        let consolidated = vec![("1", None), ("2", None), ("3", None)];
        assert_eq!(
            keys(place_pieces(consolidated, pieces, false, pos)),
            ["visas", "1", "2", "3", "annexe", "signataires"]
        );
    }

    fn sorted(mut keys: Vec<&str>) -> Vec<&str> {
        keys.sort_by_key(|k| num_key_sort_key(k));
        keys
    }

    #[test]
    fn numeric_segments_beat_lexical_order() {
        assert_eq!(
            sorted(vec!["L. 10", "L. 2", "L. 1", "L. 10-1", "L. 12"]),
            vec!["L. 1", "L. 2", "L. 10", "L. 10-1", "L. 12"]
        );
    }

    #[test]
    fn parts_follow_reading_order() {
        assert_eq!(
            sorted(vec![
                "A. 1", "D. 1", "R. 1", "LO 1", "L. 1", "Annexe I", "1240"
            ]),
            vec!["1240", "L. 1", "LO 1", "R. 1", "D. 1", "A. 1", "Annexe I"]
        );
    }

    #[test]
    fn reading_order_places_divisions_by_median() {
        let mut entries = vec![
            (
                "L. 77-10-3",
                Some("Partie législative > Livre VII : Le jugement"),
            ),
            ("L. 1", Some("Partie législative > Titre préliminaire")),
            (
                "L. 711-1",
                Some("Partie législative > Livre VII : Le jugement"),
            ),
            ("L. 511-1", Some("Partie législative > Livre V : Le référé")),
            (
                "L. 111-1",
                Some("Partie législative > Livre Ier : Le Conseil d'Etat"),
            ),
            ("R. 111-1", Some("Partie réglementaire > Livre Ier")),
        ];
        sort_reading_order(&mut entries, |e| e.0, |e| e.1);
        assert_eq!(
            entries.iter().map(|e| e.0).collect::<Vec<_>>(),
            vec![
                "L. 1",
                "L. 111-1",
                "L. 511-1",
                "L. 77-10-3",
                "L. 711-1",
                "R. 111-1"
            ]
        );
    }

    #[test]
    fn text_reading_order_prefers_source_positions() {
        let mut entries = vec![
            ("L. 77-10-3", Some("Livre VII"), None),
            ("L. 111-1", Some("Livre Ier"), Some(2)),
            ("L. 1", Some("Titre préliminaire"), Some(1)),
        ];
        sort_text_reading_order(&mut entries, |e| e.0, |e| e.1, |e| e.2);
        assert_eq!(
            entries.iter().map(|e| e.0).collect::<Vec<_>>(),
            vec!["L. 1", "L. 111-1", "L. 77-10-3"]
        );
    }

    #[test]
    fn text_reading_order_falls_back_to_medians_without_positions() {
        let mut entries = vec![
            ("L. 77-10-3", Some("Partie législative > Livre VII"), None),
            ("L. 111-1", Some("Partie législative > Livre Ier"), None),
            ("L. 711-1", Some("Partie législative > Livre VII"), None),
        ];
        sort_text_reading_order(&mut entries, |e| e.0, |e| e.1, |e| e.2);
        assert_eq!(
            entries.iter().map(|e| e.0).collect::<Vec<_>>(),
            vec!["L. 111-1", "L. 77-10-3", "L. 711-1"]
        );
    }

    #[test]
    fn starred_prefixes_keep_their_part() {
        let (rank, _, _) = num_key_sort_key("R*. 011");
        assert_eq!(rank, 3);
    }
}
