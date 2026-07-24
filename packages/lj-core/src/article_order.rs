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
/// divisions (préfixes du `title_path`, segments « > ») sont ordonnées par la
/// clé **médiane** de leurs articles, les articles d'une division par leur
/// propre clé. La médiane rend l'ordre robuste aux numérotations aberrantes —
/// dans le CJA, les articles « L. 77-10-… » du Livre VII trient numériquement
/// avant « L. 111-1 », et un ordre au premier-arrivé (ou au min) plaçait le
/// Livre VII avant le Livre Ier.
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // CJA réel : « L. 77-10-3 » (Livre VII) trie numériquement avant
        // « L. 111-1 » (Livre Ier) — la médiane du Livre VII (~7xx) le remet à
        // sa place, après les Livres I à VI.
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
    fn starred_prefixes_keep_their_part() {
        // « R*. 011 » : le préfixe alphabétique est « R », l'étoile est ignorée
        // par le rang — l'article reste dans la partie réglementaire.
        let (rank, _, _) = num_key_sort_key("R*. 011");
        assert_eq!(rank, 3);
    }
}
