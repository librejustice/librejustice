//! Tests de la canonicalisation d'instruments (extraits de l'ancien
//! `extract/common.rs` lors du découpage thématique).

use super::*;

#[test]
fn normalize_instrument_strips_article_and_titlecases() {
    assert_eq!(normalize_instrument("le code civil"), "Code civil");
    assert_eq!(normalize_instrument("CODE CIVIL"), "Code civil");
}

#[test]
fn normalize_instrument_recovers_caps_lock_source() {
    // Vieilles décisions (Cassation) en CAPS : instruments non aliasés. La
    // recasse normale doit s'appliquer plutôt que de préserver les majuscules
    // comme des acronymes. Les acronymes réels restent restaurés.
    assert_eq!(
        normalize_instrument("CODE DE PROCÉDURE PÉNALE"),
        "Code de procédure pénale"
    );
    assert_eq!(
        normalize_instrument("DÉCLARATION DES DROITS DE L'HOMME ET DU CITOYEN"),
        "Déclaration des droits de l'homme et du citoyen"
    );
    // Acronyme standalone connu : préservé tel quel.
    assert_eq!(normalize_instrument("CESEDA"), "CESEDA");
    // Acronyme interne restauré par instrument_internal_case.
    assert_eq!(normalize_instrument("LOI ELAN"), "Loi ELAN");
}

#[test]
fn normalize_instrument_canonical_eu_civil_regulations() {
    // Surnom nu, numéro nu (sigle/« n° » manquants) et forme JO complète
    // convergent vers la même clé numérotée — sinon le même règlement éclate
    // en autant de graphies (missed côté GT, spurious côté extracteur).
    for v in [
        "Règlement Bruxelles I bis",
        "Règlement 1215/2012",
        "règlement (UE) n° 1215/2012",
        "Règlement (UE) n° 1215/2012 du Parlement européen et du Conseil du 12 décembre 2012",
    ] {
        assert_eq!(
            normalize_instrument(v),
            "Règlement (UE) n° 1215/2012",
            "{v}"
        );
    }
    assert_eq!(
        normalize_instrument("Règlement Rome II"),
        "Règlement (CE) n° 864/2007"
    );
    assert_eq!(
        normalize_instrument("Règlement n° 864/2007"),
        "Règlement (CE) n° 864/2007"
    );
    assert_eq!(
        normalize_instrument("Règlement Rome I"),
        "Règlement (CE) n° 593/2008"
    );
    assert_eq!(
        normalize_instrument("Règlement Bruxelles I"),
        "Règlement (CE) n° 44/2001"
    );
    // Ordre des préfixes : les variantes spécifiques ne sont pas avalées par
    // leur générique.
    assert_eq!(
        normalize_instrument("Règlement Rome III"),
        "Règlement (UE) n° 1259/2010"
    );
    assert_eq!(
        normalize_instrument("Règlement Bruxelles II bis"),
        "Règlement (CE) n° 2201/2003"
    );
    // Idempotence : une clé déjà canonique se renvoie elle-même.
    assert_eq!(
        normalize_instrument("Règlement (CE) n° 864/2007"),
        "Règlement (CE) n° 864/2007"
    );
    // La Convention de Rome de 1980 (≠ règlement Rome I) n'est pas happée par le
    // bloc « Règlement » : elle converge vers son titre long de Convention (loi
    // applicable aux obligations contractuelles), pas vers un règlement.
    assert_eq!(
        normalize_instrument("Convention de Rome du 19 juin 1980"),
        "Convention de Rome du 19 juin 1980 sur la loi applicable aux obligations contractuelles"
    );
}

#[test]
fn normalize_instrument_canonical_foreign_and_conventions() {
    // Code des obligations suisse : ordre des mots + titre fédéral verbeux.
    for v in [
        "Code des obligations suisse",
        "Code suisse des obligations",
        "Loi fédérale complétant le code civil suisse (code des obligations suisse)",
        "Loi fédérale formant le code des obligations",
        // Nu (anaphore : « suisse » posé en amont) ou pluriel — la France n'a
        // aucun « Code des obligations ».
        "Code des obligations",
        "Code des obligations suisses",
    ] {
        assert_eq!(
            normalize_instrument(v),
            "Code des obligations suisse",
            "{v}"
        );
    }
    // COC tunisien/marocain : « Code des obligations ET DES CONTRATS » est un
    // code DISTINCT, ne doit pas être conflé avec le CO suisse.
    assert_ne!(
        normalize_instrument("Code des obligations et des contrats"),
        "Code des obligations suisse"
    );
    // CVIM : variantes ONU/Vienne/CVIM/date convergent ; ancrage marchandises.
    let cvim = "Convention de Vienne du 11 avril 1980 sur les contrats de vente internationale de marchandises";
    for v in [
        "Convention des Nations unies sur les contrats de vente internationale de marchandises (CVIM) du 11 avril 1980",
        "Convention de Vienne du 11 avril 1980 sur les contrats de vente internationale de marchandises",
        "Convention de Vienne (CVIM)",
    ] {
        assert_eq!(normalize_instrument(v), cvim, "{v}");
    }
    // CVIM : la date seule (sans sous-titre marchandises) suffit à converger —
    // « 11 avril 1980 » n'identifie qu'elle parmi les conventions de Vienne.
    assert_eq!(
        normalize_instrument("Convention de Vienne du 11 avril 1980"),
        cvim
    );
    // Lugano : toute graphie → forme datée en vigueur.
    assert_eq!(
        normalize_instrument("Convention de lugano du 30 octobre 2007"),
        "Convention de Lugano du 30 octobre 2007"
    );
    // Rome (obligations contractuelles) : formes courtes datées/sous-titrées
    // convergent vers le titre long de la GT ; le Statut de Rome (CPI) est exclu.
    let rome =
        "Convention de Rome du 19 juin 1980 sur la loi applicable aux obligations contractuelles";
    for v in [
        "Convention de Rome du 19 juin 1980",
        "Convention de Rome sur la loi applicable aux obligations contractuelles",
        "Convention de Rome de 1980 sur la loi applicable aux obligations contractuelles",
    ] {
        assert_eq!(normalize_instrument(v), rome, "{v}");
    }
    assert_ne!(
        normalize_instrument("Statut de Rome de la Cour pénale internationale"),
        rome
    );
    // La Haye 4 mai 1971 (accidents de la circulation) : forme courte datée → titre long.
    assert_eq!(
        normalize_instrument("Convention de la haye du 4 mai 1971"),
        "Convention de la haye du 4 mai 1971 sur la loi applicable en matière d'accidents de la circulation routière"
    );
    // Rome : forme NUE (anaphore) converge aussi.
    assert_eq!(normalize_instrument("Convention de Rome"), rome);
    // La Haye / Bruxelles : dates SANS ambiguïté → titre long.
    assert_eq!(
        normalize_instrument("Convention de la haye du 15 juin 1955"),
        "Convention de la haye du 15 juin 1955 sur la loi applicable aux ventes à caractère international d'objets mobiliers corporels"
    );
    assert_eq!(
        normalize_instrument("Convention de la haye du 25 octobre 1980"),
        "Convention de la haye du 25 octobre 1980 sur les aspects civils de l'enlèvement international d'enfants"
    );
    assert_eq!(
        normalize_instrument("Convention de BRUXELLES du 27 septembre 1968"),
        "Convention de Bruxelles du 27 septembre 1968 concernant la compétence judiciaire et l'exécution des décisions en matière civile et commerciale"
    );
    // Dates AMBIGUËS (deux conventions de La Haye) : NON rabattues par la date seule.
    assert_eq!(
        normalize_instrument("Convention de la haye du 2 octobre 1973"),
        "Convention de la haye du 2 octobre 1973"
    );
    assert_eq!(
        normalize_instrument("Convention de la haye du 14 mars 1978"),
        "Convention de la haye du 14 mars 1978"
    );
}

#[test]
fn normalize_instrument_strips_punct_glued_to_foreign_gentile() {
    // Aparté parenthésé « (… du Code civil espagnol) » : le `)` collé au gentilé
    // final ne doit pas casser l'identité — même canon que la forme nue.
    assert_eq!(
        normalize_instrument("Code civil espagnol)"),
        normalize_instrument("Code civil espagnol")
    );
    assert_eq!(
        normalize_instrument("Code civil suisse)"),
        normalize_instrument("Code civil suisse")
    );
    // Gentilé final propre : inchangé (pas de troncature parasite).
    assert_eq!(
        normalize_instrument("Code des obligations suisse"),
        normalize_instrument("Code des obligations suisse")
    );
}

#[test]
fn foreign_codes_do_not_collapse_onto_french_homonym() {
    // Garde droit étranger (ADR 0102 §B, étendue aux gentilés non-asile) : un
    // code étranger ne doit JAMAIS se replier sur le code FR homonyme — sinon le
    // linker confond le BGB allemand avec le code civil français. Le gentilé est
    // conservé ; l'identité reste distincte.
    for (raw, french) in [
        ("Code civil allemand", "Code civil"),
        (
            "Code civil allemand (Bürgerliches Gesetzbuch)",
            "Code civil",
        ),
        ("Code civil espagnol", "Code civil"),
        ("Code civil italien", "Code civil"),
        ("Code civil néerlandais", "Code civil"),
        ("Code civil suisse", "Code civil"),
        (
            "Code de procédure civile allemand",
            "Code de procédure civile",
        ),
        ("Code de commerce allemand", "Code de commerce"),
        ("Code pénal italien", "Code pénal"),
    ] {
        let out = normalize_instrument(raw);
        assert_ne!(out, french, "{raw} replié sur le code FR");
        assert!(
            out.to_lowercase() != french.to_lowercase(),
            "{raw} → {out} (gentilé perdu)"
        );
    }
}

#[test]
fn eu_regulations_resolved_by_adoption_date() {
    // L'extracteur capte souvent ces règlements « civils » par leur DATE
    // d'adoption (« règlement européen du 17 juin 2008 ») sans le numéro, ou
    // avec un numéro espacé (« 44/ 2001 ») que la canonisation par numéro rate.
    // La date d'adoption identifie le règlement → forme JO numérotée.
    for (raw, want) in [
        (
            "Règlement européen du 17 juin 2008",
            "Règlement (CE) n° 593/2008",
        ),
        (
            "Règlement du 17 juin 2008 sur la loi applicable aux obligations contractuelles",
            "Règlement (CE) n° 593/2008",
        ),
        (
            "Règlement (UE) du 12 décembre 2012",
            "Règlement (UE) n° 1215/2012",
        ),
        ("Règlement du 22 décembre 2000", "Règlement (CE) n° 44/2001"),
        (
            "Règlement CE n° 44/ 2001 du 22 décembre 2000",
            "Règlement (CE) n° 44/2001",
        ),
        (
            "Règlement du 27 novembre 2003",
            "Règlement (CE) n° 2201/2003",
        ),
        (
            "Règlement communautaire du 11 juillet 2007",
            "Règlement (CE) n° 864/2007",
        ),
    ] {
        assert_eq!(normalize_instrument(raw), want, "{raw}");
    }
    // Le numéro reste prioritaire sur la date quand les deux sont présents.
    assert_eq!(
        normalize_instrument("Règlement (CE) n° 593/2008 du 17 juin 2008"),
        "Règlement (CE) n° 593/2008"
    );
}

#[test]
fn german_native_code_names_converge_to_french_form() {
    // BGB / Bürgerliches Gesetzbuch (nom natif seul, natif + gloss, FR + gloss,
    // sigle) → forme française d'usage unique. Sans quoi le même code éclate en
    // une dizaine d'identités (recall ruiné).
    for v in [
        "Bürgerliches gesetzbuch",
        "Bürgerliches gesetzbuch (code civil allemand)",
        "Code civil allemand (bürgerliches gesetzbuch)",
        "Code civil allemand (BGB)",
        "BGB",
        "Code civil allemand",
    ] {
        assert_eq!(normalize_instrument(v), "Code civil allemand", "{v}");
    }
    assert_eq!(
        normalize_instrument("Handelsgesetzbuch (code de commerce allemand, HGB)"),
        "Code de commerce allemand"
    );
    assert_eq!(
        normalize_instrument("Strafgesetzbuch (code pénal allemand)"),
        "Code pénal allemand"
    );
    // EGBGB : Acte introductif du BGB (DIP allemand) — instrument DISTINCT, ne
    // doit PAS se replier sur le code civil allemand.
    let egbgb = normalize_instrument("Einführungsgesetz zum bürgerlichen gesetzbuche (EGBGB)");
    assert_ne!(egbgb, "Code civil allemand", "EGBGB conflé avec le BGB");
}

#[test]
fn foreign_code_overcapture_truncated_after_gentile() {
    // Sur-capture de la borne droite de citation : la prose qui suit le gentilé
    // est happée dans l'identité. Le code étranger n'étant pas snappé (garde
    // droit étranger), la queue survivait et éclatait l'identité (faux missed +
    // faux spurious sur le MÊME article). On tronque après le gentilé : toutes
    // ces graphies convergent vers le code étranger nu.
    for (raw, want) in [
        ("Code civil allemand à la suite", "Code civil allemand"),
        ("Code civil suisse agissant", "Code civil suisse"),
        (
            "Code civil suisse posant un principe général de bonne foi",
            "Code civil suisse",
        ),
        ("Code civil allemand (BGB), à", "Code civil allemand"),
        ("Code civil allemand -voir dire", "Code civil allemand"),
        (
            "Code civil espagnol relatif à la responsabilité",
            "Code civil espagnol",
        ),
        (
            "Code de commerce allemand (handelsgesetzbuch) dispose",
            "Code de commerce allemand",
        ),
        // Parenthèse-glose native happée : tronquée comme le reste (toutes les
        // graphies du même code étranger convergent vers la forme nue).
        (
            "Code civil allemand (bürgerliches gesetzbuch)",
            "Code civil allemand",
        ),
    ] {
        assert_eq!(normalize_instrument(raw), want, "{raw}");
    }
    // Le gentilé déjà en dernier token : rien à couper, identité intacte.
    assert_eq!(
        normalize_instrument("Code civil allemand"),
        "Code civil allemand"
    );
    assert_eq!(
        normalize_instrument("Code de procédure civile allemand"),
        "Code de procédure civile allemand"
    );
}

#[test]
fn normalize_instrument_canonical_cesdh() {
    let out = normalize_instrument("convention européenne de sauvegarde des droits de l'homme");
    assert_eq!(
        out,
        "Convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales"
    );
}

#[test]
fn normalize_instrument_cleans_truncation_tails() {
    // Connecteur pendant laissé par la borne droite de citation.
    assert_eq!(
        normalize_instrument("Décret du 8 janvier 1995 et"),
        "Décret du 8 janvier 1995"
    );
    assert_eq!(
        normalize_instrument("Arrêté du 20 décembre 2002 relatif"),
        "Arrêté du 20 décembre 2002"
    );
    assert_eq!(
        normalize_instrument("Convention collective applicable"),
        "Convention collective"
    );
    // Queue verbale capturée depuis le corps.
    assert_eq!(
        normalize_instrument(
            "Livre des procédures fiscales et ont notifié à chaque prévenu un procès-verbal"
        ),
        "Livre des procédures fiscales"
    );
    // Connecteur « et » légitime dans un vrai titre : intact.
    assert_eq!(
        normalize_instrument("Code monétaire et financier"),
        "Code monétaire et financier"
    );
    // Forme sans connecteur → titre Légifrance.
    assert_eq!(
        normalize_instrument("Code procédure civile"),
        "Code de procédure civile"
    );
    assert_eq!(normalize_instrument("Code commerce"), "Code de commerce");
}

#[test]
fn normalize_instrument_strips_orphan_trailing_paren() {
    // « (article 52 § 1 du règlement) » capte l'instrument « règlement) » : la
    // parenthèse fermante ORPHELINE est retirée (artefact ADR 0137, ≈28 K arêtes).
    assert_eq!(
        normalize_instrument("règlement)"),
        normalize_instrument("règlement")
    );
    assert_eq!(normalize_instrument("code)"), normalize_instrument("code"));
    assert_eq!(
        normalize_instrument("Règlement de la cour)"),
        normalize_instrument("Règlement de la cour")
    );
    // Aucune sortie ne conserve une ')' orpheline finale.
    for s in [
        "règlement)",
        "code)",
        "Règlement de la cour)",
        "ordonnance))",
    ] {
        assert!(!normalize_instrument(s).ends_with(')'), "{s}");
    }
    // Paire ÉQUILIBRÉE d'un vrai titre UE : préservée, pas rabotée.
    let ue = normalize_instrument("Règlement (CE) n° 44/2001");
    assert!(ue.contains("(CE)"), "paire équilibrée perdue : {ue}");
}

#[test]
fn normalize_instrument_trims_glued_prose_tail() {
    // Verbe nu collé au nom : coupé, identité préservée.
    assert_eq!(
        normalize_instrument("Loi du 11 mars 1957 définit la représentation comme consistant"),
        "Loi du 11 mars 1957",
    );
    // Tail coupé puis snap au titre Légifrance officiel.
    assert_eq!(
        normalize_instrument("Code de l'expropriation dispose"),
        "Code de l'expropriation pour cause d'utilité publique",
    );
    assert_eq!(
        normalize_instrument("Arrêté du 16 octobre 1995 dispose"),
        "Arrêté du 16 octobre 1995",
    );
    assert_eq!(
        normalize_instrument("Loi du 12 juillet 1990 prises"),
        "Loi du 12 juillet 1990",
    );
    // « et a <verbe> / et les articles » glué : coupé (auxiliaire/citation,
    // jamais dans un titre). On NE coupe PAS « et le <nom> » (cf. test de
    // non-fusion ci-dessous : appartient à de vrais titres).
    assert_eq!(
        normalize_instrument("Loi du 29 juillet 1881 et a omis d'annuler d'office les jugements"),
        "Loi du 29 juillet 1881",
    );
    assert_eq!(
        normalize_instrument("Code de commerce et les articles L"),
        "Code de commerce",
    );
    // Sous-titre boilerplate d'un Arrêté daté : coupé (extension Loi/Décret).
    assert_eq!(
        normalize_instrument("Arrêté du 27 décembre 2016 relatif aux conditions"),
        "Arrêté du 27 décembre 2016",
    );
    assert_eq!(
        normalize_instrument("Arrêté du 18 juin 1991 modifié relatif à la mise en place"),
        "Arrêté du 18 juin 1991",
    );
    // Vrais titres avec « et » : intacts (pas de verbe nu).
    assert_eq!(
        normalize_instrument("Code monétaire et financier"),
        "Code monétaire et financier",
    );
    assert_eq!(
        normalize_instrument("Code de la construction et de l'habitation"),
        "Code de la construction et de l'habitation",
    );
    // « relative aux droits… » SANS tête datée : PAS un boilerplate → intact.
    assert_eq!(
        normalize_instrument("Convention internationale relative aux droits de l'enfant"),
        "Convention internationale relative aux droits de l'enfant",
    );
}

#[test]
fn normalize_instrument_trims_year_glued_prose() {
    // Prose soudée SANS espace à un millésime (sur-capture borne droite,
    // observée en prod : « Loi du 6 juillet 1989ordonner l'expulsion… »,
    // « Décret du 28 décembre 2020portant application… »). Tous les
    // prose-cuts ancrés sur `\s+` la ratent. Coupe à la lettre collée.
    assert_eq!(
        normalize_instrument(
            "Loi du 6 juillet 1989ordonner l'expulsion des preneurs et de tout occupant"
        ),
        "Loi du 6 juillet 1989",
    );
    assert_eq!(
        normalize_instrument("Loi du 31 mai 1990visant la mise en œuvre du droit au logement"),
        "Loi du 31 mai 1990",
    );
    // Le millésime nu (cas nominal) reste intact : pas de lettre collée.
    assert_eq!(
        normalize_instrument("Loi du 6 juillet 1989"),
        "Loi du 6 juillet 1989",
    );
    // Numéro UE suivi d'un « / » (jamais d'une lettre) : intact.
    assert_eq!(
        normalize_instrument("Directive 2013/33/UE"),
        "Directive 2013/33/UE"
    );
}

#[test]
fn normalize_instrument_strips_citation_apparatus() {
    // « modifié » final, avec ou sans virgule.
    assert_eq!(
        normalize_instrument("Loi du 6 juillet 1989 modifiée"),
        "Loi du 6 juillet 1989"
    );
    assert_eq!(
        normalize_instrument("Accord franco-tunisien du 17 mars 1988 modifié"),
        "Accord franco-tunisien du 17 mars 1988"
    );
    // « modifié du <date> » : le mot saute, la date-identité reste.
    assert_eq!(
        normalize_instrument("Accord franco-marocain modifié du 9 octobre 1987"),
        "Accord franco-marocain du 9 octobre 1987"
    );
    assert_eq!(
        normalize_instrument("Loi modifiée du 31 décembre 1971"),
        "Loi du 31 décembre 1971"
    );
    // « modifié par / notamment » : queue d'apparat coupée.
    assert_eq!(
        normalize_instrument("Loi n° 78-17 du 6 janvier 1978, modifiée notamment"),
        "Loi du 6 janvier 1978"
    );
    // « devenu … » : renvoi, l'identité citée est la tête.
    assert_eq!(
        normalize_instrument("Loi du 25 janvier 1985 devenu l'article L"),
        "Loi du 25 janvier 1985"
    );
    assert_eq!(
            normalize_instrument(
                "Traité instituant la Communauté économique européenne, devenu le traité sur le fonctionnement de l'Union européenne"
            ),
            "Traité instituant la Communauté économique européenne"
        );
    // « ensemble <déterminant> » : jonction de visa coupée.
    assert_eq!(
        normalize_instrument(
            "Livre des procédures fiscales ensemble l'article 605 du code de procédure pénale"
        ),
        "Livre des procédures fiscales"
    );
    assert_eq!(
        normalize_instrument(
            "Loi n° 52-1311 du 10 décembre 1952, ensemble le statut du personnel administratif"
        ),
        "Loi du 10 décembre 1952"
    );
    // « ensemble » nom commun (pas de déterminant derrière) : intact.
    assert_eq!(
        normalize_instrument("Convention collective d'un ensemble immobilier"),
        "Convention collective d'un ensemble immobilier"
    );
    // « alors » nu en fin / « alors en vigueur ».
    assert_eq!(
        normalize_instrument("Décret du 22 décembre 1958 alors"),
        "Décret du 22 décembre 1958"
    );
    assert_eq!(
        normalize_instrument("Loi n° 84-53 du 26 janvier 1984, alors en vigueur"),
        "Loi du 26 janvier 1984"
    );
    // « visé ci-dessus » et « visé » final.
    assert_eq!(
        normalize_instrument("Loi du 11 juillet 1990 visée ci-dessus"),
        "Loi du 11 juillet 1990"
    );
    // Forme courte sans date : rabattue sur le canonique daté (ADR 0112 §6).
    assert_eq!(
        normalize_instrument("Accord franco-tunisien visé"),
        "Accord franco-tunisien du 17 mars 1988"
    );
    // « visée à l'article … » appartient à de vrais titres : intact.
    assert_eq!(
        normalize_instrument(
            "Arrêté du 9 janvier 2017 fixant la liste des pays sûrs visée à l'article L. 722-1"
        ),
        "Arrêté du 9 janvier 2017"
    );
}

#[test]
fn normalize_instrument_date_and_number_graphies() {
    // Jour zéro-paddé.
    assert_eq!(
        normalize_instrument("Loi du 06 juillet 1989 applicable à l'espèce"),
        "Loi du 6 juillet 1989"
    );
    // Indicateur ordinal º au lieu du degré °.
    assert_eq!(
        normalize_instrument("Loi nº 78-17 du 6 janvier 1978"),
        "Loi du 6 janvier 1978"
    );
    // Date tout-numérique dépliée en littéral.
    assert_eq!(
        normalize_instrument("Loi du 5/07/1985 à la charge de l'un"),
        "Loi du 5 juillet 1985"
    );
    assert_eq!(
        normalize_instrument("Arrêté du 18.12.2023"),
        "Arrêté du 18 décembre 2023"
    );
    // Mois hors plage : graphie intacte.
    assert_eq!(
        normalize_instrument("Décision du 5/13/2020"),
        "Décision du 5/13/2020"
    );
    // Tiret de numéro détaché.
    assert_eq!(
        normalize_instrument("Loi n° 2018 -1021 du 23 novembre 2018"),
        "Loi du 23 novembre 2018"
    );
}

#[test]
fn normalize_instrument_cuts_verbal_prose_markers() {
    assert_eq!(
        normalize_instrument("Code de procédure civile seront supportés in solidum"),
        "Code de procédure civile"
    );
    assert_eq!(
            normalize_instrument(
                "Charte des droits de l'Union européenne a été méconnu et présente une nouvelle conclusion"
            ),
            "Charte des droits de l'Union européenne"
        );
    assert_eq!(
        normalize_instrument("Convention d'habilitation individuelle dès lors"),
        "Convention d'habilitation individuelle"
    );
    assert_eq!(
            normalize_instrument(
                "Convention d'application de l'accord Schengen lors de son arrivée sur le territoire français"
            ),
            "Convention d'application de l'accord Schengen"
        );
    // « en cas de » vit dans de vrais titres : INTACT (et Accord est une
    // famille conventionnelle, hors coupe post-date).
    assert_eq!(
            normalize_instrument(
                "Accord du 29 mars 1990 fixant les conditions d'une garantie d'emploi en cas de changement de prestataire"
            ),
            "Accord du 29 mars 1990 fixant les conditions d'une garantie d'emploi en cas de changement de prestataire"
        );
    assert_eq!(
            normalize_instrument(
                "Convention collective nationale des entreprises de propreté garantie d'emploi en cas de changement de prestataire"
            ),
            "Convention collective nationale des entreprises de propreté garantie d'emploi en cas de changement de prestataire"
        );
}

#[test]
fn normalize_instrument_dated_head_keeps_adjective_and_pays() {
    assert_eq!(
        normalize_instrument("Arrêté préfectoral n° 2019-1067 du 29 avril 2019 sera exercée"),
        "Arrêté préfectoral du 29 avril 2019"
    );
    assert_eq!(
        normalize_instrument(
            "Arrêté interministériel du 13 avril 2022 modifiant l'arrêté du 10 avril 2020"
        ),
        "Arrêté interministériel du 13 avril 2022"
    );
    assert_eq!(
        normalize_instrument(
            "Loi du pays n° 2023-26 du 3 mars 2023 organisent les compétitions sportives"
        ),
        "Loi du pays du 3 mars 2023"
    );
}

#[test]
fn normalize_instrument_cuts_everything_after_complete_date() {
    // Sous-titre officiel : même cible que l'ancienne coupe à connecteurs.
    assert_eq!(
        normalize_instrument(
            "Loi n° 83-634 du 13 juillet 1983 portant droits et obligations des fonctionnaires"
        ),
        "Loi du 13 juillet 1983"
    );
    // Queue de prose à vocabulaire ouvert (hors de toute whitelist).
    assert_eq!(
        normalize_instrument("Loi du 31 mai 1990 et mentionne la faculté"),
        "Loi du 31 mai 1990"
    );
    assert_eq!(
        normalize_instrument("Loi du 18 novembre 2016, à peine d'irrecevabilité"),
        "Loi du 18 novembre 2016"
    );
    assert_eq!(
            normalize_instrument(
                "Décret du 11 décembre 2019 applicable aux instances introduites après le 1er janvier 2020"
            ),
            "Décret du 11 décembre 2019"
        );
    // Second instrument joint : la tête citée gagne.
    assert_eq!(
        normalize_instrument("Ordonnance du 14 mars 2016 et du décret n° 2016-884 du 29 juin 2016"),
        "Ordonnance du 14 mars 2016"
    );
    // « 1er » (raté par l'ancienne coupe : \\d+ puis espace obligatoire).
    assert_eq!(
        normalize_instrument("Loi du 1er juillet 1901 relative au contrat d'association"),
        "Loi du 1er juillet 1901"
    );
    // Date plurielle.
    assert_eq!(
        normalize_instrument("Loi des 16-24 août 1790 et le décret du 16 fructidor an III"),
        "Loi des 16-24 août 1790"
    );
    // Famille conventionnelle : l'objet fait partie du nom, INTACT.
    assert_eq!(
        normalize_instrument(
            "Convention de Genève du 28 juillet 1951 relative au statut des réfugiés"
        ),
        "Convention de Genève"
    );
    // Date incomplète (pas de millésime) : intact.
    assert_eq!(
        normalize_instrument("Décret du 2 thermidor an II"),
        "Décret du 2 thermidor an II"
    );
}

#[test]
fn normalize_instrument_collapses_dittography() {
    assert_eq!(
        normalize_instrument("Code de code de procédure civile"),
        "Code de procédure civile"
    );
    assert_eq!(
        normalize_instrument("Code du code du travail"),
        "Code du travail"
    );
    assert_eq!(
        normalize_instrument("Code de procédure de procédure civile"),
        "Code de procédure civile"
    );
    // Répétition NON adjacente d'un vrai titre : intacte.
    assert_eq!(
        normalize_instrument("Code de la construction et de l'habitation"),
        "Code de la construction et de l'habitation"
    );
}

#[test]
fn normalize_instrument_snaps_word_glued_prose() {
    assert_eq!(
        normalize_instrument("Code de procédure civileles dépens"),
        "Code de procédure civile"
    );
    assert_eq!(
        normalize_instrument(
            "Code des procédures civiles d'exécutionle sort des meubles sera régi"
        ),
        "Code des procédures civiles d'exécution"
    );
    assert_eq!(
        normalize_instrument("Livre des procédures fiscalesle contribuable"),
        "Livre des procédures fiscales"
    );
    // Extension réelle par ESPACE : titre officiel distinct, intact.
    assert_eq!(
        normalize_instrument("Code du travail maritime"),
        "Code du travail maritime"
    );
}

#[test]
fn normalize_instrument_cuts_glued_verb_and_procedural_tail() {
    // Corps de disposition avalé au titre du code (ADR 0137, ≈12 K arêtes) :
    // le verbe conjugué / la queue procédurale n'appartient jamais à un titre de
    // code ou de loi (syntagmes nominaux). On tronque au premier marqueur de prose.
    assert_eq!(
        normalize_instrument("Code de procédure civile mettent à la charge des parties"),
        "Code de procédure civile"
    );
    assert_eq!(
        normalize_instrument("Code de procédure civile il incombe à chaque partie"),
        "Code de procédure civile"
    );
    assert_eq!(
        normalize_instrument("Code de procédure civile reposait sur les articles"),
        "Code de procédure civile"
    );
    assert_eq!(
        normalize_instrument("Code de procédure civile et aux entiers dépens"),
        "Code de procédure civile"
    );
    assert_eq!(
        normalize_instrument("Code de procédure civile COMPOSITION DE LA COUR"),
        "Code de procédure civile"
    );
    // GARDE-FOU régression : un qualificatif de titre (étranger, local, adjectif)
    // ne doit JAMAIS être coupé — ce ne sont pas des marqueurs de prose.
    assert_eq!(
        normalize_instrument("Code de procédure civile allemand"),
        "Code de procédure civile allemand"
    );
    assert_eq!(
        normalize_instrument("Code monétaire et financier"),
        "Code monétaire et financier"
    );
    assert_eq!(
        normalize_instrument("Code de la construction et de l'habitation"),
        "Code de la construction et de l'habitation"
    );
}

#[test]
fn normalize_instrument_snaps_unaccented_ceseda() {
    const CESEDA: &str = "Code de l'entrée et du séjour des étrangers et du droit d'asile";
    assert_eq!(
        normalize_instrument(
            "Code de l'entrée et du séjour des etrangers et du droit de l'asile (CESEDA)"
        ),
        CESEDA
    );
    assert_eq!(
        normalize_instrument("Code de l'entrée et de séjour des etrangers et du droit d'asile"),
        CESEDA
    );
    // Dittographie + appareil de citation cumulés.
    assert_eq!(
        normalize_instrument(
            "Code de l'entrée de l'entrée et du séjour des étrangers et du droit d'asile modifié"
        ),
        CESEDA
    );
}

#[test]
fn normalize_instrument_canonical_cesdh_protocols() {
    const P1: &str = "Protocole n° 1 à la convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales";
    // « additionnel » sans numéro = protocole n° 1 (Paris, 20 mars 1952).
    assert_eq!(
            normalize_instrument(
                "Protocole additionnel à la convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales"
            ),
            P1
        );
    assert_eq!(
            normalize_instrument(
                "Protocole additionnel du 20 mars 1952, à la convention de sauvegarde des droits de l'homme et des libertés fondamentales"
            ),
            P1
        );
    assert_eq!(
            normalize_instrument(
                "Protocole n° 1 de la convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales"
            ),
            P1
        );
    assert_eq!(
        normalize_instrument(
            "Protocole additionnel n° 1 à la convention européenne des droits de l'homme"
        ),
        P1
    );
    assert_eq!(
            normalize_instrument(
                "Protocole n° 4 de la convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales"
            ),
            "Protocole n° 4 à la convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales"
        );
    // Protocoles NON-CESDH : intacts.
    assert_eq!(
        normalize_instrument("Protocole de la Haye du 23 novembre 2007"),
        // (« haye » minusculisé par la recasse existante — hors périmètre)
        "Protocole de la haye du 23 novembre 2007"
    );
}

#[test]
fn normalize_instrument_degraded_eu_formats() {
    // Tiret/espace au lieu de « / » dans le numéro UE (année première).
    assert_eq!(
        normalize_instrument(
            "Directive 2003-88 CE du Parlement européen et du Conseil du 4 novembre 2003"
        ),
        "Directive 2003/88/CE"
    );
    assert_eq!(
        normalize_instrument("Directive 1993-104 CE du Conseil du 23 novembre 1993"),
        "Directive 1993/104/CE"
    );
    // Interjection « dite retour » entre famille et numéro.
    assert_eq!(
        normalize_instrument(
            "Directive dite retour n° 2008/115/CE du Parlement européen et du Conseil"
        ),
        "Directive 2008/115/CE"
    );
    // Alias d'usage après une identité datée : coupé.
    assert_eq!(
        normalize_instrument("Règlement CE du 20 décembre 2010 dit Rome III"),
        "Règlement CE du 20 décembre 2010"
    );
    // Identité UE uniformisée : sigle nu, n° manquant, zéros de tête.
    assert_eq!(
        normalize_instrument("Règlement UE 2016/399 du Parlement européen et du Conseil du"),
        "Règlement (UE) n° 2016/399"
    );
    assert_eq!(
        normalize_instrument("Règlement (UE) 2018/1861"),
        "Règlement (UE) n° 2018/1861"
    );
    assert_eq!(
        normalize_instrument(
            "Règlement CE n° 0574/72 du Conseil du 21 mars 1972 fixant les modalités"
        ),
        "Règlement (CE) n° 574/72"
    );
    assert_eq!(
        normalize_instrument("Directive (UE) 2015/2366"),
        "Directive 2015/2366/UE"
    );
    assert_eq!(
        normalize_instrument("Directive CE 2003/88"),
        "Directive 2003/88/CE"
    );
    // Alias = seule identité (aucun chiffre en tête) : intact.
    // (« badinter » minusculisé par la recasse existante — hors périmètre)
    assert_eq!(
        normalize_instrument("Loi dite Badinter"),
        "Loi dite badinter"
    );
}

#[test]
fn normalize_instrument_keeps_distinct_instruments_distinct() {
    // Le trim de prose ne doit JAMAIS tronquer un vrai titre au point de
    // fusionner des instruments distincts (régression : « sur l'aide » et
    // « et le <nom> » réduisaient tout à « Loi » / au préfixe commun).
    assert_ne!(
        normalize_instrument("Loi sur l'aide juridictionnelle"),
        "Loi"
    );
    assert_ne!(
        normalize_instrument("Loi sur l'aide juridictionnelle"),
        normalize_instrument("Loi sur l'aide juridique"),
    );
    assert_ne!(
            normalize_instrument(
                "Convention entre le gouvernement de la République française et le gouvernement du Mali"
            ),
            normalize_instrument(
                "Convention entre le gouvernement de la République française et le gouvernement de Madagascar"
            ),
        );
    // Anaphore datée : identité résoluble -> pas jetée.
    assert!(!is_unresolvable_instrument(
        "Ordonnance précité du 25 mars 2020"
    ));
    // Anaphore nue (sans identité) : toujours jetée.
    assert!(is_unresolvable_instrument(
        "Décret précité sur l'absence d'imputabilité"
    ));
}

#[test]
fn normalize_instrument_standardizes_dated_number_to_date_form() {
    // Numéro présent sous diverses formes (n°, no ASCII, nu) -> forme datée unique.
    assert_eq!(
        normalize_instrument("Loi n° 65-557 du 10 juillet 1965"),
        normalize_instrument("Loi du 10 juillet 1965"),
    );
    assert_eq!(
        normalize_instrument("Ordonnance no58-1067 du 7 novembre 1958"),
        "Ordonnance du 7 novembre 1958",
    );
    assert_eq!(
        normalize_instrument("Loi 2004-575 du 21 juin 2004"),
        "Loi du 21 juin 2004",
    );
    assert_eq!(
        normalize_instrument("Décret n° 67-223 du 17 mars 1967"),
        "Décret du 17 mars 1967",
    );
    // Variantes d'espacement observées en prod : « n ° » (espace avant le °),
    // « n » nu suivi du numéro. Le stripper tolère l'espacement → forme datée.
    assert_eq!(
        normalize_instrument("Décret n ° 93-1362 du 30 décembre 1993"),
        "Décret du 30 décembre 1993",
    );
    assert_eq!(
        normalize_instrument("Loi n 65-557 du 10 juillet 1965"),
        normalize_instrument("Loi du 10 juillet 1965"),
    );
    // Instrument UE : le NUMÉRO est l'identité, on jette l'attribution et la
    // date (le texte cite le même règlement sous toutes ces formes — elles
    // doivent collisionner sur la clé du numéro).
    assert_eq!(
        normalize_instrument("Directive 93/13 du Conseil"),
        "Directive 93/13",
    );
}

#[test]
fn normalize_instrument_dated_number_idempotent_with_article() {
    // Chaînes canoniques observées en prod (consolidation du 2026-06-12) :
    // la forme préfixée d'article doit replier le numéro en une passe,
    // comme la forme nue — sinon norm(norm(x)) ≠ norm(x) et la couche
    // canonique produit raw → forme numérotée → forme datée.
    assert_eq!(
        normalize_instrument("la loi n° 65-557 du 10 juillet 1965"),
        "Loi du 10 juillet 1965"
    );
    let once = normalize_instrument("le décret n° 2010-569 du 28 mai 2010,");
    assert_eq!(once, "Décret du 28 mai 2010");
    assert_eq!(normalize_instrument(&once), once);
}

#[test]
fn normalize_instrument_eu_strips_to_number_identity() {
    // Toute forme de surface d'une directive numérotée → clé du numéro seul.
    for form in [
        "Directive 2003/88/CE",
        "Directive 2003/88/CE du 4 novembre 2003",
        "Directive 2003/88/CE du Conseil",
        "Directive 2003/88/CE de la Commission du 4 novembre 2003",
        "Directive 2003/88/CE du Parlement européen et du Conseil du 4 novembre 2003",
    ] {
        assert_eq!(normalize_instrument(form), "Directive 2003/88/CE", "{form}");
    }
    // Idem côté Règlement (numéro = identité), avec alias parenthétique.
    for form in [
        "Règlement (UE) n° 1215/2012",
        "Règlement (UE) n° 1215/2012 du 12 décembre 2012",
        "Règlement (UE) n° 1215/2012 (Bruxelles I bis)",
        "Règlement (UE) n° 1215/2012 du Parlement européen et du Conseil (Bruxelles I bis)",
    ] {
        assert_eq!(
            normalize_instrument(form),
            "Règlement (UE) n° 1215/2012",
            "{form}"
        );
    }
    // Vieille notation (CEE) + attribution sans date → même clé que la datée.
    assert_eq!(
        normalize_instrument("Règlement (CEE) n° 1408/71 du Conseil"),
        normalize_instrument("Règlement (CEE) n° 1408/71 du Conseil du 14 juin 1971"),
    );
    // Directive SANS numéro (date = identité) : le strip ne s'applique pas.
    assert_eq!(
        normalize_instrument("Directive du 16 décembre 2008"),
        "Directive du 16 décembre 2008",
    );
}

#[test]
fn unresolvable_instrument_flags_bare_stubs() {
    assert!(is_unresolvable_instrument("Décret"));
    assert!(is_unresolvable_instrument("Loi"));
    assert!(is_unresolvable_instrument("Décret susvisé"));
    assert!(is_unresolvable_instrument("Règlement précité"));
    assert!(is_unresolvable_instrument("Loi précitée"));
    // Anaphore + queue de prose (le marqueur n'est plus en suffixe) : filtrée.
    assert!(is_unresolvable_instrument(
        "Décret précité sur l'absence d'imputabilité"
    ));
    assert!(is_unresolvable_instrument(
        "Code précité et, à titre subsidiaire"
    ));
    assert!(is_unresolvable_instrument("Règlement susvisé sur"));
    assert!(is_unresolvable_instrument("Décret du même jour"));
    assert!(is_unresolvable_instrument("Code du même"));
    // Instruments réels : jamais filtrés.
    assert!(!is_unresolvable_instrument("Code civil"));
    assert!(!is_unresolvable_instrument("Loi du 10 juillet 1991"));
    assert!(!is_unresolvable_instrument("Décret du 8 janvier 1995"));
    // Famille nue + identité réelle (date) après « du » non-anaphorique : gardée.
    assert!(!is_unresolvable_instrument("Arrêté du 25 juin 1980"));
    // Marqueur présent mais tête non-nue (vrai titre) : gardé.
    assert!(!is_unresolvable_instrument("Code de commerce précité"));
}

#[test]
fn unresolvable_flags_generic_convention_collective() {
    // « Convention collective » sans domaine distinctif : anaphorique / générique,
    // aucune CCN identifiable → filtrée (faux positif d'extraction, ~14k arêtes).
    for g in [
        "Convention collective",
        "Convention collective nationale",
        "Convention collective de travail",
        "Convention collective précitée",
        "Convention collective susvisée",
        "Convention collective nationale précitée",
        "Convention collective applicable",
        "Convention collective applicable, 'la majoration",
        "Convention collective précise",
    ] {
        assert!(is_unresolvable_instrument(g), "devrait être générique: {g}");
    }
    // CCN qualifiée (domaine / date) : reste distinctif → émise (résoluble).
    for ok in [
        "Convention collective nationale des transports routiers",
        "Convention collective de la métallurgie",
        "Convention collective des industries chimiques",
        "Convention collective du 15 mars 1966",
        "Convention collective de travail du personnel des banques",
    ] {
        assert!(
            !is_unresolvable_instrument(ok),
            "ne devrait PAS être générique: {ok}"
        );
    }
}

#[test]
fn unresolvable_prose_overcapture_dropped() {
    // Famille + complément de prose sans identité : sur-capture droppée
    // (cf. ADR 0074). La garde digit/intitulé épargne les vrais textes.
    for g in [
        "Instruction de la demande",
        "Instruction à sa disposition",
        "Instruction et des plaidoiries ni établi un calendrier des échanges",
        "Livre VIII du même code",
        "Livre IV de la présente partie",
        "Titre III du présent code",
        "Nouveau règlement du PLU",
    ] {
        assert!(is_unresolvable_instrument(g), "devrait être garble: {g}");
    }
    // Vrais instruments : conservés.
    for ok in [
        "Code de l'entrée et du séjour des étrangers et du droit d'asile",
        "Livre des procédures fiscales",
        "Instruction générale interministérielle n° 1300",
        "Instruction du 14 janvier 2008",
        "Instruction relative à l'organisation du travail",
        "Convention collective nationale des transports routiers",
    ] {
        assert!(
            !is_unresolvable_instrument(ok),
            "ne devrait PAS être garble: {ok}"
        );
    }
}

#[test]
fn normalize_nouveau_du_code() {
    assert_eq!(
        normalize_instrument("Nouveau du code de procédure civile"),
        "Code de procédure civile"
    );
    assert_eq!(normalize_instrument("nouveau code civil"), "Code civil");
}

#[test]
fn normalize_instrument_treaty_short_forms_converge() {
    // ADR 0112 §6 : chaque forme courte/variante d'un instrument international
    // doit converger vers UNE chaîne canonique stable (sa text_key), JOIN-able
    // au title_key du catalogue. Le canonique est la forme longue datée (accords
    // bilatéraux) ou le titre long (conventions multilatérales / droit UE).
    // Idempotence vérifiée par instrument : norm(norm(x)) == norm(x).
    let groups: &[(&str, &[&str])] = &[
        (
            "Accord franco-algérien du 27 décembre 1968",
            &[
                "Accord franco-algérien du 27 décembre 1968",
                "Accord franco-algérien",
            ],
        ),
        (
            "Convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales",
            &[
                "Convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales",
                "CEDH",
                "CESDH",
            ],
        ),
        (
            "Convention internationale relative aux droits de l'enfant",
            &[
                "Convention internationale relative aux droits de l'enfant",
                "Convention internationale sur les droits de l'enfant",
                "Convention de New-York",
                "Convention de New York",
            ],
        ),
        ("Convention de Genève", &["Convention de Genève"]),
        (
            "Accord franco-marocain du 9 octobre 1987",
            &[
                "Accord franco-marocain du 9 octobre 1987",
                "Accord franco-marocain",
            ],
        ),
        (
            "Accord franco-tunisien du 17 mars 1988",
            &[
                "Accord franco-tunisien du 17 mars 1988",
                "Accord franco-tunisien",
            ],
        ),
        (
            "Charte des droits fondamentaux de l'Union européenne",
            &["Charte des droits fondamentaux de l'Union européenne"],
        ),
        (
            "Traité sur le fonctionnement de l'Union européenne",
            &["Traité sur le fonctionnement de l'Union européenne"],
        ),
    ];
    for (canonical, variants) in groups {
        for variant in *variants {
            let once = normalize_instrument(variant);
            assert_eq!(
                &once, canonical,
                "{variant:?} doit converger vers le canonique"
            );
            // Idempotence : repasser la sortie ne la change pas.
            assert_eq!(
                normalize_instrument(&once),
                once,
                "non idempotent: {variant:?}"
            );
        }
    }
}

#[test]
fn snap_code_name_gating() {
    // Non-code → None (le gating « commence par code » bloque).
    assert_eq!(snap_code_name("Loi du 10 juillet 1991"), None);
    assert!(snap_code_name("requérant").is_none());
}

#[test]
fn normalize_instrument_german_sigles_with_gloss() {
    // Sigle natif nu, + glose FR, + ponctuation collée → forme française d'usage.
    for v in [
        "BGB",
        "BGB allemand",
        "BGB)",
        "Bgb,",
        "Bürgerliches Gesetzbuch",
    ] {
        assert_eq!(normalize_instrument(v), "Code civil allemand", "{v}");
    }
    assert_eq!(normalize_instrument("HGB"), "Code de commerce allemand");
    assert_eq!(normalize_instrument("StGB"), "Code pénal allemand");
    // EGBGB (acte introductif, DIP allemand) reste DISTINCT — premier token ≠ bgb.
    assert_ne!(normalize_instrument("EGBGB"), "Code civil allemand");
}
