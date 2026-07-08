//! `lj-core` — noyau PUR partagé serve+ingest de LibreJustice.
//!
//! Aucun I/O au runtime : primitives texte (tokenisation, recasse, folding),
//! modèle `Decision`, taxonomie, résumé, publication. Les données statiques
//! (tables FR de recasse, collocations) sont embarquées via `include_str!`.
//! L'I/O vit dans `lj-store` / `lj-sources` / `lj-llm` / `lj-api` /
//! `lj-ingest`.
//!
//! La pile d'extraction lourde (recognizer de citations, normaliseurs, parsers
//! de sources, identité) est sortie dans `lj-extract` (étage ingest, ADR 0123)
//! pour que `lj-server` ne la compile plus.

pub mod aliases;
pub mod body_tok;
pub mod collocations;
pub mod decision;
pub mod error;
pub mod normalizer;
pub mod parsing;
pub mod publication;
pub mod referential_labels;
pub mod schema;
pub mod source_authority;
pub mod summary;
pub mod text;
pub mod titles;
pub mod tokens;
pub mod truecase;

/// Version du pipeline d'extraction (ADR 0085) : `extract_version` stampé sur
/// `decisions`, comparé pour le gating de ré-extraction. Produite par `lj-extract`
/// (re-exportée en `lj_extract::extract::EXTRACT_VERSION`) et consommée par
/// `lj-store` pour le gating SQL — d'où sa résidence dans le noyau partagé, leur
/// ancêtre commun, plutôt que dans `lj-extract` (que `lj-store` ne tire plus,
/// ADR 0123 §3). À bumper quand la logique d'extraction change.
/// v9 = capture d'occurrences (ADR 0143) : spans token, dédup déterministe.
/// v10 = linker in-pass (ADR 0145) : les occurrences sortent avec leur cible
/// catalogue (`ref_text_uid`/`ref_num_key`) posée sur le `LinkSnapshot` du run.
/// v11 = uids référentiels (ADR 0146) : `solution:*`/`voie:*`/`office:*`/
/// `domaine:*`/`instance:*`/`publication:*` + `jurisdiction_code` dérivés à
/// l'extraction (`lj_extract::facets`).
/// v12 = fusion extraction→référentiels (ADR 0148/0151) : les scanners
/// émettent les uids directement (plus de vocabulaire intermédiaire ni
/// d'écriture des colonnes TEXT ancien-monde, droppées en 0107) ; l'axe
/// `instance:*` disparaît.
/// v13 = extraction compilée en prod (ADR 0156/0157/0158) : UN `doc_extract`
/// par décision (automate fusionné marqueurs + citations), champs texte par
/// les composeurs `DocScan`, citations par le moteur compilé (ancres
/// structurelles + snap catalogue) ; `themes` Judilibre matérialisés
/// (ADR 0159, migration 0108).
/// v14 = mono-extraction à flux de tokens unifié (ADR 0160) : mort de la
/// chaîne legacy refs, discipline de consommation unique (rôles cite+marker
/// par motif, clôture de préfixe), spans disjoints par construction (plus de
/// balayage anti-chevauchement), énumérations enjambant le qualificatif de
/// rédaction.
/// v15 = alignement école gold des facettes (#78) : solution admin au
/// dispositif verbatim (ANNULATION/REFORMATION, cassation CE), gabarit Admin
/// réécrit (passives fléchies, condamnations), PAPC décisif (jamais chambre
/// criminelle, label « Désistement PAPC » qualifiant), MAGISTRAT_DESIGNE
/// = juge unique TA (référés compris), JEX = TJ seul, vote domaine branché en
/// prod + hint texte (FP/aide/urba/étrangers/fiscal) quand seul le CJA vote,
/// CCH 300/441 → aide sociale. Citations byte-identiques à v14.
/// v16 = liens de chronologie (ADR 0161) : couche `decision_links` extraite
/// (métadonnée Judilibre `contested` + texte admin « Par un jugement… » +
/// « Décision déférée ») en clés pendantes `canonical_ref`, résolues à
/// l'écriture puis en batch. Champs colonnes identiques à v15.
/// v17 = la marche d'énumération d'articles enjambe le paragraphe romain de
/// subdivision (« les articles L. 1142-1, I, du code de la santé publique et
/// 36 de la loi n° 66-879 ») : le numéral distribué derrière le génitif est
/// capturé et lié. Banc gold : +47 spans, +2 liens, zéro perte.
/// v18 = instrument consommé par une locution d'articles retiré des données
/// (ADR 0166) : plus de ligne instrument génitive (« article N du X »), le
/// pont de rendu `is_attached_instrument` disparaît.
/// v19 = citations de jurisprudence (ADR 0165) : `lex_case` dans le scan
/// fusionné, six familles (CC/CE/CONSTIT/CJUE/CEDH/RG) en clés pendantes vers
/// `case_citation`, résolution SQL par famille en fin de run. Citations
/// d'instruments byte-identiques à v18.
/// v20 = le désignateur de zone d'urbanisme fait génitif orphelin
/// (« l'article 3 UA du POS » : article du règlement local de zone, hors
/// catalogue — l'unicité le liait à la première loi porteuse citée), et le
/// suffixe fiscal deux-lettres fait adjacence (« L. 80 CA du livre des
/// procédures fiscales » lie par le génitif, l'instrument consommé). Banc
/// gelé : +1 hit, −2 faux-uid, −17 spans d'instruments consommés.
/// v21 = grammaire `lex_case` étendue (#93) : chaîne du fond administratif
/// (jugements TA 7 chiffres, arrêts CAA, ordonnances CE/TA — clés
/// `af|ta_*|…`, `af|caa_*|…`, `ce|…`, ADR 0165 [af]), sondes ARRIÈRE CE/CONSTIT (« Par une
/// décision n° X …, le Conseil d'État »), chambres Cassation citées seules
/// (« Civ. 1ère », « Soc. »), énumérations datées et plages de pourvois,
/// graphies élargies (paren, virgule, tirets), CEDH derrière « arrêt » et
/// « affaire », CJUE find_iter préfixé + héritage de préfixe, RG :
/// juridiction aval, CPH/TA/CAA au ChronoSnapshot, formes TCOM
/// lettrées/tout-chiffres, ancres « enregistrée sous le n° » / « déférée » /
/// sigle CA, clé NUE `rg||NUM` quand la juridiction ne mappe pas (jamais
/// résolue — zéro mislink). Gold case : R exact 0,42 → 0,94. Citations
/// d'instruments byte-identiques à v20 (banc gelé inchangé).
/// v22 = l'ordinal arabe de subdivision n'est plus un article (« l'article
/// L. 1142-1-1, 1°, du code de la santé publique » : la marche d'énumération
/// enjambe « 1° » comme le paragraphe romain — le span parasite « 1 »
/// disparaît), et spans PONTÉS MÉTADONNÉE (ADR 0161 ∩ 0165) : la décision
/// attaquée citée inline sans docket (« l'arrêt attaqué (Paris, 22 août
/// 2024) », « l'arrêt rendu le 22 août 2024 par la cour d'appel de Paris »,
/// ville + date validées contre la métadonnée `contested`) devient une
/// citation `case_citation` au `target_ref` du lien de chronologie, résolue
/// par le pont SQL depuis `decision_links`.
/// v23 = formation structurée (ADR 0170) : les champs source de la formation
/// (code chambre CC, chambre de bandeau Judilibre, formation greffe) sont
/// décomposés en axes — `chamber_position` (display canonique recomposé),
/// `chambre_uid`/`formation_uid` (FK référentiels), rôle versé dans
/// `office`/`voie` en repli des scanners texte. `formation_or_chamber`
/// continue d'être écrite à l'identique jusqu'à son drop (séquencement 0170).
/// v24 = fusion greffe + bandeau des axes formation : le champ `chamber`
/// greffe prime (position), le bandeau complète (spécialisation → position
/// adjectivée « 1re chambre civile », badge `chambre:*`) ; display composé
/// après toutes les parts, position du bandeau seulement si le greffe est
/// muet.
/// v25 = correctifs du gate de titres (ADR 0170 ét.4, revue exhaustive) :
/// suffixe ordinal requis devant un mot-clé (« chambre 2 section 1 » ne vaut
/// plus 2/2), ordre source section↔chambre restitué (TA), SSR/chambres
/// réunies numérotées génériques (tirets, « et », listes, toutes-lettres),
/// consommation de span office→formation (« président de la section du
/// contentieux » ne vote plus SECTION), graphies éclatées recollées
/// (« R E F E R E », « J.A.F. »), lettres de section portées par la
/// spécialisation (« Sociale A »), ordinal en degré (« 2° »), points d'alias
/// (« Ch. 3 »), lexiques enrichis (procédures collectives, juge unique par
/// régime ≤ 10 000 € / R222-13 / 15 jours, plénière, TERRES, CIVI…) ; le
/// siège affiche l'office entre parenthèses derrière une position.
/// v26 = derniers correctifs du gate (revue exhaustive finale) : « JU » de
/// greffe TA (« 10ème chambre (JU) » → juge unique), composée à lettre collée
/// (« Chambre 4-8a » → « Chambre 4-8, section A »), tiret de jonction
/// alias-numéro (« Chambre-1 civile »), ordinal suffixé isolé porté par la
/// spécialisation (« 2EME protection sociale » → « 2e chambre — protection
/// sociale », « 3ème chambre famille » → « 3e chambre de la famille ») ; le
/// siège sans position affiche l'office avant le type de formation
/// (« juge des référés », pas « juge unique »).
/// v27 = seconde revue exhaustive du gate : chambre zéro = placeholder
/// (« Chambre 0 référés » ne fait plus « 0e chambre »), composée à alias
/// pointé / spécialisation interposée (« Ch civ. 1-4 », « Ch.protection
/// sociale 4-7 » → « Chambre 4-7 — protection sociale »), numéro collé au mot
/// chambre (« 13CH JCP civil » → « 13e chambre civile »), lettre de section
/// derrière le numéro ou le mot chambre (« Pôle 4 - chambre 9 - B »,
/// « 1re chambre B », « Chambre 1 A » → «, section X »), spécialisations
/// famille/filiation lettrées, apostrophe exclue des lettres de section
/// (« baux d'habitation » ≠ section D).
/// v28 = contre-revue du diff v27 : l'ordinal « e » collé au chiffre n'est
/// plus une lettre de section (« 1re chambre 2e section » ne fait plus
/// «, section E » — séparateur requis avant la lettre), lettre de section
/// collée au numéro de chambre hors e/h (« Ch 9b » → « 9e chambre,
/// section B »).
pub const EXTRACT_VERSION: i16 = 28;

/// Version du **certifieur de capture** (ADR 0125 Inc.2-bis) : axe orthogonal à
/// `EXTRACT_VERSION`. Une décision est « certifiée » à `decisions.certified_version =
/// CERTIFIER_VERSION` quand l'oracle haut-recall ne trouve rien de plus que le
/// recognizer ET que ses citations résolvent (ou sont confiance-ment non résolvables).
/// Le re-extract par défaut SKIP les décisions certifiées à la version courante
/// (`certified_version >= CERTIFIER_VERSION`) même sur un bump d'`EXTRACT_VERSION` —
/// cache de confiance invalidé par comparaison (sélectif, sans write de masse), jamais
/// un sceau (le re-extract ciblé `--juridiction-type`/`--overwrite` l'ignore). À bumper
/// quand l'oracle progresse (re-certifie tout). Tant qu'aucun setter ne l'écrit, la
/// colonne reste NULL → comportement strictement préservé (skip-rule no-op).
pub const CERTIFIER_VERSION: i32 = 1;
