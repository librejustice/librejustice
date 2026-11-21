use crate::i18n::NormUi;

pub const NORM: NormUi = NormUi {
    article_n,
    article_of,
    art_short,
    article_status,
    read_from: "Lu depuis",
    not_found_title: "Article introuvable - LibreJustice",
    no_version_on,
    unknown_address: "Cette adresse ne désigne ni un lieu ni une date que nous connaissons",
    see_in_force: "Voir la version en vigueur",
    gap_explained,
    gap_unexplained,
    version_before,
    version_after,
    in_force_on,
    wording_of,
    since,
    article_expired: "Cet article n'est plus en vigueur",
    version_expired: "Cette version n'est plus en vigueur",
    in_force: "En vigueur",
    will_enter_into_force: "Cet article entrera en vigueur",
    will_be_modified: "Cet article sera modifié",
    neighbours: "Articles voisins",
    article_versions: "Versions de l'article",
    back_to_in_force: "Revenir à la version en vigueur",
    on_date: "À la date du",
    source_legi: "Base LEGI · DILA",
    source_jorf: "Journal officiel · DILA",
    same_chapter: "Dans le même chapitre",
    text_unavailable: "Texte indisponible.",
    unofficial_translation: "Traduction non officielle",
    automatic_translation: "Traduction automatique, non vérifiée",
    original_text: "Texte original",
    original_text_arabic: "Texte original (arabe)",
    original_text_in,
    notes: "Notes",
    modifies: "Modifie",
    creates: "Crée",
    repeals: "Abroge",
    preparatory_work: ", travaux préparatoires sur Légifrance",
    parliamentary_work,
    places_and_more,
    at_this_date: "à cette date",
    today: "aujourd'hui",
    read_from_place,
    not_applicable_frozen,
    not_applicable,
    no_mention,
    frozen,
    outside_window,
    applies_elsewhere: " Où il s'applique : ",
    by_rule,
    applies_in,
    since_clause,
    start_undated: " (début non daté par la source)",
    stops_on,
    end_computed: " (fin déduite de l'état suivant)",
    by_source,
    competing,
    applies_in_short,
    also_applies,
    metropole: "Métropole",
    overseas_identity: "Outre-mer, identité législative",
    overseas_specialty: "Outre-mer, spécialité législative",
    other_states: "Autres États",
    other_version: "autre version",
    filter_places: "Filtrer (ex. Mayotte…)",
    no_place_matches: "Aucun lieu ne correspond.",
    undetermined_date: "à une date à déterminer",
    on_day,
    in_force_from_to,
    in_force_since,
    compare_versions: "Comparer les versions",
    versions: "Versions",
    initial_version: "Version initiale",
    upcoming: "à venir",
    by: "par ",
    related_articles: "Articles liés",
    often_cited_with: "Souvent cité avec",
    cited_together_in,
    copy_reference: "Copier la référence",
    most_cited: "Articles les plus cités",
    cited_by: "Cité par",
    this_article: "cet article",
    this_division: "cette division",
    cites_division_articles,
    loading_decisions: "Chargement des décisions…",
    citing_unavailable,
    no_citing_decision,
    see_all_citing,
    cites_art_one,
    cites_art_list,
    cites_art_more,
    invalid_article: "Référence d'article invalide",
    invalid_section: "Référence de section invalide",
    nothing_at_address: "Aucun article ni aucune division à cette adresse",
    invalid_code: "Code invalide",
    norms: "Normes",
    code_not_found_title: "Code introuvable - LibreJustice",
    code_description,
    year_n,
    undated: "sans date",
    data_sources: "Données & sources",
    n_articles,
    last_modified_on,
    signed_on,
    published_in,
    journal_name: |label| label,
    nor,
    jo_text: "Texte au JO · Légifrance",
    text_will_change: "Ce texte sera modifié",
    text_repealed: "Ce texte est abrogé.",
    text_heading: "Texte",
    pub_notice: "Notice",
    pub_summary: "Résumé",
    pub_visas: "Visas",
    pub_text: "Texte publié",
    pub_signatories: "Signataires",
    pub_preparatory: "Travaux préparatoires",
    search_in_code: "Rechercher dans ce code",
    search_in_code_placeholder: "Rechercher dans ce code…",
    loading_toc: "Chargement du sommaire…",
    toc_unavailable,
    full_text: "Texte intégral",
    toc: "Table des matières",
    text_plan: "Plan du texte",
    n_art,
    read: "Lire",
    loading: "Chargement…",
    contents: "Sommaire",
    section_not_found_title: "Section introuvable - LibreJustice",
    section_description,
    section_count,
    show_repealed: "Afficher les articles abrogés",
    hide_repealed: "Masquer les articles abrogés",
    related_articles_divisions: "Articles et divisions liés",
    in_this_division: "Dans cette division",
    previous_block: "← Bloc précédent",
    next_block: "Bloc suivant →",
    neighbour_divisions: "Divisions voisines",
    nota: "Nota",
    compare_not_found_title: "Comparaison introuvable - LibreJustice",
    change_n,
    reference_version: "Version de référence",
    absent_paragraph: "Alinéa absent de cette version",
    compared_version: "Version comparée",
    deleted_paragraph: "Alinéa supprimé",
    compare_title,
    identical: "Les deux rédactions sont identiques, mot pour mot.",
    n_changes,
    go_to_first: "Aller à la première",
    back_to_article,
    comparator: "Comparateur de versions",
    reference: "Référence",
    compared: "Comparée",
    removed: "retiré",
    removed_from: " de la version de référence",
    added: "ajouté",
    added_in: " dans la version comparée",
    notice_description_on,
    notice_description,
    fact_published: "Parution",
    fact_text_date: "Date du texte",
    fact_nature: "Nature",
    fact_issuer: "Émetteur",
    in_force_text: "Le texte en vigueur : ",
    see_official: "Voir la publication officielle",
    action_verb,
    amends,
};

fn article_n(num: &str) -> String {
    format!("Article {num}")
}

fn article_of(num: &str, text: &str) -> String {
    match lj_dtos::instrument_with_de(text) {
        Some(of_text) => format!("Article {num} {of_text}"),
        None => format!("Article {num}, {text}"),
    }
}

fn art_short(num: &str) -> String {
    format!("art. {num}")
}

fn no_version_on(day: &str) -> String {
    format!("Aucune version de cet article au {day}")
}

fn gap_explained(status: &str, closed: &str, resumed: &str) -> String {
    format!(
        "{status} le {closed}, l'article est à nouveau présent le {resumed} : il n'existait pas sous ce numéro à la date demandée."
    )
}

fn gap_unexplained(closed: &str, resumed: &str) -> String {
    format!(
        "L'article court de part et d'autre de cette date, mais nous ne détenons aucune version entre le {closed} et le {resumed}."
    )
}

fn version_before(day: &str) -> String {
    format!("Version qui précède, du {day}")
}

fn version_after(day: &str) -> String {
    format!("Version qui suit, du {day}")
}

fn in_force_on(day: &str) -> String {
    format!("Rédaction en vigueur le {day}")
}

fn wording_of(day: &str) -> String {
    format!("Rédaction du {day}")
}

fn since(day: &str) -> String {
    format!("depuis le {day}")
}

fn original_text_in(lang: &str) -> String {
    format!("Texte original ({lang})")
}

fn parliamentary_work(n: usize) -> String {
    format!("Travaux parlementaires · {n}")
}

fn places_and_more(places: &str, more: usize) -> String {
    format!("{places} et {more} autres lieux")
}

fn read_from_place(place: &str) -> String {
    format!("Lu {place} : ")
}

fn not_applicable_frozen(lead: &str, day: &str) -> String {
    format!(
        "{lead}cet article ne s'y applique pas : il n'était pas en vigueur le {day}, jour où la matière est passée à la collectivité."
    )
}

fn not_applicable(lead: &str, when: &str) -> String {
    format!("{lead}cet article ne s'y applique pas {when}.")
}

fn no_mention(lead: &str) -> String {
    format!(
        "{lead}une loi ne s'y applique que si un texte le dit, et aucune mention qui étende cet article n'a été relevée."
    )
}

fn frozen(lead: &str, day: &str) -> String {
    format!(
        "{lead}la matière relève de la collectivité depuis le {day}. Le texte national y reste dans sa rédaction de cette date tant que la collectivité ne l'a pas remplacé : voici cette rédaction."
    )
}

fn outside_window(place: &str, when: &str) -> String {
    format!("Aucun état connu de cet article ne s'applique {place} {when}.")
}

fn by_rule(rule: &str) -> String {
    format!("Selon la règle : « {rule} »")
}

fn applies_in(lead: &str, places: &str) -> String {
    format!("{lead}S'applique {places}")
}

fn since_clause(day: &str) -> String {
    format!(" depuis le {day}")
}

fn stops_on(day: &str) -> String {
    format!(", cesse de s'appliquer le {day}")
}

fn by_source(clause: &str) -> String {
    format!("Selon la source : « {clause} »")
}

fn competing(place: &str, when: &str) -> String {
    format!(
        "Plusieurs états s'appliquent {place} {when} et la source ne les départage pas. S'applique aussi, dans une autre version : "
    )
}

fn applies_in_short(place: &str) -> String {
    format!("s'applique {place}")
}

fn also_applies(n: usize) -> String {
    match n {
        1 => "s'applique aussi dans 1 autre lieu".to_string(),
        n => format!("s'applique aussi dans {n} autres lieux"),
    }
}

fn on_day(day: &str) -> String {
    format!("le {day}")
}

fn in_force_from_to(start: &str, end: &str) -> String {
    format!("En vigueur du {start} au {end}")
}

fn in_force_since(start: &str) -> String {
    format!("En vigueur depuis le {start}")
}

fn cited_together_in(decisions: &str) -> String {
    format!("Cités ensemble dans {decisions}")
}

fn cites_division_articles(n: i64) -> String {
    match n {
        1 => "cite 1 article de la division".to_string(),
        n => format!("cite {n} articles de la division"),
    }
}

fn citing_unavailable(error: &str) -> String {
    format!("Décisions citantes indisponibles ({error}).")
}

fn no_citing_decision(subject: &str) -> String {
    format!("Aucune décision citant {subject} n'a encore été identifiée.")
}

fn see_all_citing(subject: &str) -> String {
    format!("Voir toutes les décisions citant {subject}")
}

fn cites_art_one(num: &str) -> String {
    format!("cite l'art. {num}")
}

fn cites_art_list(first: &str, last: &str) -> String {
    format!("cite les art. {first} et {last}")
}

fn cites_art_more(named: &str, more: usize) -> String {
    format!("cite les art. {named} et {more} autres")
}

fn code_description(title: &str, articles: i64) -> String {
    format!("{title} : {articles} articles. Référentiel consolidé, versions à date.")
}

fn year_n(year: i32) -> String {
    format!("année {year}")
}

fn n_articles(n: &str) -> String {
    format!("{n} articles")
}

fn last_modified_on(day: &str) -> String {
    format!("Dernière modification le {day}")
}

fn signed_on(day: &str) -> String {
    format!("Date de signature : {day}")
}

fn published_in(issue: &str) -> String {
    format!("Publication : {issue}")
}

fn nor(nor: &str) -> String {
    format!("NOR : {nor}")
}

fn toc_unavailable(error: &str) -> String {
    format!("Sommaire indisponible ({error}).")
}

fn n_art(n: &str) -> String {
    format!("{n} art.")
}

fn section_description(title: &str, text: &str) -> String {
    format!(
        "{title}, {text}. Lecture à la suite des articles de la division, texte consolidé en vigueur."
    )
}

fn section_count(in_force: usize, repealed: usize) -> String {
    match (in_force, repealed) {
        (1, 0) => "1 article en vigueur".to_string(),
        (n, 0) => format!("{n} articles en vigueur"),
        (n, 1) => format!("{n} en vigueur, 1 abrogé"),
        (n, m) => format!("{n} en vigueur, {m} abrogés"),
    }
}

fn change_n(n: usize) -> String {
    format!("Modification {n}")
}

fn compare_title(num: &str, text: &str) -> String {
    format!("Versions de l'article {num} du {text} - LibreJustice")
}

fn n_changes(n: usize) -> String {
    let plural = if n > 1 { "s" } else { "" };
    format!("{n} modification{plural}")
}

fn back_to_article(num: &str, text: &str) -> String {
    format!("← Article {num} du {text}")
}

fn notice_description_on(title: &str, journal: &str, day: &str) -> String {
    format!("{title}, publié au {journal} le {day}.")
}

fn notice_description(title: &str, journal: &str) -> String {
    format!("{title}, publié au {journal}.")
}

fn action_verb(action: &str) -> Option<&'static str> {
    match action {
        "modifie" => Some("modifie"),
        "cree" => Some("crée"),
        "abroge" => Some("abroge"),
        _ => None,
    }
}

fn amends(title: &str, verbs: &str, codes: &str) -> String {
    format!("{title} : {verbs} des dispositions de {codes}.")
}

fn article_status(status: &str) -> Option<&'static str> {
    Some(match status {
        "VIGUEUR" => "En vigueur",
        "MODIFIE" => "Modifié",
        "ABROGE" => "Abrogé",
        "PERIME" => "Périmé",
        "REMPLACE" => "Remplacé",
        "TRANSFERE" => "Transféré",
        "DEPLACE" => "Déplacé",
        "ANNULE" => "Annulé",
        "DENONCE" => "Dénoncé",
        "DISJOINT" => "Disjoint",
        "VIGUEUR_DIFF" => "Entrée en vigueur différée",
        "ABROGE_DIFF" => "Abrogation différée",
        "MODIFIE_MORT_NE" => "Modifié, jamais entré en vigueur",
        _ => return None,
    })
}
