//! Files under the Union what the international pile carried of it.

use anyhow::{anyhow, Result};
use lj_fold::fold;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// A text is filed under the most specific authority that enacts it. The Union
/// enacts its primary law and what amends it, and its member states enact, on
/// the basis of its treaties, what they sign among themselves. An agreement a
/// third party signs stays international, the Union being one party among
/// several.
fn enacted_by_the_union(title: &str) -> bool {
    const MARKERS: [&str; 5] = [
        "traite sur l union europeenne",
        "traite sur le fonctionnement de l union europeenne",
        "etats membres de l union europeenne",
        "systeme des ressources propres",
        "privileges et immunites et sur le statut de la cour",
    ];
    let words = words_only(title);
    MARKERS.iter().any(|m| words.contains(m))
}

/// The title cut down to its words, lowercase and unaccented: punctuation and
/// case vary from one publication to the next on the same instrument.
fn words_only(title: &str) -> String {
    fold(title)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Command `refile-union-texts`. Replays as a no-op once the pile is sorted.
pub async fn refile_union_texts() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let pile = repo
        .international_texts()
        .await
        .map_err(|e| anyhow!("international_texts: {e}"))?;
    let read = pile.len();
    let mut filed = 0u64;
    for (text_uid, title) in pile {
        if !enacted_by_the_union(&title) {
            continue;
        }
        repo.set_text_jurisdiction(&text_uid, "EU")
            .await
            .map_err(|e| anyhow!("set_text_jurisdiction({text_uid}): {e}"))?;
        filed += 1;
    }
    tracing::info!(read, filed, "Union texts moved out of the INTL stack");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_union_takes_its_own_law_and_leaves_the_treaties_it_merely_signs() {
        for taken in [
            "Traité sur l'Union européenne, signé à Maastricht le 7 février 1992",
            "Traité de Lisbonne modifiant le traité sur l'Union européenne et le traité instituant la Communauté européenne",
            "Convention établie sur la base de l'article K. 3 du traité sur l'Union européenne, relative à l'extradition",
            "Convention établie par le Conseil conformément à l'article 34 du traité sur l'Union européenne",
            "Accord entre les Etats membres de l'Union européenne relatif au statut du personnel militaire",
            "Protocole modifiant le protocole sur les dispositions transitoires annexé au traité sur l'Union européenne",
            "Décision du Conseil du 26 mai 2014 relative au système des ressources propres de l'Union européenne",
            "PROTOCOLES SUR LES PRIVILEGES ET IMMUNITES ET SUR LE STATUT DE LA COUR DE JUSTICE",
        ] {
            assert!(enacted_by_the_union(taken), "missed: {taken}");
        }
        for left in [
            "Accord-cadre entre l'Union européenne et ses Etats membres, d'une part, et la République de Corée, d'autre part",
            "Convention de sauvegarde des droits de l'homme et des libertés fondamentales",
            "ACCORD CREANT UNE ASSOCIATION ENTRE LA COMMUNAUTE ECONOMIQUE EUROPEENNE ET LA TURQUIE",
            "Accord entre le Gouvernement de la République française et le Gouvernement de la République algérienne",
        ] {
            assert!(!enacted_by_the_union(left), "taken in error: {left}");
        }
    }
}
