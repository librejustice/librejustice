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
pub mod article_key;
pub mod article_order;
pub mod body_tok;
pub mod collocations;
pub mod compare;
pub mod decision;
pub mod error;
pub mod forme_juridique;
pub mod jurisdictions;
pub mod normalizer;
pub mod parsing;
pub mod procedural;
pub mod publication;
pub mod referential_labels;
pub mod schema;
pub mod source_authority;
pub mod suggest;
pub mod summary;
pub mod text;
pub mod titles;
pub mod tokens;
pub mod truecase;
pub mod usage;

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
/// v29 = suffixe matière deux lettres collé à la composée (« Chambre 1-5DP »
/// → « Chambre 1-5 ») ; `search_title` composé et écrit par l'ingest
/// (ADR 0170 ét.5 — colonne simple, plus de colonne générée sur
/// `formation_or_chamber`).
/// v30 = titre : juridiction guérie par le label référentiel (code → label
/// avec ville) quand le libellé source est nu ; CAA identifiée par le code
/// cour du numéro de requête (« 12BX02667 » → Bordeaux) ; « 5e chambre
/// (chambre jugeant seule) » → « 5e chambre jugeant seule ».
/// v31 = repli docket CAA aussi quand le nom extrait est nu (« Cour
/// administrative d'appel » sans ville) ; greffe « jugeant seule » sur les
/// positions en « sous-section » (« 3e sous-section jugeant seule »).
/// v32 = composition juge unique : les offices à juge seul (juge des
/// référés L. 511-2 CJA, magistrat désigné) posent `formation:JUGE_UNIQUE` ;
/// suffixe greffe « - R. 222-13 » lu comme tel ; composition dite par le
/// TEXTE en zone d'en-tête (« statuant en juge unique », « siégeant seul »)
/// et référé TA signé par ses articles CJA.
/// v33 = domaine coloré par la chambre (`domain::context_for`) : code
/// Judilibre CC (`soc`/`comm`/`cr`), TCOM commercial par nature, l'action
/// civile devant la chambre criminelle reste CRIMINEL ; votes de TERMES du
/// scan (`domain_term_votes` + `refine_with_terms`) — un domaine nul ou
/// parent nu se raffine au vocabulaire de la matière compté plein texte
/// (≥ 2 occurrences, ≥ 2× le second, parent cohérent) ; la chambre
/// criminelle prime TOUT vote (fraude fiscale au pénal comprise) ; plancher
/// CIVIL sur les chambres civiles CC muettes ; le défaut « obligations »
/// (RC_CONTRATS) se sur-classe à seuil renforcé (≥ 5, ≥ 3×) ; CPP/CP muets
/// dans l'ordre admin — les extraditions CE restent du droit des étrangers.
/// v34 = domaine par code NAC (ADR 0177) : `Decision.nac` (payload Judilibre,
/// TJ 100 % / CA 90 %) mappé sur la taxonomie par la table curée de la
/// nomenclature officielle (`domain::nac_domain`) — comble un domaine absent
/// ou parent nu et tranche les désaccords de parent, sans contester un
/// sous-domaine du même parent posé par les codes cités ; CSS voté par plage
/// (L. 452 faute inexcusable → travail, cotisations/recouvrement → SOCIAL
/// nu, prestations → aide, SOCIAL même en ordre admin).
/// v35 = office : le référé est une voie, jamais un office (`JUGE_REFERES`
/// n'est plus émis — la surface greffe implique toujours la composition juge
/// unique) ; MAGISTRAT_DESIGNE posé sur CAA/CE quand la signature en pied
/// porte « …désigné » (président / conseiller d'État / magistrat désigné).
/// v36 = formation : chambre lue en LIGNE de bandeau CA (« 2ème chambre »,
/// villes en « d' », listes à virgule, abréviations à points) ; solution :
/// écoles TJ divorce / expulsion référé-conditionnelle = PARTIELLE,
/// marqueurs de partialité « en tant que », « dans cette mesure »,
/// retranchement ; office : MAGISTRAT_DESIGNE sur signature mise en état /
/// magistrat instructeur CA-TJ. Le legacy `formation_or_chamber` est éradiqué
/// (exécution finale ADR 0170) : la coloration du vote domaine lit les AXES
/// (`chambre_uid`/`chamber_position`) au lieu de la chaîne aplatie.
/// v37 = decision_party au grain acteur (ADR 0182) : relation canonique
/// émise au fil de l'eau (spans-évidences par matching replié, nature,
/// `resolve_key`, `extract_version`) — la qualité `intervenor` est gatée
/// (§7, P < 85 % au banc). Les colonnes plates de `decisions` sont
/// inchangées (la passe v36 tardive portait déjà les écoles solution
/// « irrecevabilité prononcée » et formation JUGE_UNIQUE référés).
/// v38 = évidences de résolution des avocats (ADR 0188) : `resolve_key`
/// counsel enrichie du prénom en apposition (« Me Laura JAVERT » pour la
/// valeur nom-seul), colonne `barreau` (slug officiel CNB en apposition) —
/// nourrit la résolution `cnb:` (join ± rotations de tokens, départage par
/// barreau, décisions CC/CE exclues). Colonnes plates inchangées.
/// v39 = rôle des avocats + cohérence de côté (ADR 0194) : rôle explicite
/// capté en apposition (substituant/substitue sur les constructions de
/// délégation, postulant/plaidant style CA, fenêtre bornée au prochain
/// « Me ») ; une même valeur counsel émise des deux côtés par le NER
/// fusionne en UNE ligne à côté indéterminé. Colonnes plates inchangées.
/// v40 = hygiène counsel (campagne parties 2026-07-10) : la valeur counsel
/// est la personne seule — queue d'apposition cabinet ébarbée (« X de la
/// SELARL Y » → « X »), titre nu final « Avocat(s) » retiré (ellipse
/// d'anonymisation ancienne « F... » préservée) ; la métadonnée
/// `Avocat_Requerant` route en cabinet aussi sur structure en QUEUE
/// (« DHALLUIN SCP »).
///
/// **v41 — doctrine administrative (ADR 0196)** : ancre `boi` (code BOI cité
/// = `text_uid` BOFiP, alias direct, versions datées `-AAAAMMJJ` rabotées à
/// la clé) ; circulaires datées résolues règles 3/9 (`head_act_nature` +
/// index (circulaire, `date_texte`) en colonne — les titres du fond sont
/// libres).
///
/// **v42 — clé NOR** : le token NOR d'une mention d'acte daté est capturé
/// (entre nature et date, en queue, graphie collée « NORINTK… » incluse),
/// raboté du `text_key` quand une identité chiffrée survit, et résolu par
/// l'index `nor → uid` du snapshot (règle 1bis, avant le gate — un NOR est
/// un acte ministériel publié, jamais un acte individuel). Catalogue : NOR
/// backfillé depuis LEGI (`META_COMMUN/NOR`) en plus du fond CIRCULAIRES.
///
/// **v43 — domaine par thèmes Judilibre** : `Decision.themes` (titrage CC,
/// nomenclature CA/TJ) mappé sur la taxonomie par table de matières curée
/// (`domain::theme_domain`) — comble un domaine absent ou parent nu et
/// tranche les désaccords de parent comme le NAC (ADR 0177), sans contester
/// un sous-domaine du même parent ni le CRIMINEL de la chambre criminelle ;
/// rétention / zone d'attente / nationalité signent le contentieux JLD des
/// étrangers à n'importe quelle profondeur du titrage.
///
/// **v44 — vocabulaire domaine admin plein texte** : les familles de termes
/// admin (`ProcDomFp`/`Aide`/`Etr`) s'étoffent du vocabulaire réel des corps
/// anonymisés (échelon, reclassement, agent contractuel, congés maladie ;
/// allocations familiales, ASE ; asile, réfugié, apatride, nationalité
/// française) — le fallback PUBLIC nu se raffine par les votes de termes
/// existants ; le vocabulaire travail des deux ordres (licenciement, heures
/// supplémentaires, rupture conventionnelle, indemnité de préavis) migre en
/// `ProcDomTravailMixte` et vote fonction publique dans l'ordre admin.
///
/// **v45 — école AT/MP alignée** : plage CSS livre 4 entier (L. 411-482 —
/// déclaration, rentes, maladies professionnelles, pas seulement la faute
/// inexcusable L. 452) + prévoyance collective (L. 911-914) = contentieux
/// du travail ; les TERMES AT/MP (`ProcDomSecu`) votent travail dans les
/// deux ordres au lieu d'aide sociale, « cotisations sociales » vote le
/// SOCIAL nu terminal (`ProcDomCotisations`).
///
/// **v46 — confirme/infirme au gabarit Admin** : les ordonnances judiciaires
/// sans étiquettes de bloc de greffe (JLD rétention/hospitalisation,
/// référés) routées au gabarit Admin faute de pivot lisent désormais
/// « confirme/confirmons » en appel judiciaire — CONFIRMATION, ou
/// INFIRMATION_PARTIELLE sur les mêmes règles de partialité que le gabarit
/// Blocs (« partiellement », « sauf », infirme joint). Un dispositif
/// administratif ne dit jamais « confirme » (gold : 166 judiciaires,
/// 0 admin).
///
/// **v47 — zone dispositif ancrée** : `dispositif_start` préfère le premier
/// « par ces motifs » aux autres surfaces (« en conséquence », « décide »,
/// « arrête ») qui abondent dans les motifs et ouvraient la zone sur la
/// narration (les demandes des parties « infirmer le jugement » y
/// polluaient la partialité) ; « sauf à + infinitif » (rectifier une
/// erreur, moduler l'astreinte) ne compte plus comme partialité.
///
/// **v48 — irrecevabilité des motifs de clôture** : au gabarit Admin, un
/// dispositif « rejette » nu lit aussi le prononcé d'irrecevabilité dans les
/// 800 chars de motifs précédant le dispositif (`closing_irrec`) —
/// « la requête est manifestement irrecevable », « n'est pas recevable »,
/// « tardive », rejet au 4° de l'article R. 222-1 — quand la phrase vise
/// l'objet contentieux et qu'aucun marqueur de fond ni de mixte (moyen
/// irrecevable, « dépourvue de fondement », nouvelle tête « sur les
/// conclusions ») ne s'interpose. L'appel confirmant une irrecevabilité de
/// première instance reste REJET (école gold non tranchée). Le pourvoi non
/// admis tolère un sujet long (« le pourvoi de la commune X et de la
/// communauté de communes Y n'est pas admis » : contexte pourvoi porté de
/// 80 à 160 chars, « admis au bénéfice de… » exclu explicitement).
///
/// **v49 — octrois sans condamnation (gabarit Blocs + Admin)** : nouveau
/// kind `OutGrant` (ouverture de procédure collective, interdiction de
/// gérer / faillite personnelle, renouvellement / prorogation de période
/// d'observation, mainlevée d'une mesure JLD, ordonnance commune et
/// prorogation de délai en référé) → SATISFACTION_TOTALE, PARTIELLE si un
/// rejet l'accompagne. Au gabarit Blocs, cond/grant priment désormais le
/// neutre (un dispositif qui condamne à payer n'est pas AUTRE parce qu'il
/// renvoie aussi à la mise en état) — le rejet nu reste derrière le
/// neutre ; l'irrecevabilité Blocs exige un prononcé non nié (« recevable
/// et non prescrite » ne compte plus).
///
/// **v50 — mixte irrecevable + fond, et arbitrage label ↔ dispositif
/// judiciaire** : aux gabarits Cc et Blocs, l'irrecevabilité ne sort plus
/// quand un rejet/débouté SUBSTANTIEL (clause hors art. 700/dépens/surplus/
/// « toute autre ») figure au même dispositif — le fond absorbe (école
/// gold). Côté labels judiciaires : « Fait droit à l'ensemble des
/// demandes » avec un dispositif lu SATISFACTION_PARTIELLE (condamnation +
/// têtes rejetées) → PARTIELLE ; label « irrecevabilité » sur dispositif
/// lu REJET → REJET (même arbitrage que l'ordre admin).
///
/// **v51 — partialité de cassation assainie, labels de routage arbitrés** :
/// « casse et annule, dans/en toutes ses dispositions » force la cassation
/// TOTALE (la formule de transcription « l'arrêt partiellement cassé » et le
/// « en tant qu'il » d'une clause de rejet voisine ne votent plus) ; les
/// en-têtes de moyens annexés sans « produit par » bornent la zone
/// dispositif ; un label opendata « satisfaction partielle » ne fait plus
/// mécaniquement une CASSATION_PARTIELLE (partialité lue au dispositif, le
/// label « cassation partielle » du greffe reste souverain). Labels de
/// routage judiciaires (« renvoi à la mise en état », « statue sur un
/// incident », qpc…) : le dispositif prime quand il lit un sort réel
/// (IRREC/SATISFACTION/CONF-INF), AUTRE n'est que son silence. Non-admis :
/// formes féminines/plurielles (« ne sont pas admises », « pourvois non
/// admis »).
///
/// **v52 — partialité de satisfaction et de confirmation assainies** : le
/// NOM « rejet » en énumération (« propositions d'admission ou de rejet »,
/// boilerplate d'ouverture RJ/LJ) n'est plus un prononcé ; le point
/// d'abréviation « M. [X] » ne clôt plus la clause d'un condamne/rejette
/// (« condamnons M. [W] à payer… » redevient une condamnation
/// substantielle) ; le rejet d'une DÉFENSE (délais de paiement, suspension
/// des effets de la clause résolutoire, exception, note écartée des débats)
/// ne rend plus la satisfaction partielle ; le non-lieu du gabarit Blocs
/// n'est décisif que sans contre-signal substantiel (tête de non-lieu
/// isolée dans un dispositif mixte → partialité) ; après un CONFIRME,
/// « en ce qu'il a… / en ses dispositions soumises à la cour » ÉNUMÈRE les
/// chefs confirmés (CONFIRMATION) — seuls « sauf en ce qu », « mais
/// seulement »… rendent la confirmation partielle (après un INFIRME,
/// « en ce qu'il » reste une partialité).
///
/// **v53 — zone dispositif fiabilisée, labels et objets rejetés arbitrés** :
/// `dispositif_start` préfère un ouvreur FORT (« par ces motifs », formes
/// espacées de greffe, verbe en capitales suivi de « : ») — « Sur la
/// légalité de l'arrêté : » (l'acte attaqué en intertitre) et « par voie de
/// conséquence » n'ouvrent plus la zone en plein motifs. Label admin
/// « Rejet » arbitré au dispositif VERBATIM : jugement renversé →
/// ANNULATION/REFORMATION (évocation en appel). L'OBJET d'une demande
/// rejetée ne vote plus le sort neutre (« rejetons la demande de
/// radiation », « exception d'incompétence, n'y fait pas droit », « avant
/// dire droit, rejette la mesure d'expertise » → REJET, pas AUTRE).
///
/// **v54 — non-lieu principal vs rejet accessoire, QPC non renvoyée** :
/// `substantive_rejet` lit aussi le segment AVANT un token passif (« le
/// surplus des conclusions est rejeté » : l'objet précède le verbe) et
/// exclut 761-1 ; le non-lieu admin ne bascule en REJET que sur rejet
/// SUBSTANTIEL (le rejet du seul accessoire laisse le non-lieu principal —
/// ordonnances CE de non-lieu sur pourvoi) ; « n'y avoir lieu de
/// renvoyer » (QPC) est un non-lieu, label Cc de routage arbitré
/// (« qpc » → NON_LIEU quand le dispositif le dit) ; le non-lieu Blocs
/// passe APRÈS confirme/infirme (catégorie la plus spécifique).
///
/// **v55 — vocabulaire fonction publique élargi** : pensions publiques
/// (« pension de retraite », « pensions civiles et militaires » — CPCMR
/// seul, les pensions militaires d'invalidité/victimes de guerre CPMIVG
/// restent PUBLIC nu), concours de recrutement, statuts hospitaliers
/// (« praticien hospitalier »), préretraite amiante (« exposition
/// professionnelle ») ; « harcèlement moral » et « temps de travail »
/// votent travail-mixte (fonction publique en admin, prud'hommes en
/// judiciaire). Écarté après mesure : « juridictions de pension » (cite
/// l'article R. 822-5 CJA en boilerplate PAPC), surfaces cotisations
/// (le domaine de ces décisions vient des citations/NAC, effet nul).
///
/// **v56 — vote CSS : mention nue et plage technique** : une citation du
/// code de la sécurité sociale SANS article lié vote le parent SOCIAL à
/// poids 0.5 (elle votait AIDE par le bras par défaut — 29 votes AIDE sur
/// une seule décision cotisations) ; la plage SOCIAL nu s'étend au livre 1
/// technique (114 pénalités, 124, 131-145 recouvrement/expertise/
/// contentieux, 161, 165) et à l'assujettissement (311, 315). Écarté après
/// mesure : neutraliser 142/144 (perd plus qu'il ne gagne), exception NAC
/// 88E (verrouillée par le vote de termes, même parent — effet nul).
///
/// **v57 — immobilier public : codes votants et vocabulaire urbanisme** :
/// le CG3P et le code de la voirie routière votent URBANISME_IMMOBILIER_
/// PUBLIC (admin), le code de l'expropriation vote selon l'ordre (admin →
/// immobilier public, judiciaire → CIVIL_EXPROPRIATION_PREEMPTION) ;
/// surfaces ProcDomUrba élargies (déclaration préalable, permis d'aménager/
/// démolir, certificat d'urbanisme, taxe d'aménagement, domaine public,
/// travaux/ouvrage publics, aménagement commercial/cinématographique,
/// grande voirie, menaçant ruine). Écarté après mesure : vote du code de
/// la commande publique (golds majoritairement PUBLIC nu — école à
/// trancher).
///
/// **v58 — vocabulaire étrangers / aide sociale / fiscal** : OFII,
/// conditions matérielles d'accueil, laissez-passer (étrangers) ; fonds de
/// solidarité pour le logement, carte mobilité inclusion, bourse sur
/// critères sociaux, attribution de logement (injonction DALO), retraite
/// du combattant, indemnités journalières (aide/action sociale) ; saisie
/// administrative et avis à tiers détenteur, crédit d'impôt, comptable
/// public, fonds de solidarité COVID entreprises (fiscal — distinct du FSL
/// logement, qui reste aide sociale).
///
/// **v59 — familles environnement et répression administrative** : deux
/// nouveaux marqueurs de domaine admin. `ProcDomEnv` (ICPE, autorisation
/// environnementale, dépollution, zone humide, méthanisation, régime
/// forestier, défrichement, espèces protégées, Natura 2000, prairies
/// permanentes, affichage environnemental) → PUBLIC_DROIT_ENVIRONNEMENT ;
/// `ProcDomPenalPub` (conditions de détention, administration
/// pénitentiaire, amendes administratives, saisie définitive d'armes) →
/// PUBLIC_DROIT_PENAL_PUBLIC. Écarté après mesure : « pénitentiaire » nu
/// (capture les litiges d'AGENTS pénitentiaires, gold PUBLIC_TRAVAIL).
///
/// **v60 — plages code civil preuve/quasi-contrats, visas et extraditions** :
/// les articles de preuve anciens (1315-1316, cités partout) ne votent plus
/// RC_CONTRATS mais CIVIL 0.5 ; les quasi-contrats anciens (1371-1381 :
/// gestion d'affaires, répétition de l'indu) rejoignent la plage
/// RESPONSABILITE (école thèmes « responsabilité et quasi-contrats »).
/// Surfaces étrangers : « refus de visa » (commission de recours) et
/// « extradition » (décrets CE — l'école admin était déjà ETRANGERS, le
/// garde-parent de `refine_with_terms` protège le CRIMINEL judiciaire).
///
/// **v61 — longue traîne fonction publique** : surfaces relation d'emploi
/// hors vocabulaire statutaire — accident de service, sanction de blâme,
/// déplacement d'office (La Poste), concours professionnel, gestion de sa
/// carrière, indemnité de logement. Écarté après mesure : hint FP en plein
/// texte (`has_any` au lieu de la zone en-tête — effet nul, les cas à
/// marqueur tardif sont bloqués par un vote citations non-PUBLIC).
///
/// **v62 — date d'audience : formules référés TA et Cc épelées** : nouvelles
/// variantes « audience publique qui a eu lieu / qui s'est tenue / tenue
/// le … », « averties du jour de l'audience du … » (formule d'avis, la plus
/// fréquente : +66 à elle seule), « audience de plaidoiries du … »,
/// « audience tenue en chambre du conseil du … », date numérique nue après
/// l'ancre (« audience publique du 14/05/2001 ») ; formule Cc épelée avec
/// lieu interposé (« tenue au Palais de Justice, à PARIS, le … ») ; année
/// « deux mil » et jour numérique dans les dates épelées.
///
/// **v63 — PUBLI_RECUEIL dila-jade, réparation d'année retirée** : le
/// classement Lebon A/B/C (`META_JURI_ADMIN/PUBLI_RECUEIL`) alimente
/// `publication_codes` dans les deux constructeurs DILA jumeaux (XML et
/// `source_fields`) — il n'était jamais lu (100 manqués → 1). La
/// « réparation » d'année de la date d'audience (année-1, même mois, ±7
/// jours → recalée sur l'année de lecture) est retirée : elle écrasait des
/// audiences réellement anciennes d'un an (délibérés COVID). Écartés après
/// mesure : exclusion « prononcé … à l'audience du » (tue plus de vraies
/// dates de débats qu'elle n'en corrige), BARE avant DEBATS (idem).
///
/// **v64 — filtrage R. 222-1 élargi, magistrat désigné par formule
/// d'en-tête** : qualifieurs `ProcFiltrage` étendus (« par ordonnance en
/// application », citations élidées « par ordonnance, rejeter », séries 6°,
/// délégation 7°, « sans (le) ministère d'avocat ») ; nouveau marqueur
/// `ProcMagdesForm` (« désigné M./Mme X ») lu en tête avec contexte
/// (« président » + « de la cour »/« du tribunal » avant ; référés,
/// R. 222-1, « pour statuer », pouvoirs prévus après) → MAGISTRAT_DESIGNE
/// pour les ordonnances CAA (formule « le président de la cour a désigné »)
/// et TA (« le président du tribunal a désigné … pour statuer sur les
/// demandes de référé »).
///
/// **v65 — référé judiciaire dit par le texte** : nouveau marqueur
/// `ProcRefCivil` → REFERE_CIVIL pour CA/TJ/TCOM (jamais CC : le récit y
/// raconte l'instance d'origine) quand le texte dit « Vu l'assignation en
/// référé » (visa premier président), « ordonnance de référé » au bandeau
/// (titre ou décision déférée, hors récit « Par ordonnance de référé »),
/// appel d'une / confirme l' / réforme l'ordonnance de référé (contextes
/// AVANT le token — des surfaces composées voleraient « confirme » /
/// « réforme » au dispositif), ou « fait assigner … devant le juge des
/// référés » en tête ; pas de voie sur un désistement (label métadonnée ou
/// « ARRÊT DE DÉSISTEMENT » au bandeau, nouveau signal `desist_bandeau`).
///
/// **v66 — « et suivants » dans le span des citations (ADR 0226)** : la
/// locution qui suit un numéro d'article (« 26 et suivants », féminin,
/// « et s. », virgule OCR) entre dans le span et pose le signal `suivants`
/// sur la ligne `legal_citation` ; à l'écriture, la famille TOC de l'ancre
/// (`_suivants_family`, section unique, VIGUEUR, cap 20) alimente
/// `legal_article_composite`. Banc citations : exact P 0.876→0.890,
/// R 0.934→0.950 ; touché LINKED 0.956→0.974 ; 16 champs et cases au bit
/// près.
///
/// **v67 — droit primaire UE en forme courte** : les désignations courtes des
/// décisions CJUE lient par alias — « du statut de la Cour » (EU/STATUT-CJUE ;
/// les formes CPI/CIJ plus longues priment par leftmost-longest et ciblent
/// leurs propres fiches), « du règlement de procédure du Tribunal / de la
/// Cour » (EU/RPROC/*, la forme nue ambiguë s'abstient) ; « statut » entre
/// dans les natures d'anaphore (« l'article 53 du même statut »).
///
/// **v68 — parent implicite par juridiction + fonds sans alias** : le forum
/// (CEDH, CJUE Cour/Tribunal/TFP via ECLI) résout les instruments nus — « du
/// règlement de procédure » d'un arrêt CJUE cible le règlement de la
/// formation qui parle, « de la Convention » et l'article nu de style
/// Convention (« Art. 13 - … » des annexes, « 5 § 3 ») d'un arrêt CEDH
/// ciblent la CESDH (article validé au catalogue). Gap génitif : paragraphe
/// romain collé (« 75-I de la loi ») et « of the » anglais. Alias :
/// Constitution de 1946, convention n° 108 du Conseil de l'Europe,
/// convention OIT n° 158, CESDH en anglais, aide juridique 91-647 par
/// articles 37/75 (clé datée tuée par les neuf lois du même JO). Droit
/// dérivé UE cité par date sans numéro (« règlement du 17 décembre 2013
/// établissant les règles… ») : capture datée + départage par tokens de
/// titre dans l'index (nature, date), abstention sans queue distinctive.
///
/// **v69 — filtre des noms anonymisés à l'émission** (école « noms
/// anonymisés », 2026-07-19) : une personne au patronyme placeholder
/// (« X... », « Bruno Y... », « B...pour » — l'anonymisation source avale
/// parfois l'espace) ne s'émet plus dans `*_counsel_names` ; un cabinet ne
/// tombe que s'il est tout-placeholder (« SCP F... N... G... »), un token
/// réel le sauve (« SELARL FONTENEAU - B... - MARCHAND »). La mention reste
/// extraite en interne (attribution des côtés) — filtre à l'émission
/// uniquement, ce qui purge aussi `decision_party` (~181 k lignes prod).
/// Banc : counsel P 75.8→76.4 / 63.8→65.6, R 75.9→76.4 / 65.1→66.5 ;
/// autres volets au bit près.
///
/// **v70 — convergence clé de citation ↔ clé d'identité (ADR 0236)** : la
/// série ordinale complète (« duovicies » … « septtricies », variantes
/// « cinquies »/« novies »/« sexties », alternation triée longueur
/// décroissante partagée capture ↔ normaliseur) avec garde de frontière
/// (« terrain » ≠ « ter ») ; les discriminants post-suffixe (« 46
/// quater-0 W ») et les sous-numéros pointés KALI (« 1.01 » ≡ clé `1-01`)
/// survivent à `article_core`. Les citations vers les annexes CGI/LPF et
/// conventions collectives portent désormais le num ENTIER — même clé que
/// l'article servi. Banc GT neutre (la série n'y est presque pas citée) ;
/// le gain vit en prod, sur les nums que la clé d'identité a séparés.
///
/// **v71 — annexes du CGI comme instruments distincts** (école 2026-07-19) :
/// « article N de l'annexe X au/du CGI » cible le texte ANNEXE du catalogue
/// (LEGITEXT propres, I-IV), plus le CGI de base. 16 alias TSV (surfaces
/// d'automate) : la phrase « annexe III au code général des impôts » est
/// l'instrument reconnu, le génitif « de l' » rattache le numéral, les
/// énumérations héritent, résolution validée par existence (un lapsus de
/// cour — « 53 A de l'annexe III » inexistant — reste non résolu). Banc :
/// lien +34 hits, −25 spans instrument parasites, GT retargeté (68 flips)
/// + 4 spans gold oubliés réparés.
///
/// **v72 — clé pliée sur le cadre de quote à text_key vide** : la branche
/// « article DANS une quote » posait le `ref_num_key` en forme citable brute
/// (« L. 1233-30 ») quand le cadre était lui-même un article nu (résolu par
/// unicité, antécédent hors portée) — clés injoignables au catalogue (~560
/// lignes prod v70/v71). Passage par `num_key_for` comme toutes les autres
/// branches. Banc au bit près (la forme du num n'y est pas scorée).
pub const EXTRACT_VERSION: i16 = 72;

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
