//! Scan compilé du DOCUMENT (ADR 0156 volets 2-4, ADR 0157) : le vocabulaire
//! des valeurs est ouvert, mais les MARQUEURS qui structurent une décision
//! sont un vocabulaire fermé — formes sociales, têtes institutionnelles,
//! en-têtes de bloc (APPELANT/INTIMÉ), pivots du pourvoi CC, ouvertures de
//! requête admin, frontières de zone (motifs, dispositif), terminateurs de
//! nom, intros de conseil. Ce catalogue est compilé UNE fois en automate
//! Aho-Corasick leftmost-longest ; chaque document est scanné en UNE passe
//! sur texte plié longueur-stable (casse d'origine conservée pour les
//! contrôles de légitimité et les tranches verbatim), et les composeurs par
//! champ rejouent le même flux de tokens ([`DocScan`]).
//!
//! Zones par tokens, jamais par budget de chars (ADR 0157) : l'en-tête finit
//! au premier marqueur Motifs/Dispositif, le dispositif commence à son
//! marqueur. Gabarits de parties auto-détectés dans le flux :
//! - CC moderne : « <demandeurs> a/ont formé le/un pourvoi … l'opposant
//!   <défendeurs> … défendeurs à la cassation » ;
//! - CC ancien : « pourvoi formé par <demandeurs> contre l'arrêt … » ;
//! - CA/TJ/TCOM : blocs d'en-tête APPELANT/INTIMÉ (tous, pas le premier) ;
//! - admin : blocs de requête (« Par une requête…, X, …, demande »).
//!
//! Un nom d'entité = tranche VERBATIM entre un marqueur ouvrant (forme
//! sociale, « société » + qualificatifs, tête institutionnelle) et le premier
//! token fermant ou ponctuation de liste — plus aucune grammaire dispersée en
//! regex ; les regex résiduelles opèrent sur de petits spans positionnés par
//! les tokens.

use regex::Regex;
use std::sync::OnceLock;

use crate::compiled::{fold_stable, Norm};

// ── catalogue de marqueurs (surfaces PLIÉES : minuscules, sans accents) ─────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mk {
    /// En-tête de bloc demandeur (APPELANT, DEMANDEUR…) — exige la casse
    /// d'en-tête (tout en capitales, ou suivi de « : »).
    BlockApp,
    /// En-tête de bloc défendeur (INTIMÉ, DÉFENDEUR…).
    BlockDef,
    /// En-tête de bloc tiers (PARTIE INTERVENANTE…) : borne les segments ;
    /// récolté par `intervenors` seulement.
    BlockOther,
    /// Ouvreur de mémoire en intervention (admin) : l'intervenant suit.
    IntervIntro,
    /// Fin de zone parties (COMPOSITION DE LA COUR, DÉBATS…).
    Stop,
    /// Pivot CC moderne : « a/ont formé le/un pourvoi ».
    PivotNew,
    /// Pivot CC ancien : « pourvoi formé par ».
    PivotOld,
    /// « l'opposant », « en cassation contre » — début des défendeurs CC.
    Opposant,
    /// « contre l'arrêt/le jugement… » — fin des demandeurs (gabarit ancien).
    Contre,
    /// Fin de l'énumération des défendeurs CC (« défendeurs à la cassation »,
    /// « Sur le rapport »…) : au-delà, la prose cite les DEUX parties.
    DefEnd,
    /// Forme sociale de PARTIE (SAS, S.A.R.L.…) — exige les capitales.
    Form,
    /// « société » : le nom suit, après les qualificatifs.
    Societe,
    /// Qualificatif de forme consommé sans être capturé (« anonyme », « par
    /// actions simplifiée »…).
    Qualif,
    /// Tête institutionnelle : le marqueur + son complément EST le nom
    /// (« caisse primaire d'assurance maladie (CPAM) de l'Isère »).
    InstHead,
    /// Tête institutionnelle sigle (CPAM, CAF, AGS…) — exige les capitales.
    InstSigle,
    /// Structure d'avocats (SELARL, SCP…) : jamais une partie, sauf ès
    /// qualités (mandataire, liquidateur, notaire).
    LawStruct,
    /// Intro de conseil (« représenté par », « avocat au barreau »…) :
    /// termine un nom de partie, ouvre le contexte conseil.
    CounselIntro,
    /// « avocat de » : rattache une partie au conseil (filet CC).
    AvocatDe,
    /// « Me » / « Maître » (capitale exigée — « me » pronom exclu).
    Me,
    /// Terminateur de nom, toutes casses (« dont », « agissant », « N° SIRET »,
    /// civilités…) — les en-têtes de greffe les capitalisent.
    TrimAlways,
    /// Terminateur de nom en bas-de-casse UNIQUEMENT (verbes et articles de
    /// prose : « a », « est », « demande »…) — en capitales c'est du nom.
    TrimLower,
    /// Rôle indirect : l'entité qui SUIT n'est pas une partie (« aux droits de
    /// la société X » = prédécesseur, « en qualité d'assureur de Y » = assuré,
    /// « anciennement dénommée Z » = ancien nom).
    IndirectRole,
    /// Ouverture d'un bloc de requête admin (« Par une requête… », « Sous le
    /// n°… », « Vu la requête… ») : les requérants se nomment entre cette
    /// ouverture et leur verbe « demande(nt) ».
    AdminReq,
    /// Ouverture d'un mémoire admin (« Par un mémoire… ») : borne de segment,
    /// côté déterminé par le contenu (défense ou réplique du requérant).
    MemIntro,
    /// Ouverture d'un contexte défense admin (« mémoire en défense ») : les
    /// conseils qui suivent sont côté défendeur.
    DefIntro,
    /// Marqueur RÉTROACTIF de défense (« conclut au rejet ») : qualifie le
    /// mémoire qui le porte sans le borner (le conseil se nomme avant).
    DefConclu,
    /// « Moyen produit par <cabinet> … pour <partie> » (moyens annexés CC) :
    /// filet cabinet demandeur quand le préambule n'en nomme pas.
    MoyenPar,
    /// Début des motifs (« Considérant ce qui suit », « EXPOSÉ DU LITIGE »,
    /// « Faits et procédure ») : fin de l'en-tête — capitale initiale exigée
    /// (« en considérant que » de prose exclu).
    Motifs,
    /// Début du dispositif (« PAR CES MOTIFS », « DÉCIDE : », « D E C I D E »)
    /// — casse d'en-tête exigée (« le tribunal décide que » de prose exclu).
    Dispositif,
    /// Ancre de date d'audience (« audience », « débats » de prose — recasté
    /// au scan —, « plaidoiries », « débattue », « appelée ») : positionne
    /// les regex de date sur petites fenêtres au lieu du texte intégral.
    Audience,
    /// « joint les pourvois » (jonction CC) : ancre de la fenêtre où se
    /// lisent les numéros de pourvois joints.
    JointPourvois,
    /// Ouverture de la liste de visas admin (« Vu : », « Vu les autres
    /// pièces ») : au-delà, « sous le n° » cite d'AUTRES affaires (requête au
    /// fond d'un référé…), pas des requêtes jointes.
    VisaList,
    // ── issue du litige (lus dans la ZONE dispositif, ADR 0157) ──
    /// Désistement constaté : PRIME le label du greffe.
    OutDesist,
    /// Non-admission du pourvoi (filtrage cassation) : prime le label.
    OutNonAdmis,
    /// « n'y a pas/plus lieu de statuer » — l'exception « sur les dépens/
    /// frais » se vérifie sur le petit span suivant.
    OutNonLieu,
    /// Demande ACCUEILLIE sans verbe de condamnation : ouverture de
    /// procédure collective, sanction de gestion, mainlevée d'une mesure
    /// (JLD), ordonnance commune / prorogation en référé. Gold :
    /// SATISFACTION_*, pas AUTRE.
    OutGrant,
    /// Issue neutre → AUTRE : renvoi entre juridictions, incompétence,
    /// radiation, caducité/péremption, rectification, interruption,
    /// médiation/conciliation, mesures d'instruction, procédure collective.
    OutNeutral,
    /// Jonction : neutre mais testée en DERNIER (souvent jointe à une issue
    /// substantielle dans le même dispositif).
    OutJonction,
    /// Irrecevabilité (dont prescription/forclusion — garde « non/pas »
    /// vérifiée sur le petit span précédent).
    OutIrrec,
    /// « casse et annule ».
    OutCasse,
    /// Indice de partialité (« mais seulement », « sauf en ce que »,
    /// « partiellement », « statuant à nouveau de ce seul chef »…).
    OutPartial,
    /// « confirme » (l'apposition « partiellement/en partie » bascule en
    /// infirmation partielle).
    OutConfirme,
    /// « infirme / réforme / annule le jugement… ».
    OutInfirme,
    /// « sauf / à l'exception de » : partialité si dans la clause d'un
    /// confirme/infirme.
    OutSauf,
    /// « rejette / déboute / rejet ».
    OutRejette,
    /// « annule » (admin : satisfaction du requérant).
    OutAnnule,
    /// « condamne » — substantiel si « à payer/verser…/somme de/dommages »
    /// dans la clause, procédural si article 700/dépens seulement.
    OutCondamne,
    // ── signaux procéduraux (voie/office/domaine) ──
    /// « question prioritaire de constitutionnalité ».
    ProcQpc,
    /// Filtrage cassation au texte (« procédure préalable d'admission »,
    /// « l'admission est refusée », « non spécialement motivé », L822-1,
    /// R822-5).
    ProcPapc,
    /// R222-1 (filtrage ordonnances TA/CAA) — qualifié par proximité au
    /// compose (manifestement/irrecevable/tardive…).
    ProcFiltrage,
    /// L521-1 : référé suspension.
    ProcRefSusp,
    /// L521-2 : référé liberté.
    ProcRefLib,
    /// L521-3 : référé mesures utiles.
    ProcRefUtile,
    /// L551-*/L552-* ou « précontractuel » : référé précontractuel.
    ProcRefPrecontr,
    /// R541-4 : référé provision.
    ProcRefProv,
    /// Contexte droit des étrangers (CESEDA, OQTF…) : neutralise le faux
    /// « précontractuel » et route le domaine.
    ProcImmig,
    /// Rétention administrative (JLD).
    ProcRetention,
    /// Soins psychiatriques / hospitalisation sans consentement (JLD).
    ProcHospi,
    /// Requête en rectification/interprétation.
    ProcRectif,
    /// Juridiction du premier président (CC).
    ProcPremPres,
    /// Juge de l'exécution / saisie immobilière.
    ProcJex,
    /// Faux positif JEX (« compétence du juge de l'exécution et non… »).
    ProcJexFalse,
    /// Ouverture de procédure collective (en-tête TCOM).
    ProcCollective,
    /// « juge des référés de la cour » : référé d'appel L521-3, sans voie.
    ProcRefCour,
    /// « magistrat désigné » / « président désigné » (signature ou en-tête) :
    /// juge unique désigné (OQTF TA, ordonnances déléguées).
    ProcMagdes,
    /// « désigné M./Mme X » : formule ACTIVE de désignation en tête —
    /// « le président de la cour / du tribunal a désigné … pour statuer »
    /// (R. 222-1, référés L. 511-2). Contexte lu autour du token
    /// (`magdes_form_cour` / `magdes_form_trib`).
    ProcMagdesForm,
    /// Composition à juge unique dite par le texte (« statuant en juge
    /// unique », « siégeant seul ») — bloc de composition d'en-tête.
    ProcJugeUnique,
    /// « juge des référés » nu (le composé « … de la cour » garde son kind
    /// propre au leftmost-longest) — le contexte se lit autour du token
    /// (`jref_demande` / `jref_conseil`).
    ProcJref,
    /// Référé judiciaire dit par le texte : assignation en référé, appel /
    /// réforme / confirmation d'une ordonnance de référé, titre « ordonnance
    /// de référé » au bandeau (cf. `refere_civil`).
    ProcRefCivil,
    /// Domaine admin : fonction publique (agent public, fonctionnaire).
    ProcDomFp,
    /// Domaine admin : aide et action sociale (RSA, CASF).
    ProcDomAide,
    /// Domaine admin : urbanisme / immobilier public (permis de construire,
    /// PLU, expropriation, préemption).
    ProcDomUrba,
    /// Domaine admin : droit des étrangers hors surfaces OQTF/CESEDA
    /// (titre de séjour, asile, assignation à résidence).
    ProcDomEtr,
    /// Domaine admin : contentieux fiscal (impôts nommés, quand CGI/LPF ne
    /// sont pas cités).
    ProcDomFisc,
    /// Domaine admin : environnement (ICPE, police de l'eau, forêt) quand le
    /// code de l'environnement n'est pas cité.
    ProcDomEnv,
    /// Domaine admin : répression administrative (pénitentiaire, saisies
    /// d'armes, amendes administratives).
    ProcDomPenalPub,
    /// Domaines judiciaires par vocabulaire du corps — votes de TERMES
    /// comptés plein texte ([`DocScan::domain_term_votes`]), consommés par
    /// `domain::refine_with_terms` (raffinement d'un domaine nul ou parent
    /// nu, jamais d'un sous-domaine posé par les codes cités).
    ProcDomTravail,
    /// Vocabulaire travail valable dans les DEUX ordres (licenciement,
    /// heures supplémentaires…) : vote SOCIAL_DROIT_TRAVAIL en judiciaire,
    /// PUBLIC_DROIT_TRAVAIL en admin (un agent contractuel licencié plaide
    /// de la fonction publique) — contrairement à `ProcDomTravail`, dont le
    /// vocabulaire est propre au privé (salarié, prud'hommes, convention
    /// collective).
    ProcDomTravailMixte,
    /// AT/MP (accident du travail, maladie professionnelle, faute
    /// inexcusable, taux d'incapacité) : contentieux du travail dans les
    /// deux ordres (école gold, cf. plage CSS livre 4).
    ProcDomSecu,
    /// Cotisations & recouvrement (URSSAF) : SOCIAL nu — l'école gold en
    /// fait un produit terminal, ni prestation ni travail.
    ProcDomCotisations,
    ProcDomFamille,
    ProcDomDivorce,
    ProcDomSuccessions,
    ProcDomLocatif,
    ProcDomCopro,
    ProcDomConstruction,
    ProcDomSaisieImmo,
    ProcDomExecution,
    ProcDomAssurances,
    ProcDomBancaire,
    ProcDomResp,
    ProcDomEntDiff,
    ProcDomSocietes,
    ProcDomConcurrence,
    ProcDomConso,
    ProcDomPiLit,
    ProcDomPiInd,
}

#[rustfmt::skip]
const MARKERS: &[(&str, Mk)] = &[
    // ouvertures de bloc de requête admin
    ("par une requete", Mk::AdminReq), ("par deux requetes", Mk::AdminReq),
    ("par trois requetes", Mk::AdminReq), ("par un recours", Mk::AdminReq),
    ("par une protestation", Mk::AdminReq), ("par une reclamation", Mk::AdminReq),
    ("vu la requete", Mk::AdminReq), ("vu le recours", Mk::AdminReq),
    ("vu la protestation", Mk::AdminReq),
    ("sous le n", Mk::AdminReq), ("sous les n", Mk::AdminReq),
    ("sous le numero", Mk::AdminReq), ("sous les numeros", Mk::AdminReq),
    // en-têtes de bloc judiciaire
    ("appelant", Mk::BlockApp), ("appelante", Mk::BlockApp), ("appelants", Mk::BlockApp),
    ("appelantes", Mk::BlockApp), ("appelant(s)", Mk::BlockApp),
    ("demandeur", Mk::BlockApp), ("demanderesse", Mk::BlockApp),
    ("demandeurs", Mk::BlockApp), ("demanderesses", Mk::BlockApp), ("demandeur(s)", Mk::BlockApp),
    ("appelants et intimes incidents", Mk::BlockApp),
    ("appelantes et intimees incidentes", Mk::BlockApp),
    ("appelante et intimee incidente", Mk::BlockApp),
    ("appelant et intime incident", Mk::BlockApp),
    ("intime", Mk::BlockDef), ("intimee", Mk::BlockDef), ("intimes", Mk::BlockDef),
    ("intimees", Mk::BlockDef), ("intime(s)", Mk::BlockDef),
    ("defendeur", Mk::BlockDef), ("defenderesse", Mk::BlockDef),
    ("defendeurs", Mk::BlockDef), ("defenderesses", Mk::BlockDef), ("defendeur(s)", Mk::BlockDef),
    ("intimes et appelants incidents", Mk::BlockDef),
    ("intimees et appelantes incidentes", Mk::BlockDef),
    ("intimee et appelante incidente", Mk::BlockDef),
    ("intime et appelant incident", Mk::BlockDef),
    ("partie intervenante", Mk::BlockOther), ("parties intervenantes", Mk::BlockOther),
    ("intervenant", Mk::BlockOther), ("intervenante", Mk::BlockOther),
    ("intervenants", Mk::BlockOther), ("en presence de", Mk::BlockOther),
    ("autres parties", Mk::BlockOther),
    ("memoire en intervention", Mk::IntervIntro),
    ("intervention enregistree", Mk::IntervIntro),
    ("interventions presentees", Mk::IntervIntro),
    // fin de zone parties
    ("composition de la cour", Mk::Stop), ("composition du tribunal", Mk::Stop),
    ("debats", Mk::Stop), ("sur ce", Mk::Stop),
    ("greffier", Mk::Stop), ("president", Mk::Stop), ("presidente", Mk::Stop),
    ("date de cloture", Mk::Stop), ("l'affaire a ete debattue", Mk::Stop),
    ("ordonnance", Mk::Stop), ("arret", Mk::Stop), ("jugement", Mk::Stop),
    ("ministere public", Mk::Stop), ("decision deferee", Mk::Stop),
    ("decision attaquee", Mk::Stop),
    // frontières de zone : motifs (fin d'en-tête) et dispositif
    ("considerant ce qui suit", Mk::Motifs), ("considerant que", Mk::Motifs),
    ("expose du litige", Mk::Motifs), ("faits et procedure", Mk::Motifs),
    ("expose des faits", Mk::Motifs), ("motifs de la decision", Mk::Motifs),
    ("expose de la procedure", Mk::Motifs),
    ("par ces motifs", Mk::Dispositif), ("en consequence", Mk::Dispositif),
    ("decide", Mk::Dispositif),
    ("d e c i d e", Mk::Dispositif), ("ordonne", Mk::Dispositif),
    ("o r d o n n e", Mk::Dispositif), ("a r r e t e", Mk::Dispositif),
    ("arrete", Mk::Dispositif),
    // ancres de date d'audience (« debats » de prose y est recasté au scan)
    ("audience", Mk::Audience), ("debat", Mk::Audience),
    ("debattue", Mk::Audience), ("debattu", Mk::Audience),
    ("plaidoiries", Mk::Audience), ("plaidee", Mk::Audience),
    ("appelee", Mk::Audience),
    // jonction des pourvois CC
    ("joint les pourvois", Mk::JointPourvois), ("jointes les pourvois", Mk::JointPourvois),
    // ouverture de la liste de visas admin
    ("vu :", Mk::VisaList), ("vu:", Mk::VisaList),
    ("vu les autres pieces", Mk::VisaList),
    // pivots CC
    ("a forme le pourvoi", Mk::PivotNew), ("ont forme le pourvoi", Mk::PivotNew),
    ("a forme un pourvoi", Mk::PivotNew), ("ont forme un pourvoi", Mk::PivotNew),
    ("a forme les pourvois", Mk::PivotNew), ("ont forme les pourvois", Mk::PivotNew),
    ("ont forme respectivement les pourvois", Mk::PivotNew),
    ("pourvoi forme par", Mk::PivotOld), ("pourvois formes par", Mk::PivotOld),
    // jonction : « Statuant sur le pourvoi n° X 00-00.000 formé par : » — le
    // numéro interposé échappe aux surfaces « pourvoi formé par »
    ("statuant sur le pourvoi", Mk::PivotOld), ("statuant sur les pourvois", Mk::PivotOld),
    ("l'opposant", Mk::Opposant), ("les opposant", Mk::Opposant),
    ("en cassation contre", Mk::Opposant),
    // gabarit ancien : « en cassation d'un arrêt rendu … au profit de
    // <défendeur>, défenderesse à la cassation »
    ("au profit de", Mk::Opposant), ("au profit du", Mk::Opposant),
    ("au profit des", Mk::Opposant), ("au profit d'", Mk::Opposant),
    ("en cassation d'un arret", Mk::Contre), ("en cassation d'un jugement", Mk::Contre),
    ("contre l'arret", Mk::Contre), ("contre le jugement", Mk::Contre),
    ("contre l'ordonnance", Mk::Contre), ("contre la decision", Mk::Contre),
    ("contre un arret", Mk::Contre), ("contre un jugement", Mk::Contre),
    ("contre une ordonnance", Mk::Contre), ("contre deux arrets", Mk::Contre),
    // gabarit ancien : « …, en cassation d'un arrêt rendu …, au profit de … »
    ("en cassation d'", Mk::Contre),
    ("defendeur a la cassation", Mk::DefEnd), ("defendeurs a la cassation", Mk::DefEnd),
    ("defenderesse a la cassation", Mk::DefEnd), ("defenderesses a la cassation", Mk::DefEnd),
    ("vu la communication", Mk::DefEnd), ("sur le rapport", Mk::DefEnd),
    ("le demandeur invoque", Mk::DefEnd), ("la demanderesse invoque", Mk::DefEnd),
    ("les demandeurs invoquent", Mk::DefEnd), ("les demanderesses invoquent", Mk::DefEnd),
    ("apres deliberation", Mk::DefEnd),
    // gabarit ancien : le préambule débouche sans transition sur les moyens
    ("attendu que", Mk::DefEnd), ("attendu ,", Mk::DefEnd), ("attendu,", Mk::DefEnd),
    ("mais attendu", Mk::DefEnd), ("vu l'article", Mk::DefEnd),
    ("sur le moyen", Mk::DefEnd), ("sur le premier moyen", Mk::DefEnd),
    ("sur le second moyen", Mk::DefEnd), ("aux motifs", Mk::DefEnd),
    ("donne acte", Mk::DefEnd), ("statuant tant", Mk::DefEnd),
    // formes sociales de parties (capitales exigées)
    ("sas", Mk::Form), ("s.a.s.", Mk::Form), ("sasu", Mk::Form), ("s.a.s.u.", Mk::Form),
    ("sarl", Mk::Form), ("s.a.r.l.", Mk::Form), ("sarlu", Mk::Form),
    ("sa", Mk::Form), ("s.a.", Mk::Form), ("sau", Mk::Form), ("s.a.u.", Mk::Form),
    ("sci", Mk::Form), ("s.c.i.", Mk::Form), ("snc", Mk::Form), ("s.n.c.", Mk::Form),
    ("scea", Mk::Form), ("sca", Mk::Form), ("earl", Mk::Form), ("eurl", Mk::Form),
    ("eirl", Mk::Form), ("gie", Mk::Form), ("sem", Mk::Form), ("scop", Mk::Form),
    ("sccv", Mk::Form), ("sasp", Mk::Form), ("sea", Mk::Form),
    // « société » + qualificatifs
    ("societe", Mk::Societe),
    ("anonyme", Mk::Qualif), ("civile", Mk::Qualif), ("immobiliere", Mk::Qualif),
    ("par actions simplifiee", Mk::Qualif), ("unipersonnelle", Mk::Qualif),
    ("a responsabilite limitee", Mk::Qualif), ("d'exercice liberal", Mk::Qualif),
    ("en nom collectif", Mk::Qualif), ("cooperative", Mk::Qualif),
    ("mutualiste", Mk::Qualif), ("d'economie mixte", Mk::Qualif),
    ("en commandite simple", Mk::Qualif), ("en commandite par actions", Mk::Qualif),
    ("par actions", Mk::Qualif), ("a associe unique", Mk::Qualif),
    ("de droit", Mk::Qualif), // + un mot (nationalité) consommé au compose
    // têtes institutionnelles (le marqueur fait partie du nom)
    ("caisse", Mk::InstHead), ("mutuelle", Mk::InstHead), ("ligue", Mk::InstHead),
    ("fonds de garantie", Mk::InstHead), ("fonds commun", Mk::InstHead),
    ("association", Mk::InstHead), ("fondation", Mk::InstHead),
    ("syndicat", Mk::InstHead), ("banque", Mk::InstHead), ("groupement", Mk::InstHead),
    ("institut", Mk::InstHead), ("institution", Mk::InstHead),
    ("office public", Mk::InstHead), ("office national", Mk::InstHead),
    ("office francais", Mk::InstHead), ("office du tourisme", Mk::InstHead),
    ("clinique", Mk::InstHead), ("polyclinique", Mk::InstHead),
    ("centre hospitalier", Mk::InstHead), ("comite", Mk::InstHead),
    ("union des", Mk::InstHead), ("union de", Mk::InstHead), ("union d'", Mk::InstHead),
    ("federation", Mk::InstHead), ("confederation", Mk::InstHead),
    ("club", Mk::InstHead),
    ("etablissement public", Mk::InstHead), ("etablissement francais", Mk::InstHead),
    ("organisme", Mk::InstHead), ("compagnie", Mk::InstHead),
    ("pole emploi", Mk::InstHead),
    ("credit agricole", Mk::InstHead), ("credit mutuel", Mk::InstHead),
    ("credit lyonnais", Mk::InstHead), ("credit foncier", Mk::InstHead),
    ("credit industriel", Mk::InstHead), ("credit cooperatif", Mk::InstHead),
    ("aeroports de", Mk::InstHead), ("aeroport de", Mk::InstHead),
    ("ville de", Mk::InstHead), ("commune de", Mk::InstHead),
    ("departement de", Mk::InstHead), ("communaute de communes", Mk::InstHead),
    ("communaute d'agglomeration", Mk::InstHead), ("metropole de", Mk::InstHead),
    ("cpam", Mk::InstSigle), ("c.p.a.m.", Mk::InstSigle), ("crcam", Mk::InstSigle),
    ("caf", Mk::InstSigle), ("chu", Mk::InstSigle), ("chru", Mk::InstSigle),
    ("gaec", Mk::InstSigle), ("urssaf", Mk::InstSigle), ("unedic", Mk::InstSigle),
    ("ags", Mk::InstSigle), ("ags-cgea", Mk::InstSigle), ("cgea", Mk::InstSigle),
    ("sncf", Mk::InstSigle), ("ratp", Mk::InstSigle), ("edf", Mk::InstSigle),
    ("cavimac", Mk::InstSigle), ("epic", Mk::InstSigle),
    // structures d'avocats
    ("selarl", Mk::LawStruct), ("s.e.l.a.r.l.", Mk::LawStruct), ("selarlu", Mk::LawStruct),
    ("seleurl", Mk::LawStruct), ("selas", Mk::LawStruct), ("selafa", Mk::LawStruct),
    ("scp", Mk::LawStruct), ("s.c.p.", Mk::LawStruct), ("aarpi", Mk::LawStruct),
    ("scm", Mk::LawStruct), ("selca", Mk::LawStruct), ("cabinet", Mk::LawStruct),
    // intros de conseil
    ("represente par", Mk::CounselIntro), ("representee par", Mk::CounselIntro),
    ("representes par", Mk::CounselIntro), ("representees par", Mk::CounselIntro),
    ("representant", Mk::CounselIntro), ("representant :", Mk::CounselIntro),
    ("representants", Mk::CounselIntro), ("representant legal", Mk::CounselIntro),
    ("representants legaux", Mk::CounselIntro),
    ("assiste de", Mk::CounselIntro), ("assistee de", Mk::CounselIntro),
    ("assistes de", Mk::CounselIntro), ("assistees de", Mk::CounselIntro),
    ("assiste par", Mk::CounselIntro), ("assistee par", Mk::CounselIntro),
    ("assistes par", Mk::CounselIntro), ("assistees par", Mk::CounselIntro),
    // En-tête de greffe CA : « Rep/assistant : la SCP … » — l'intro de
    // conseil au plus près, la classification ne dépend plus d'un
    // « représentant légal » lointain en bord de fenêtre (60 chars).
    ("rep/assistant", Mk::CounselIntro), ("rep/assistants", Mk::CounselIntro),
    ("ayant pour avocat", Mk::CounselIntro), ("comparant par", Mk::CounselIntro),
    ("plaidant", Mk::CounselIntro), ("postulant", Mk::CounselIntro),
    ("substitue par", Mk::CounselIntro), ("substituee par", Mk::CounselIntro),
    ("avocat au barreau", Mk::CounselIntro), ("avocats au barreau", Mk::CounselIntro),
    ("avocat aux conseils", Mk::CounselIntro), ("avocats aux conseils", Mk::CounselIntro),
    ("avocat(s) :", Mk::CounselIntro), ("au cabinet de", Mk::CounselIntro),
    ("les observations de", Mk::CounselIntro), ("toque", Mk::CounselIntro),
    ("vestiaire", Mk::CounselIntro),
    ("memoire en defense", Mk::DefIntro), ("memoires en defense", Mk::DefIntro),
    ("par un memoire en defense", Mk::DefIntro), ("par des memoires en defense", Mk::DefIntro),
    ("par deux memoires en defense", Mk::DefIntro),
    ("par un memoire", Mk::MemIntro), ("par des memoires", Mk::MemIntro),
    ("par deux memoires", Mk::MemIntro), ("par trois memoires", Mk::MemIntro),
    ("par un nouveau memoire", Mk::MemIntro), ("par de nouveaux memoires", Mk::MemIntro),
    ("conclut au rejet", Mk::DefConclu), ("concluent au rejet", Mk::DefConclu),
    ("moyen produit par", Mk::MoyenPar), ("moyens produits par", Mk::MoyenPar),
    ("moyen annexe produit par", Mk::MoyenPar), ("moyens annexes produits par", Mk::MoyenPar),
    // en-tetes sans « produit par » (« MOYENS ANNEXES au present arret » puis
    // « moyens produits AU POURVOI PRINCIPAL par… ») — sans eux, les « en ce
    // qu'il » des moyens votent la partialite de la cassation
    ("moyen annexe au present arret", Mk::MoyenPar),
    ("moyens annexes au present arret", Mk::MoyenPar),
    ("moyens produits au pourvoi", Mk::MoyenPar),
    ("avocat de", Mk::AvocatDe), ("avocats de", Mk::AvocatDe), ("avocate de", Mk::AvocatDe),
    ("avocat du", Mk::AvocatDe), ("avocats du", Mk::AvocatDe), ("avocate du", Mk::AvocatDe),
    ("avocat des", Mk::AvocatDe), ("avocats des", Mk::AvocatDe),
    ("avocat d'", Mk::AvocatDe), ("avocats d'", Mk::AvocatDe),
    // terminateurs de nom — toutes casses (descripteurs de greffe)
    ("dont", Mk::TrimAlways), ("immatricule", Mk::TrimAlways), ("immatriculee", Mk::TrimAlways),
    ("immatricules", Mk::TrimAlways), ("immatriculees", Mk::TrimAlways),
    ("inscrit", Mk::TrimAlways), ("inscrite", Mk::TrimAlways),
    ("prise en la personne", Mk::TrimAlways), ("pris en la personne", Mk::TrimAlways),
    ("agissant", Mk::TrimAlways), ("venant", Mk::TrimAlways), ("ayant", Mk::TrimAlways),
    ("exercant", Mk::TrimAlways), ("anciennement", Mk::TrimAlways),
    ("domicilie", Mk::TrimAlways), ("domiciliee", Mk::TrimAlways),
    ("domicilies", Mk::TrimAlways), ("domiciliees", Mk::TrimAlways),
    ("demeurant", Mk::TrimAlways), ("elisant", Mk::TrimAlways),
    ("es qualite", Mk::TrimAlways), ("es qualites", Mk::TrimAlways),
    ("es-qualites", Mk::TrimAlways), ("aux droits", Mk::TrimAlways),
    ("aux droits de", Mk::IndirectRole), ("aux droits et obligations de", Mk::IndirectRole),
    ("anciennement denommee", Mk::IndirectRole), ("anciennement denomme", Mk::IndirectRole),
    ("en qualite d'assureur de", Mk::IndirectRole), ("assureur de", Mk::IndirectRole),
    ("venant aux droits de", Mk::IndirectRole),
    ("au capital", Mk::TrimAlways), ("au titre", Mk::TrimAlways),
    ("n° siret", Mk::TrimAlways), ("no siret", Mk::TrimAlways), ("rcs", Mk::TrimAlways),
    ("ne le", Mk::TrimAlways), ("nee le", Mk::TrimAlways),
    ("pour une duree", Mk::TrimAlways), ("en qualite", Mk::TrimAlways),
    ("en la personne", Mk::TrimAlways),
    // quantifieur : jamais dans un nom propre (« la commune de Richelieu
    // et plusieurs particuliers »)
    ("et plusieurs", Mk::TrimAlways),
    ("non comparant", Mk::TrimAlways), ("non comparante", Mk::TrimAlways),
    ("ni comparant", Mk::TrimAlways), ("ni comparante", Mk::TrimAlways),
    ("defaillant", Mk::TrimAlways), ("defaillante", Mk::TrimAlways),
    ("partie civile", Mk::TrimAlways), ("personne morale", Mk::TrimAlways),
    ("declaree", Mk::TrimAlways), ("declarees", Mk::TrimAlways),
    ("m.", Mk::TrimAlways), ("mme", Mk::TrimAlways), ("mlle", Mk::TrimAlways),
    ("mm.", Mk::TrimAlways), ("mmes", Mk::TrimAlways),
    ("monsieur", Mk::TrimAlways), ("madame", Mk::TrimAlways),
    ("mademoiselle", Mk::TrimAlways), ("veuve", Mk::TrimAlways),
    ("epoux", Mk::TrimAlways), ("epouse", Mk::TrimAlways),
    ("me", Mk::Me), ("maitre", Mk::Me),
    // terminateurs de nom — bas-de-casse seulement (prose)
    ("a", Mk::TrimLower), ("ont", Mk::TrimLower), ("est", Mk::TrimLower),
    ("sont", Mk::TrimLower), ("etait", Mk::TrimLower), ("avait", Mk::TrimLower),
    ("doit", Mk::TrimLower), ("demande", Mk::TrimLower), ("demandent", Mk::TrimLower),
    ("conclut", Mk::TrimLower), ("concluent", Mk::TrimLower),
    ("soutient", Mk::TrimLower), ("soutiennent", Mk::TrimLower),
    ("declare", Mk::TrimLower), ("declarent", Mk::TrimLower),
    ("sollicite", Mk::TrimLower), ("sollicitent", Mk::TrimLower),
    ("expose", Mk::TrimLower), ("exposent", Mk::TrimLower),
    ("reclame", Mk::TrimLower), ("reclament", Mk::TrimLower),
    ("releve", Mk::TrimLower), ("relevent", Mk::TrimLower),
    ("fait", Mk::TrimLower), ("s'est", Mk::TrimLower), ("n'a", Mk::TrimLower),
    ("qui", Mk::TrimLower), ("devant", Mk::TrimLower),
    ("tendant", Mk::TrimLower), ("dirige", Mk::TrimLower), ("dirigee", Mk::TrimLower),
    ("dirigees", Mk::TrimLower), ("en vue", Mk::TrimLower),
    ("la somme", Mk::TrimLower), ("une somme", Mk::TrimLower),
    // « se désiste » vit en OutDesist (issue) ET ferme les noms (closes-liste)
    ("les sommes", Mk::TrimLower),
    ("lui", Mk::TrimLower), ("sous", Mk::TrimLower),
    ("en application", Mk::TrimLower), ("en vertu", Mk::TrimLower),
    ("poursuites", Mk::TrimLower), ("un", Mk::TrimLower), ("une", Mk::TrimLower),
    ("et la", Mk::TrimLower), ("et le", Mk::TrimLower), ("et les", Mk::TrimLower),
    ("et l'", Mk::TrimLower), ("et de la", Mk::TrimLower), ("et de l'", Mk::TrimLower),
    ("et du", Mk::TrimLower), ("et des", Mk::TrimLower), ("et par la", Mk::TrimLower),
    ("et par", Mk::TrimLower), ("ainsi que", Mk::TrimLower),
    ("aux depens", Mk::TrimLower), ("in solidum", Mk::TrimLower),
    ("a la", Mk::TrimLower), ("a l'", Mk::TrimLower),
    ("a son", Mk::TrimLower), ("a sa", Mk::TrimLower), ("a ses", Mk::TrimLower),
    ("a leur", Mk::TrimLower), ("a leurs", Mk::TrimLower), ("prise", Mk::TrimLower),
    ("pris", Mk::TrimLower), ("dite", Mk::TrimLower),
    ("celle-ci", Mk::TrimLower),
    ("celui-ci", Mk::TrimLower), ("laquelle", Mk::TrimLower), ("lequel", Mk::TrimLower),
    ("elle", Mk::TrimLower), ("il", Mk::TrimLower), ("ne", Mk::TrimLower),
    ("n'est", Mk::TrimLower), ("d'un", Mk::TrimLower), ("d'une", Mk::TrimLower),
    // issue du litige — zone dispositif (les composeurs ignorent ces tokens
    // hors zone). Aucune surface ne commence par « ordonne/arrête/décide »
    // pour ne pas voler les marqueurs Dispositif au leftmost-longest.
    ("desistement", Mk::OutDesist), ("se desiste", Mk::OutDesist),
    ("n'est pas admis", Mk::OutNonAdmis), ("ne sont pas admis", Mk::OutNonAdmis),
    ("n'est pas admise", Mk::OutNonAdmis), ("ne sont pas admises", Mk::OutNonAdmis),
    ("pourvois non admis", Mk::OutNonAdmis),
    ("n'admet pas le pourvoi", Mk::OutNonAdmis), ("pourvoi non admis", Mk::OutNonAdmis),
    ("non-admission", Mk::OutNonAdmis), ("non admission", Mk::OutNonAdmis),
    ("n'y a pas lieu de statuer", Mk::OutNonLieu), ("n'y a plus lieu de statuer", Mk::OutNonLieu),
    ("n'y a pas lieu a statuer", Mk::OutNonLieu), ("n'y a plus lieu a statuer", Mk::OutNonLieu),
    ("n'y avoir lieu a statuer", Mk::OutNonLieu), ("n'y avoir lieu de statuer", Mk::OutNonLieu),
    // QPC non renvoyée (école gold Cc : NON_LIEU)
    ("n'y avoir lieu de renvoyer", Mk::OutNonLieu),
    ("n'y a pas lieu de renvoyer", Mk::OutNonLieu),
    ("non-lieu", Mk::OutNonLieu), ("non lieu a statuer", Mk::OutNonLieu),
    ("renvoie la cause", Mk::OutNeutral), ("reglant de juges", Mk::OutNeutral),
    ("renvoie devant", Mk::OutNeutral), ("renvoie les parties", Mk::OutNeutral),
    ("incompetent", Mk::OutNeutral), ("incompetente", Mk::OutNeutral),
    ("incompetence", Mk::OutNeutral),
    ("radiation", Mk::OutNeutral), ("est radiee", Mk::OutNeutral),
    ("caducite", Mk::OutNeutral), ("caduque", Mk::OutNeutral), ("caduc", Mk::OutNeutral),
    ("peremption", Mk::OutNeutral),
    ("rectification", Mk::OutNeutral), ("rectifie l'arret", Mk::OutNeutral),
    ("il faut lire", Mk::OutNeutral), ("sera rectifie", Mk::OutNeutral),
    ("interruption de l'instance", Mk::OutNeutral), ("constate l'interruption", Mk::OutNeutral),
    ("mediation", Mk::OutNeutral), ("mediateur", Mk::OutNeutral),
    ("conciliateur", Mk::OutNeutral), ("conciliation", Mk::OutNeutral),
    ("expertise", Mk::OutNeutral), ("avant dire droit", Mk::OutNeutral),
    ("avant-dire droit", Mk::OutNeutral),
    ("juge-commissaire", Mk::OutNeutral), ("juge commissaire", Mk::OutNeutral),
    ("plan de redressement", Mk::OutNeutral), ("plan de sauvegarde", Mk::OutNeutral),
    ("plan de cession", Mk::OutNeutral), ("periode d'observation", Mk::OutNeutral),
    ("cessation des paiements", Mk::OutNeutral),
    ("homologue", Mk::OutNeutral), ("homologation", Mk::OutNeutral),
    ("interpretation", Mk::OutNeutral),
    // Octrois sans condamnation (« prononce la liquidation » est absent à
    // dessein : la surface éclipserait « liquidation judiciaire »
    // (ProcCollective) en leftmost-longest ; « ouvre la/une procédure »
    // s'arrête AVANT elle)
    ("ouvre la procedure", Mk::OutGrant), ("ouvre une procedure", Mk::OutGrant),
    ("interdiction de gerer", Mk::OutGrant), ("faillite personnelle", Mk::OutGrant),
    ("renouvelle la periode d'observation", Mk::OutGrant),
    ("proroge la periode d'observation", Mk::OutGrant),
    ("mainlevee de la mesure", Mk::OutGrant),
    ("rendons commune", Mk::OutGrant), ("rendons communes", Mk::OutGrant),
    ("rend commune", Mk::OutGrant),
    ("proroge le delai", Mk::OutGrant), ("prorogeons le delai", Mk::OutGrant),
    ("la jonction", Mk::OutJonction), ("joint les causes", Mk::OutJonction),
    ("joint les instances", Mk::OutJonction), ("joint les affaires", Mk::OutJonction),
    ("joint les procedures", Mk::OutJonction), ("joint les dossiers", Mk::OutJonction),
    ("irrecevable", Mk::OutIrrec), ("irrecevables", Mk::OutIrrec),
    ("irrecevabilite", Mk::OutIrrec),
    ("prescrit", Mk::OutIrrec), ("prescrite", Mk::OutIrrec), ("prescrites", Mk::OutIrrec),
    ("forclos", Mk::OutIrrec), ("forclose", Mk::OutIrrec),
    ("casse et annule", Mk::OutCasse),
    ("mais seulement", Mk::OutPartial), ("sauf en ce qu", Mk::OutPartial),
    ("en ce qu'il", Mk::OutPartial), ("en ce qu'elle", Mk::OutPartial),
    ("en ses seules dispositions", Mk::OutPartial), ("partiellement", Mk::OutPartial),
    ("en partie", Mk::OutPartial), ("statuant a nouveau de ce seul chef", Mk::OutPartial),
    // annulation bornée (CE : « annulée en tant qu'elle a statué sur… »,
    // renvoi « dans cette mesure ») et cassation par retranchement
    ("en tant qu'il", Mk::OutPartial), ("en tant qu'elle", Mk::OutPartial),
    ("dans cette mesure", Mk::OutPartial),
    ("dans ses seules dispositions", Mk::OutPartial),
    ("en ses dispositions", Mk::OutPartial),
    // passif fléchi (« la décision est confirmée/infirmée/annulée ») : le
    // masculin singulier passe par le pliage d'accent (« confirmé » →
    // « confirme »), les formes féminines/plurielles ont leur surface propre
    ("confirme", Mk::OutConfirme), ("confirmons", Mk::OutConfirme),
    ("confirment", Mk::OutConfirme), ("confirmee", Mk::OutConfirme),
    ("confirmees", Mk::OutConfirme), ("confirmes", Mk::OutConfirme),
    ("infirme", Mk::OutInfirme), ("infirmons", Mk::OutInfirme),
    ("infirment", Mk::OutInfirme), ("infirmant", Mk::OutInfirme),
    ("infirmee", Mk::OutInfirme), ("infirmees", Mk::OutInfirme),
    ("infirmes", Mk::OutInfirme),
    ("reforme", Mk::OutInfirme), ("reformons", Mk::OutInfirme),
    ("reforment", Mk::OutInfirme), ("reformant", Mk::OutInfirme),
    ("reformee", Mk::OutInfirme), ("reformees", Mk::OutInfirme),
    ("annule le jugement", Mk::OutInfirme), ("annule l'ordonnance", Mk::OutInfirme),
    ("annule la contrainte", Mk::OutInfirme), ("annule l'arret", Mk::OutInfirme),
    ("annule la decision", Mk::OutInfirme),
    ("sauf", Mk::OutSauf), ("a l'exception de", Mk::OutSauf),
    ("rejette", Mk::OutRejette), ("rejetons", Mk::OutRejette), ("rejettent", Mk::OutRejette),
    ("rejet", Mk::OutRejette), ("rejete", Mk::OutRejette), ("rejetee", Mk::OutRejette),
    ("rejetes", Mk::OutRejette), ("rejetees", Mk::OutRejette),
    ("rejeter", Mk::OutRejette), // infinitif des motifs (« il y a lieu de rejeter »)
    ("deboute", Mk::OutRejette), ("deboutons", Mk::OutRejette), ("deboutent", Mk::OutRejette),
    ("annule", Mk::OutAnnule), ("annulons", Mk::OutAnnule),
    ("annulee", Mk::OutAnnule), ("annulees", Mk::OutAnnule),
    ("annules", Mk::OutAnnule),
    ("condamne", Mk::OutCondamne), ("condamnons", Mk::OutCondamne),
    // signaux procéduraux (les articles L521-*/R222-1… sont générés au build
    // du vocabulaire, cf. `vocab()`)
    ("question prioritaire de constitutionnalite", Mk::ProcQpc),
    ("procedure prealable d'admission", Mk::ProcPapc),
    ("l'admission est refusee", Mk::ProcPapc),
    ("non specialement motive", Mk::ProcPapc), ("non specialement motives", Mk::ProcPapc),
    ("precontractuel", Mk::ProcRefPrecontr),
    ("code de l'entree et du sejour", Mk::ProcImmig), ("ceseda", Mk::ProcImmig),
    ("reconduite a la frontiere", Mk::ProcImmig),
    ("obligation de quitter le territoire", Mk::ProcImmig),
    ("centre de retention administrative", Mk::ProcRetention),
    ("retention administrative", Mk::ProcRetention),
    ("maintien en retention", Mk::ProcRetention),
    ("prolongation de la retention", Mk::ProcRetention),
    ("soins psychiatriques", Mk::ProcHospi), ("soin psychiatrique", Mk::ProcHospi),
    ("hospitalisation complete", Mk::ProcHospi),
    ("hospitalisation sans consentement", Mk::ProcHospi),
    ("requete en rectification", Mk::ProcRectif),
    ("requete aux fins de rectification", Mk::ProcRectif),
    ("requete en interpretation", Mk::ProcRectif),
    ("rectification d'erreur materielle", Mk::ProcRectif),
    // PAS de « premier président » nu : toute composition de CA en cite un
    // (président de chambre) — seules les formes de saisine comptent.
    ("premiere presidence", Mk::ProcPremPres),
    ("juridiction du premier president", Mk::ProcPremPres),
    ("premier president de la cour", Mk::ProcPremPres),
    ("juge de l'execution", Mk::ProcJex),
    ("saisie immobiliere", Mk::ProcJex), ("saisies immobilieres", Mk::ProcJex),
    ("juge de l'execution et non", Mk::ProcJexFalse),
    ("laisser au juge de l'execution", Mk::ProcJexFalse),
    ("magistrat designe", Mk::ProcMagdes), ("magistrate designee", Mk::ProcMagdes),
    ("president designe", Mk::ProcMagdes), ("presidente designee", Mk::ProcMagdes),
    ("conseiller d'etat designe", Mk::ProcMagdes),
    ("conseillere d'etat designee", Mk::ProcMagdes),
    ("designe mme", Mk::ProcMagdesForm), ("designe m.", Mk::ProcMagdesForm),
    // magistrat délégué judiciaire : signature en pied des ordonnances de
    // mise en état / d'instruction CA-TJ (cf. `magdes_tail`)
    ("juge de la mise en etat", Mk::ProcMagdes),
    ("magistrat de la mise en etat", Mk::ProcMagdes),
    ("conseiller de la mise en etat", Mk::ProcMagdes),
    ("conseillere de la mise en etat", Mk::ProcMagdes),
    ("magistrat charge d'instruire", Mk::ProcMagdes),
    ("juge unique", Mk::ProcJugeUnique),
    ("statuant seul", Mk::ProcJugeUnique), ("statuant seule", Mk::ProcJugeUnique),
    ("siegeant seul", Mk::ProcJugeUnique), ("siegeant seule", Mk::ProcJugeUnique),
    ("magistrat unique", Mk::ProcJugeUnique),
    // domaines admin par vocabulaire d'en-tête (les codes substantiels ne
    // sont presque jamais cités dans ces contentieux — le CJA seul vote)
    ("fonction publique", Mk::ProcDomFp), ("fonctionnaire", Mk::ProcDomFp),
    ("fonctionnaires", Mk::ProcDomFp), ("agent public", Mk::ProcDomFp),
    ("agents publics", Mk::ProcDomFp), ("titularisation", Mk::ProcDomFp),
    ("conseil de discipline", Mk::ProcDomFp),
    ("protection fonctionnelle", Mk::ProcDomFp),
    ("regime indemnitaire", Mk::ProcDomFp), ("mutation d'office", Mk::ProcDomFp),
    ("commission de reforme", Mk::ProcDomFp),
    ("agent titulaire", Mk::ProcDomFp), ("agents titulaires", Mk::ProcDomFp),
    ("agent stagiaire", Mk::ProcDomFp), ("avancement de grade", Mk::ProcDomFp),
    ("imputabilite au service", Mk::ProcDomFp),
    ("imputable au service", Mk::ProcDomFp),
    ("suspendu de ses fonctions", Mk::ProcDomFp),
    ("suspendue de ses fonctions", Mk::ProcDomFp),
    ("suspension de fonctions", Mk::ProcDomFp),
    ("echelon", Mk::ProcDomFp), ("reclassement", Mk::ProcDomFp),
    ("exclusion temporaire de fonctions", Mk::ProcDomFp),
    ("conge de maladie", Mk::ProcDomFp), ("conge de longue", Mk::ProcDomFp),
    ("commission administrative paritaire", Mk::ProcDomFp),
    ("nouvelle bonification indiciaire", Mk::ProcDomFp),
    ("abandon de poste", Mk::ProcDomFp), ("cadre d'emplois", Mk::ProcDomFp),
    ("agent contractuel", Mk::ProcDomFp), ("agents contractuels", Mk::ProcDomFp),
    ("agente contractuelle", Mk::ProcDomFp),
    ("radiation des cadres", Mk::ProcDomFp),
    ("temps partiel therapeutique", Mk::ProcDomFp),
    ("enseignant", Mk::ProcDomFp), ("enseignante", Mk::ProcDomFp),
    ("commission de recours des militaires", Mk::ProcDomFp),
    ("france travail", Mk::ProcDomFp), ("pole emploi", Mk::ProcDomFp),
    // pensions publiques (CPCMR), concours de recrutement, statuts
    // hospitaliers, préretraite amiante des agents
    ("pension de retraite", Mk::ProcDomFp), ("pensions de retraite", Mk::ProcDomFp),
    // CPCMR seul : « pensions militaires d'invalidité » (CPMIVG, victimes
    // de guerre) reste PUBLIC nu au gold
    ("pensions civiles et militaires", Mk::ProcDomFp),
    ("concours de recrutement", Mk::ProcDomFp),
    ("concours complementaire", Mk::ProcDomFp),
    ("praticien hospitalier", Mk::ProcDomFp), ("praticiens hospitaliers", Mk::ProcDomFp),
    ("exposition professionnelle", Mk::ProcDomFp),
    ("accident de service", Mk::ProcDomFp),
    ("sanction de blame", Mk::ProcDomFp),
    ("deplacement d'office", Mk::ProcDomFp),
    ("concours professionnel", Mk::ProcDomFp),
    ("gestion de sa carriere", Mk::ProcDomFp),
    ("indemnite de logement", Mk::ProcDomFp),
    ("revenu de solidarite active", Mk::ProcDomAide),
    ("code de l'action sociale et des familles", Mk::ProcDomAide),
    ("aide sociale", Mk::ProcDomAide),
    ("commission de mediation", Mk::ProcDomAide),
    ("droit au logement opposable", Mk::ProcDomAide),
    ("hebergement d'urgence", Mk::ProcDomAide),
    // pas de « caisse primaire d'assurance maladie » : la surface volerait
    // au leftmost-longest l'entité CPAM des extracteurs de parties
    ("pension d'invalidite", Mk::ProcDomAide),
    ("allocation aux adultes handicapes", Mk::ProcDomAide),
    ("allocation personnalisee d'autonomie", Mk::ProcDomAide),
    ("allocation de solidarite", Mk::ProcDomAide),
    ("allocations familiales", Mk::ProcDomAide),
    ("aide personnalisee au logement", Mk::ProcDomAide),
    ("allocation de retour a l'emploi", Mk::ProcDomAide),
    ("aide sociale a l'enfance", Mk::ProcDomAide),
    ("prestation de compensation du handicap", Mk::ProcDomAide),
    ("fonds de solidarite pour le logement", Mk::ProcDomAide),
    ("carte mobilite inclusion", Mk::ProcDomAide),
    ("bourse sur criteres sociaux", Mk::ProcDomAide),
    ("attribuer un logement", Mk::ProcDomAide),
    ("retraite du combattant", Mk::ProcDomAide),
    ("indemnites journalieres", Mk::ProcDomAide),
    ("permis de construire", Mk::ProcDomUrba),
    ("plan local d'urbanisme", Mk::ProcDomUrba),
    ("code de l'urbanisme", Mk::ProcDomUrba),
    ("expropriation", Mk::ProcDomUrba), ("preemption", Mk::ProcDomUrba),
    ("declaration prealable", Mk::ProcDomUrba),
    ("permis d'amenager", Mk::ProcDomUrba), ("permis de demolir", Mk::ProcDomUrba),
    ("certificat d'urbanisme", Mk::ProcDomUrba),
    ("taxe d'amenagement", Mk::ProcDomUrba),
    ("domaine public", Mk::ProcDomUrba),
    ("travaux publics", Mk::ProcDomUrba), ("ouvrage public", Mk::ProcDomUrba),
    ("amenagement commercial", Mk::ProcDomUrba),
    ("amenagement cinematographique", Mk::ProcDomUrba),
    ("grande voirie", Mk::ProcDomUrba), ("menacant ruine", Mk::ProcDomUrba),
    ("titre de sejour", Mk::ProcDomEtr), ("carte de sejour", Mk::ProcDomEtr),
    ("certificat de residence", Mk::ProcDomEtr),
    ("quitter le territoire", Mk::ProcDomEtr),
    ("assignation a residence", Mk::ProcDomEtr),
    ("assigne a residence", Mk::ProcDomEtr),
    ("assignee a residence", Mk::ProcDomEtr),
    ("assignant a residence", Mk::ProcDomEtr),
    ("demande d'asile", Mk::ProcDomEtr), ("demandeur d'asile", Mk::ProcDomEtr),
    ("regroupement familial", Mk::ProcDomEtr),
    ("naturalisation", Mk::ProcDomEtr),
    ("statut de refugie", Mk::ProcDomEtr),
    ("asile", Mk::ProcDomEtr),
    ("refugie", Mk::ProcDomEtr), ("refugiee", Mk::ProcDomEtr),
    ("refugies", Mk::ProcDomEtr), ("refugiees", Mk::ProcDomEtr),
    ("apatride", Mk::ProcDomEtr), ("apatrides", Mk::ProcDomEtr),
    ("apatridie", Mk::ProcDomEtr),
    ("interdiction de retour", Mk::ProcDomEtr),
    ("nationalite francaise", Mk::ProcDomEtr),
    ("office francais de protection", Mk::ProcDomEtr),
    ("office francais de l'immigration", Mk::ProcDomEtr),
    ("conditions materielles d'accueil", Mk::ProcDomEtr),
    ("laissez-passer", Mk::ProcDomEtr),
    ("refus de visa", Mk::ProcDomEtr),
    ("extradition", Mk::ProcDomEtr),
    ("impot sur le revenu", Mk::ProcDomFisc),
    ("impot sur les societes", Mk::ProcDomFisc),
    ("taxe sur la valeur ajoutee", Mk::ProcDomFisc),
    ("taxe fonciere", Mk::ProcDomFisc), ("taxe d'habitation", Mk::ProcDomFisc),
    ("cotisation fonciere des entreprises", Mk::ProcDomFisc),
    ("saisie administrative a tiers detenteur", Mk::ProcDomFisc),
    ("avis a tiers detenteur", Mk::ProcDomFisc),
    ("credit d'impot", Mk::ProcDomFisc),
    ("comptable public", Mk::ProcDomFisc),
    ("fonds de solidarite a destination des entreprises", Mk::ProcDomFisc),
    ("installation classee", Mk::ProcDomEnv),
    ("installations classees", Mk::ProcDomEnv),
    ("autorisation environnementale", Mk::ProcDomEnv),
    ("affichage environnemental", Mk::ProcDomEnv),
    ("depollution", Mk::ProcDomEnv), ("zone humide", Mk::ProcDomEnv),
    ("methanisation", Mk::ProcDomEnv),
    ("regime forestier", Mk::ProcDomEnv), ("defrichement", Mk::ProcDomEnv),
    ("espece protegee", Mk::ProcDomEnv), ("especes protegees", Mk::ProcDomEnv),
    ("natura 2000", Mk::ProcDomEnv),
    ("prairies permanentes", Mk::ProcDomEnv),
    ("conditions de detention", Mk::ProcDomPenalPub),
    ("administration penitentiaire", Mk::ProcDomPenalPub),
    ("amende administrative", Mk::ProcDomPenalPub),
    ("amendes administratives", Mk::ProcDomPenalPub),
    ("saisie definitive d'armes", Mk::ProcDomPenalPub),
    ("saisie definitive de ses armes", Mk::ProcDomPenalPub),
    ("saisie definitive des armes", Mk::ProcDomPenalPub),
    ("jugement d'ouverture", Mk::ProcCollective),
    ("redressement judiciaire", Mk::ProcCollective),
    ("liquidation judiciaire", Mk::ProcCollective),
    ("juge des referes de la cour", Mk::ProcRefCour),
    ("juge des referes", Mk::ProcJref),
    ("assignation en refere", Mk::ProcRefCivil),
    ("ordonnance de refere", Mk::ProcRefCivil),
    // domaines judiciaires par vocabulaire du corps (votes de termes,
    // domain::refine_with_terms) — termes canoniques de la matière ; pas de
    // noms de parties institutionnelles (URSSAF, CPAM, syndicat des
    // copropriétaires…), qui voleraient les entités NER au leftmost-longest
    ("licenciement", Mk::ProcDomTravailMixte),
    ("licenciements", Mk::ProcDomTravailMixte),
    ("contrat de travail", Mk::ProcDomTravail),
    ("contrats de travail", Mk::ProcDomTravail),
    ("conseil de prud'hommes", Mk::ProcDomTravail),
    ("prud'homale", Mk::ProcDomTravail),
    ("salarie", Mk::ProcDomTravail), ("salariee", Mk::ProcDomTravail),
    ("salaries", Mk::ProcDomTravail), ("salariees", Mk::ProcDomTravail),
    ("heures supplementaires", Mk::ProcDomTravailMixte),
    ("rupture conventionnelle", Mk::ProcDomTravailMixte),
    ("indemnite de preavis", Mk::ProcDomTravailMixte),
    ("harcelement moral", Mk::ProcDomTravailMixte),
    ("temps de travail", Mk::ProcDomTravailMixte),
    ("convention collective", Mk::ProcDomTravail),
    ("accident du travail", Mk::ProcDomSecu),
    ("maladie professionnelle", Mk::ProcDomSecu),
    ("faute inexcusable", Mk::ProcDomSecu),
    ("cotisations sociales", Mk::ProcDomCotisations),
    ("taux d'incapacite", Mk::ProcDomSecu),
    ("autorite parentale", Mk::ProcDomFamille), ("filiation", Mk::ProcDomFamille),
    ("tutelle", Mk::ProcDomFamille), ("curatelle", Mk::ProcDomFamille),
    ("pension alimentaire", Mk::ProcDomFamille),
    ("residence de l'enfant", Mk::ProcDomFamille),
    ("droit de visite", Mk::ProcDomFamille),
    ("juge aux affaires familiales", Mk::ProcDomFamille),
    ("divorce", Mk::ProcDomDivorce),
    ("prestation compensatoire", Mk::ProcDomDivorce),
    ("separation de corps", Mk::ProcDomDivorce),
    ("succession", Mk::ProcDomSuccessions), ("successions", Mk::ProcDomSuccessions),
    ("heritier", Mk::ProcDomSuccessions), ("heritiers", Mk::ProcDomSuccessions),
    ("testament", Mk::ProcDomSuccessions),
    ("indivision successorale", Mk::ProcDomSuccessions),
    ("reserve hereditaire", Mk::ProcDomSuccessions),
    ("bail", Mk::ProcDomLocatif), ("baux", Mk::ProcDomLocatif),
    ("bailleur", Mk::ProcDomLocatif), ("bailleresse", Mk::ProcDomLocatif),
    ("bailleurs", Mk::ProcDomLocatif),
    ("locataire", Mk::ProcDomLocatif), ("locataires", Mk::ProcDomLocatif),
    ("loyer", Mk::ProcDomLocatif), ("loyers", Mk::ProcDomLocatif),
    ("clause resolutoire", Mk::ProcDomLocatif), ("preneur", Mk::ProcDomLocatif),
    ("indemnite d'occupation", Mk::ProcDomLocatif),
    ("trouble de jouissance", Mk::ProcDomLocatif),
    ("copropriete", Mk::ProcDomCopro), ("coproprietaire", Mk::ProcDomCopro),
    ("coproprietaires", Mk::ProcDomCopro), ("parties communes", Mk::ProcDomCopro),
    ("reglement de copropriete", Mk::ProcDomCopro),
    ("servitude", Mk::ProcDomCopro), ("mitoyennete", Mk::ProcDomCopro),
    ("bornage", Mk::ProcDomCopro),
    ("garantie decennale", Mk::ProcDomConstruction),
    ("maitre d'ouvrage", Mk::ProcDomConstruction),
    ("maitre de l'ouvrage", Mk::ProcDomConstruction),
    ("malfacons", Mk::ProcDomConstruction),
    ("reception des travaux", Mk::ProcDomConstruction),
    ("dommages-ouvrage", Mk::ProcDomConstruction),
    ("adjudication", Mk::ProcDomSaisieImmo),
    ("audience d'orientation", Mk::ProcDomSaisieImmo),
    ("commandement de payer valant saisie", Mk::ProcDomSaisieImmo),
    ("cahier des conditions de vente", Mk::ProcDomSaisieImmo),
    ("saisie-attribution", Mk::ProcDomExecution),
    ("saisie attribution", Mk::ProcDomExecution),
    ("mainlevee", Mk::ProcDomExecution), ("titre executoire", Mk::ProcDomExecution),
    ("saisie des remunerations", Mk::ProcDomExecution),
    ("assureur", Mk::ProcDomAssurances), ("assureurs", Mk::ProcDomAssurances),
    ("police d'assurance", Mk::ProcDomAssurances),
    ("contrat d'assurance", Mk::ProcDomAssurances),
    ("sinistre", Mk::ProcDomAssurances),
    ("decheance du terme", Mk::ProcDomBancaire),
    ("cautionnement", Mk::ProcDomBancaire),
    ("pret immobilier", Mk::ProcDomBancaire),
    ("credit immobilier", Mk::ProcDomBancaire),
    ("offre de pret", Mk::ProcDomBancaire),
    ("caution solidaire", Mk::ProcDomBancaire),
    ("solde debiteur", Mk::ProcDomBancaire),
    ("tableau d'amortissement", Mk::ProcDomBancaire),
    ("prejudice corporel", Mk::ProcDomResp),
    ("perte de chance", Mk::ProcDomResp),
    ("responsabilite delictuelle", Mk::ProcDomResp),
    ("deficit fonctionnel", Mk::ProcDomResp),
    ("procedure collective", Mk::ProcDomEntDiff),
    ("procedures collectives", Mk::ProcDomEntDiff),
    ("administrateur judiciaire", Mk::ProcDomEntDiff),
    ("juge-commissaire", Mk::ProcDomEntDiff), ("juge commissaire", Mk::ProcDomEntDiff),
    ("plan de cession", Mk::ProcDomEntDiff),
    ("declaration de creance", Mk::ProcDomEntDiff),
    ("insuffisance d'actif", Mk::ProcDomEntDiff),
    ("etat des creances", Mk::ProcDomEntDiff),
    ("parts sociales", Mk::ProcDomSocietes),
    ("cession de parts", Mk::ProcDomSocietes),
    ("assemblee generale des associes", Mk::ProcDomSocietes),
    ("commissaire aux comptes", Mk::ProcDomSocietes),
    ("compte courant d'associe", Mk::ProcDomSocietes),
    ("concurrence deloyale", Mk::ProcDomConcurrence),
    ("parasitisme", Mk::ProcDomConcurrence),
    ("pratiques anticoncurrentielles", Mk::ProcDomConcurrence),
    ("rupture brutale des relations commerciales", Mk::ProcDomConcurrence),
    ("consommateur", Mk::ProcDomConso), ("consommateurs", Mk::ProcDomConso),
    ("credit a la consommation", Mk::ProcDomConso),
    ("clause abusive", Mk::ProcDomConso), ("clauses abusives", Mk::ProcDomConso),
    ("surendettement", Mk::ProcDomConso),
    ("droit d'auteur", Mk::ProcDomPiLit),
    ("oeuvre de l'esprit", Mk::ProcDomPiLit),
    ("contrefacon", Mk::ProcDomPiInd), ("brevet", Mk::ProcDomPiInd),
    ("brevets", Mk::ProcDomPiInd), ("marque deposee", Mk::ProcDomPiInd),
];

/// Articles procéduraux (CJA) : la lettre + le numéro se déclinent en
/// variantes de surface (« L. 521-2 », « L.521-2 », « L 521-2 », « L521-2 »,
/// tiret ou tiret demi-cadratin) au build du vocabulaire. La frontière de
/// mot du scan rend gratuite la garde `(?!\d)` (« R222-11 » ne matche pas).
const PROC_ARTICLES: &[(&str, &str, Mk)] = &[
    ("l", "521-1", Mk::ProcRefSusp),
    ("l", "521-2", Mk::ProcRefLib),
    ("l", "521-3", Mk::ProcRefUtile),
    ("l", "551-", Mk::ProcRefPrecontr),
    ("l", "552-", Mk::ProcRefPrecontr),
    ("r", "541-4", Mk::ProcRefProv),
    ("r", "222-1", Mk::ProcFiltrage),
    ("l", "822-1", Mk::ProcPapc),
    ("r", "822-5", Mk::ProcPapc),
];

/// Surfaces marqueurs (pliées) + leur type — consommées par l'automate
/// FUSIONNÉ de `compiled` (un seul scan par document, ADR 0156/0158).
pub(crate) fn marker_patterns() -> (Vec<String>, Vec<Mk>) {
    let mut surfaces: Vec<String> = MARKERS.iter().map(|(s, _)| s.to_string()).collect();
    let mut kinds: Vec<Mk> = MARKERS.iter().map(|(_, k)| *k).collect();
    for (letter, num, kind) in PROC_ARTICLES {
        for sep in [". ", ".", " ", ""] {
            for dash in ['-', '\u{2013}'] {
                surfaces.push(format!(
                    "{letter}{sep}{}",
                    num.replace('-', &dash.to_string())
                ));
                kinds.push(*kind);
            }
        }
    }
    (surfaces, kinds)
}

// ── scan : une passe automate + lexer placeholders ──────────────────────────

/// Token positionné (offsets en CHARS du texte collapsé).
#[derive(Clone, Debug)]
pub(crate) struct PTok {
    s: usize,
    e: usize,
    kind: Mk,
}

/// Résultat du scan d'un texte : le flux de tokens + la préparation du
/// document ([`Norm`] : chars d'origine sur lesquels les spans s'appliquent,
/// texte plié, tables byte ↔ char).
pub struct DocScan {
    norm: Norm,
    toks: Vec<PTok>,
    // mémo : le chemin prod projette 7 champs NER depuis le même scan
    // (companies ×2, counsel ×4, intervenors) — le registre de parties
    // (ADR 0175 V0) se construit une fois, les champs en sont des vues.
    registry_memo: std::cell::OnceCell<crate::registry::PartyRegistry>,
}

/// Gabarit structurel d'en-tête, partagé par tous les composeurs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gabarit {
    /// Préambule de pourvoi CC (pivot moderne ou ancien).
    Cc,
    /// Blocs d'en-tête judiciaire du fond (APPELANT/INTIMÉ).
    Blocs,
    /// En-tête de requête admin (ni pivot ni bloc).
    Admin,
}

/// Segments structurels du préambule CC (bornes en chars). Un arrêt de
/// jonction porte plusieurs pourvois principaux : une zone par pivot.
struct CcSegs {
    /// Demandeurs au pourvoi — une zone par pivot principal.
    app: Vec<(usize, usize)>,
    /// Défendeurs à la cassation — une fenêtre par pivot principal.
    def: Vec<(usize, usize)>,
}

/// Signaux procéduraux textuels (voie/office/domaine) — les combinaisons
/// avec les métadonnées (`solution`, `type_recours`, formation) vivent dans
/// `extract::extract_procedure`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcSignals {
    pub qpc: bool,
    pub papc: bool,
    pub filtrage: bool,
    pub magdes: bool,
    pub magdes_tail: bool,
    pub magdes_form_cour: bool,
    pub magdes_form_trib: bool,
    pub jref_demande: bool,
    pub jref_conseil: bool,
    pub refere_suspension: bool,
    pub refere_liberte: bool,
    pub refere_utiles: bool,
    pub refere_precontractuel: bool,
    pub refere_provision: bool,
    pub refere_cour: bool,
    pub refere_civil: bool,
    /// « ARRÊT DE DÉSISTEMENT » en titre : l'instance s'interrompt, aucune
    /// voie (le label solution métadonnée est souvent « other »).
    pub desist_bandeau: bool,
    pub retention: bool,
    pub retention_anywhere: bool,
    pub hospi: bool,
    pub hospi_anywhere: bool,
    pub rectification: bool,
    pub premier_president: bool,
    pub jex: bool,
    pub jex_saisie_immo: bool,
    pub proc_collective: bool,
    pub dom_fp: bool,
    pub dom_aide: bool,
    pub dom_urba: bool,
    pub dom_etr: bool,
    pub dom_fisc: bool,
    pub dom_env: bool,
    pub dom_penal_pub: bool,
    pub immig_anywhere: bool,
}

/// Conseils extraits (tranches verbatim), par côté.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CounselOut {
    pub applicant_names: Vec<String>,
    pub applicant_firms: Vec<String>,
    pub defendant_names: Vec<String>,
    pub defendant_firms: Vec<String>,
}

fn is_upper_span(chars: &[char], s: usize, e: usize) -> bool {
    let mut seen = false;
    for &c in &chars[s..e] {
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            seen = true;
        }
    }
    seen
}

fn is_lower_span(chars: &[char], s: usize, e: usize) -> bool {
    !chars[s..e].iter().any(|c| c.is_uppercase())
}

/// En-tête de bloc légitime : tout en capitales, ou suivi de « : » (± espaces).
fn header_case(chars: &[char], s: usize, e: usize) -> bool {
    if is_upper_span(chars, s, e) {
        return true;
    }
    let mut i = e;
    while i < chars.len() && chars[i] == ' ' {
        i += 1;
    }
    i < chars.len() && chars[i] == ':'
}

/// Scanne `text` (déjà collapsé en espaces simples) : flux de tokens trié.
pub fn scan(text: &str) -> DocScan {
    scan_norm(Norm::new(text))
}

/// Variante à préparation fournie : le pipeline par-document construit UN
/// [`Norm`] et le partage entre les moteurs (marqueurs ici, citations
/// `crate::compiled`). Le `DocScan` en prend possession.
pub fn scan_norm(norm: Norm) -> DocScan {
    let toks = crate::compiled::scan_marks(&norm);
    DocScan {
        norm,
        toks,
        registry_memo: std::cell::OnceCell::new(),
    }
}

impl DocScan {
    /// Texte plié du document (`fold_stable`, 1:1 char-stable) — partagé avec
    /// la dérivation des spans-évidences de `crate::parties` (ADR 0182), qui
    /// évite ainsi un second pliage par décision.
    pub fn folded(&self) -> &str {
        &self.norm.folded
    }
}

/// Assemblage depuis le scan fusionné (`compiled::doc_extract`) — les champs
/// de [`DocScan`] restent privés au module.
pub(crate) fn docscan_from_parts(norm: Norm, toks: Vec<PTok>) -> DocScan {
    DocScan {
        norm,
        toks,
        registry_memo: std::cell::OnceCell::new(),
    }
}

/// Gate de légitimité d'un match marqueur (frontières de mot, casse, recast
/// « débats ») — appelé par le scan FUSIONNÉ de `compiled` pour chaque match
/// de motif marqueur. `None` = match rejeté (ne consomme pas le texte).
pub(crate) fn marker_token(
    chars: &[char],
    folded: &str,
    byte2char: &[usize],
    bs: usize,
    be: usize,
    kind: Mk,
) -> Option<PTok> {
    let bytes = folded.as_bytes();
    // Frontières de mot — seulement là où la surface finit/commence par un
    // alphanumérique (« et l' » finit par une apostrophe : pas de frontière
    // à exiger à droite).
    let before_ok =
        !bytes[bs].is_ascii_alphanumeric() || bs == 0 || !bytes[bs - 1].is_ascii_alphanumeric();
    let after_ok = !bytes[be - 1].is_ascii_alphanumeric()
        || be >= bytes.len()
        || !bytes[be].is_ascii_alphanumeric();
    if !before_ok || !after_ok {
        return None;
    }
    let (s, e) = (byte2char[bs], byte2char[be]);
    // « débats » / « l'affaire a été débattue » : en casse d'en-tête c'est un
    // Stop (fin de zone parties, « DÉBATS : ») ; en prose c'est l'ancre des
    // dates d'audience (« lors des débats du 3 octobre… », « L'affaire a été
    // débattue le 4 septembre… » — le composite avale son « débattue »
    // intérieur au leftmost-longest, le recast rend le signal).
    let kind = if kind == Mk::Stop
        && matches!(&folded[bs..be], "debats" | "l'affaire a ete debattue")
        && !header_case(chars, s, e)
    {
        Mk::Audience
    } else {
        kind
    };
    let legit = match kind {
        // Sigles : tout en capitales dans l'original (« SA » oui, « sa » non).
        Mk::Form | Mk::InstSigle => is_upper_span(chars, s, e),
        // Structures d'avocats : les greffes les écrivent aussi en casse
        // mixte (« Selarl DNL Avocats ») — capitale initiale suffit, aucune
        // n'est un mot de prose. « cabinet » l'est : accepté tel quel, la
        // légitimité de conseil se joue au contexte (intro/apposition).
        Mk::LawStruct => chars[s].is_uppercase() || &folded[bs..be] == "cabinet",
        // En-têtes de bloc et stops : casse d'en-tête de greffe.
        Mk::BlockApp | Mk::BlockDef | Mk::BlockOther | Mk::Stop => header_case(chars, s, e),
        // Dispositif : casse d'en-tête, ou « Par ces motifs Confirme… » —
        // capitale initiale suivie d'un verbe de dispositif Capitalisé
        // (la prose « par ces motifs, la cour… » reste exclue).
        Mk::Dispositif => {
            header_case(chars, s, e)
                || (chars[s].is_uppercase()
                    && chars[e..]
                        .iter()
                        .find(|c| **c != ' ')
                        .is_some_and(|c| c.is_uppercase()))
        }
        // Motifs : capitale initiale (« en considérant que » de prose exclu).
        Mk::Motifs => chars[s].is_uppercase(),
        // Prose : bas-de-casse exigé (en capitales c'est du nom). Le
        // pattern « a » vise le verbe avoir : la préposition « à » (même
        // fold, accent dans l'original) vit DANS les noms (« Vivre à
        // Machilly »).
        Mk::TrimLower => is_lower_span(chars, s, e) && (&folded[bs..be] != "a" || chars[s] == 'a'),
        // « Me »/« Maître » : capitale initiale (« me » pronom exclu).
        Mk::Me => chars[s].is_uppercase(),
        // « M. » suivi d'autres points = placeholder anonymisé « M... »
        // (un NOM de partie), pas une civilité.
        Mk::TrimAlways if matches!(&folded[bs..be], "m." | "mm.") => {
            chars.get(e).copied() != Some('.')
        }
        _ => true,
    };
    legit.then_some(PTok { s, e, kind })
}

// ── composition : entités d'un segment ──────────────────────────────────────

static RE_TRAILING_ADDR: OnceLock<Regex> = OnceLock::new();
static RE_AFTER_COUNSEL: OnceLock<Regex> = OnceLock::new();

fn re_trailing_addr() -> &'static Regex {
    RE_TRAILING_ADDR
        .get_or_init(|| Regex::new(r"(?:\s*\[(?:Adresse|Localité|Localite)[^\]]*\])+\s*$").unwrap())
}

/// Structures d'avocats : jamais des parties — sauf ès qualités (mandataire,
/// liquidateur, notaire), où la SELARL/SCP EST la partie.
fn is_law_structure(name: &str) -> bool {
    let up = name.to_uppercase();
    if ["MANDATAIRE", "LIQUIDAT", "NOTAIR", "HUISSIER"]
        .iter()
        .any(|m| up.contains(m))
    {
        return false;
    }
    ["SELARL", "SELAS", "SELAFA", "SCP ", "AARPI", "SCM "]
        .iter()
        .any(|m| up.starts_with(m.trim_end()))
        || up.contains("AVOCAT")
}

/// Mêmes mots pliés, ordre libre — le greffe d'un arrêt de jonction re-liste
/// une partie d'un pourvoi à l'autre en permutant parfois les patronymes
/// (« J... F..., Y... L... » ↔ « Y... L..., J... F... »).
fn same_words(a: &str, b: &str) -> bool {
    let words = |s: &str| {
        let f = crate::compiled::fold_stable(s);
        let mut w: Vec<String> = f
            .split(|c: char| !c.is_alphanumeric())
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect();
        w.sort_unstable();
        w
    };
    words(a) == words(b)
}

/// Le nom nettoyé n'est-il qu'une phrase de forme (« d'exercice libéral à
/// responsabilité limitée ») ? Résidu de qualificatifs, pas une raison sociale.
fn is_form_phrase(name: &str) -> bool {
    const FORM_WORDS: &[&str] = &[
        "d'exercice",
        "libéral",
        "liberal",
        "à",
        "a",
        "responsabilité",
        "responsabilite",
        "limitée",
        "limitee",
        "anonyme",
        "civile",
        "immobilière",
        "immobiliere",
        "par",
        "actions",
        "simplifiée",
        "simplifiee",
        "unipersonnelle",
        "en",
        "nom",
        "collectif",
        "coopérative",
        "cooperative",
        "de",
        "droit",
        "mère",
        "mere",
    ];
    name.split_whitespace()
        .all(|w| FORM_WORDS.contains(&w.to_lowercase().as_str()))
}

impl DocScan {
    /// Jumelle mieux cassée d'une valeur NER : quand `value` porte un mot
    /// tout-CAPS (l'en-tête parties écrase le patronyme en capitales), cherche
    /// dans le texte plié une autre occurrence des mêmes chars pliés, en
    /// frontières de mot, dont la tranche source est mieux cassée
    /// (cf. [`crate::extract::common::better_cased`]) — la sortie reste une
    /// tranche VERBATIM du texte, jamais une réécriture.
    pub(crate) fn best_cased_twin(&self, value: &str) -> Option<String> {
        let needle = crate::compiled::fold_stable(value);
        if needle.is_empty() {
            return None;
        }
        let bytes = self.norm.folded.as_bytes();
        let len = needle.chars().count();
        for (bs, _) in self.norm.folded.match_indices(&needle) {
            let be = bs + needle.len();
            let boundary = (bs == 0 || !bytes[bs - 1].is_ascii_alphanumeric())
                && (be >= bytes.len() || !bytes[be].is_ascii_alphanumeric());
            if !boundary {
                continue;
            }
            let cs = self.norm.byte2char[bs];
            let cand: String = self.norm.chars[cs..cs + len].iter().collect();
            if crate::extract::common::better_cased(value, &cand) {
                return Some(cand);
            }
        }
        None
    }

    /// Tranche verbatim ; les runs de blancs (mappés `' '` par [`Norm`]) se
    /// collapsent en un espace — même forme de sortie que la convention GT
    /// (espaces collapsés), les offsets du scan restent ceux du texte.
    /// Tranche du texte plié aux bornes CHARS (les `PTok` sont en chars, la
    /// `String` pliée en octets — conversion via `char2byte`).
    fn folded_slice(&self, s: usize, e: usize) -> &str {
        &self.norm.folded[self.norm.char2byte[s]..self.norm.char2byte[e]]
    }

    fn text_slice(&self, s: usize, e: usize) -> String {
        let mut out = String::with_capacity(e - s);
        for &c in &self.norm.chars[s..e] {
            if c != ' ' || !out.ends_with(' ') {
                out.push(c);
            }
        }
        out
    }

    fn len(&self) -> usize {
        self.norm.chars.len()
    }

    /// Position (chars) du premier token de `kind` dans `[from..to)`.
    fn find_tok(&self, kinds: &[Mk], from: usize, to: usize) -> Option<&PTok> {
        self.toks
            .iter()
            .find(|t| t.s >= from && t.s < to && kinds.contains(&t.kind))
    }

    /// Étend un nom depuis `start` (char) : s'arrête à la ponctuation de liste,
    /// au prochain token fermant, ou à 90 chars. Les placeholders `[…]` et UN
    /// groupe parenthésé ouvert en capitale/chiffre font partie du nom.
    fn extend_name(&self, start: usize, seg_end: usize) -> usize {
        let hard_end = seg_end.min(self.len()).min(start + 90);
        // positions de terminaison par token
        let mut tok_end = hard_end;
        for t in &self.toks {
            if t.s <= start || t.s >= tok_end {
                continue;
            }
            // Les marqueurs OUVRANTS (Form/InstHead/InstSigle) ne ferment PAS
            // l'extension : les noms institutionnels composés en contiennent en
            // plein milieu (« caisse régionale de crédit agricole mutuel… »
            // porte « crédit agricole » ; « SA Banque Populaire » porte
            // « banque »). Mesuré : les faire fermer coûte −3 pts de recall
            // defendant. La séparation des énumérations passe par les
            // connecteurs (« et la/et le… », TrimLower) et la virgule.
            let closes = matches!(
                t.kind,
                Mk::TrimAlways
                    | Mk::TrimLower
                    | Mk::CounselIntro
                    | Mk::AvocatDe
                    | Mk::Me
                    | Mk::BlockApp
                    | Mk::BlockDef
                    | Mk::BlockOther
                    | Mk::Stop
                    | Mk::PivotNew
                    | Mk::PivotOld
                    | Mk::Opposant
                    | Mk::Contre
                    | Mk::DefEnd
                    | Mk::Societe
                    | Mk::LawStruct
                    | Mk::IndirectRole
                    | Mk::DefIntro
                    | Mk::DefConclu
                    | Mk::MoyenPar
                    | Mk::Motifs
                    | Mk::Dispositif
                    | Mk::OutDesist
            );
            if closes {
                tok_end = t.s;
            }
        }
        let mut i = start;
        while i < tok_end {
            match self.norm.chars[i] {
                ',' => {
                    // continuation d'apposition nominale « comité d'hygiène,
                    // de sécurité et des conditions de travail » : virgule +
                    // de/des/du + mot bas-de-casse non article
                    let rest: String = self.norm.chars[i + 1..tok_end.min(i + 12)].iter().collect();
                    let rest = rest.trim_start();
                    let cont = ["de ", "des ", "du "].iter().any(|p| {
                        rest.strip_prefix(p).is_some_and(|after| {
                            let w: String =
                                after.chars().take_while(|c| c.is_alphabetic()).collect();
                            !w.is_empty()
                                && w.chars().all(|c| c.is_lowercase())
                                && !matches!(
                                    w.as_str(),
                                    "la" | "le"
                                        | "les"
                                        | "ce"
                                        | "cette"
                                        | "ces"
                                        | "même"
                                        | "meme"
                                        | "son"
                                        | "sa"
                                        | "ses"
                                        | "leur"
                                        | "leurs"
                                        | "tout"
                                        | "toute"
                                        | "tous"
                                        | "toutes"
                                )
                        })
                    });
                    if !cont {
                        break;
                    }
                    i += 1;
                }
                ';' | ':' | ')' | '\n' => break,
                '[' => {
                    // placeholder : consommé en bloc
                    let close = self.norm.chars[i..tok_end.min(i + 42)]
                        .iter()
                        .position(|&c| c == ']');
                    match close {
                        Some(off) => i += off + 1,
                        None => break,
                    }
                }
                '(' => {
                    // groupe parenthésé : gardé si ouvert en capitale/chiffre
                    let inner = self.norm.chars.get(i + 1).copied().unwrap_or(' ');
                    if !(inner.is_uppercase() || inner.is_ascii_digit()) {
                        break;
                    }
                    let close = self.norm.chars[i..tok_end.min(i + 42)]
                        .iter()
                        .position(|&c| c == ')');
                    match close {
                        Some(off) => i += off + 1,
                        None => break,
                    }
                }
                _ => i += 1,
            }
        }
        i
    }

    /// Nettoie la tranche `[s..e)` en nom de partie (ou rien).
    fn clean(&self, s: usize, e: usize) -> Option<String> {
        let name = self.clean_inner(s, e)?;
        if is_form_phrase(&name) || is_law_structure(&name) {
            return None;
        }
        Some(name)
    }

    /// Socle du nettoyage : placeholders, adresses de greffe, connecteurs
    /// orphelins, exigence d'un mot porteur.
    fn clean_inner(&self, s: usize, e: usize) -> Option<String> {
        let raw = self.text_slice(s, e);
        // placeholders d'anonymisation VIDES « [...] » : du bruit, jamais du nom
        let raw = raw.replace("[...]", " ").replace("[..]", " ");
        let raw = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        // « M... » / « X... » : les points TRAILING d'un placeholder anonymisé
        // sont le nom — on ne les rogne pas.
        let mut name = if raw.trim_end().ends_with("...") {
            raw.trim().to_string()
        } else {
            raw.trim()
                .trim_end_matches(['.', ',', ';', '-', '*'])
                .trim()
                .to_string()
        };
        // tête doublée « syndicat Syndicat des pilotes… » : une seule
        let words: Vec<&str> = name.splitn(3, ' ').collect();
        if words.len() >= 2 && words[0].to_lowercase() == words[1].to_lowercase() {
            name = name[words[0].len() + 1..].to_string();
        }
        // Placeholders d'adresse de greffe : coupe au PREMIER « [Adresse/
        // [Localité » précédé d'une capitale ou d'un chiffre (« SAS X
        // [Adresse 15] [Localité 9]- Suisse ») ; liés par la prose
        // (« résidence [Adresse 3] ») ou seuls porteurs, ils restent.
        let addr_at = name
            .find(" [Adresse")
            .or_else(|| name.find(" [Localité").or_else(|| name.find(" [Localite")));
        if let Some(pos) = addr_at {
            let kept = name[..pos].trim_end();
            // connecteur en capitales devant un placeholder UNIQUE : celui-ci
            // est le nom même (« OFFICE DES POURSUITES DE [Localité 20] »),
            // pas un bloc adresse — la règle capitale seule l'amputerait
            let connector = name[pos..].matches('[').count() == 1
                && kept
                    .rsplit(' ')
                    .next()
                    .is_some_and(|w| matches!(w, "DE" | "DU" | "DES" | "LA" | "LE" | "LES"));
            if !connector
                && kept
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
            {
                name = kept.to_string();
            }
        }
        if let Some(m) = re_trailing_addr().find(&name) {
            let kept = name[..m.start()].trim_end().to_string();
            // connecteur en capitales + placeholder UNIQUE : le placeholder
            // est le nom même (« OFFICE DES POURSUITES DE [Localité 20] ») ;
            // un RUN de placeholders est un bloc adresse collé au nom
            // (« CPAM DE CHARENTE DE [Localité 5] [Adresse 2] [Localité 5] »)
            let connector = m.as_str().matches('[').count() == 1
                && kept
                    .rsplit(' ')
                    .next()
                    .is_some_and(|w| matches!(w, "DE" | "DU" | "DES" | "LA" | "LE" | "LES"));
            if !connector
                && kept
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
            {
                name = kept;
            }
        }
        // connecteurs orphelins en fin de nom (« Limeacorp et », « du
        // Bas-Rhin par »)
        loop {
            let trimmed = name.trim_end();
            let cut = [
                "et", "de", "du", "des", "par", "le", "la", "les", "à", "d'", "l'",
            ]
            .iter()
            .find_map(|w| {
                trimmed
                    .strip_suffix(w)
                    .filter(|rest| rest.ends_with(' '))
                    .map(|rest| rest.trim_end().to_string())
            });
            match cut {
                Some(c) if !c.is_empty() => name = c,
                _ => {
                    name = trimmed.trim_end_matches(['-', ',']).trim_end().to_string();
                    break;
                }
            }
        }
        let carrier = name.chars().filter(|c| c.is_alphanumeric()).count() >= 2
            || ((name.contains("...") || name.contains('['))
                && name.chars().any(|c| c.is_alphanumeric()));
        if !carrier {
            return None;
        }
        Some(name)
    }

    /// Variante de [`Self::clean`] pour une partie ouverte par une structure
    /// d'avocats (SELARL mandataire, SCP notaire…) : le nom qui suit la
    /// structure n'est pas filtré comme conseil.
    fn clean_law_party(&self, s: usize, e: usize) -> Option<String> {
        let name = self.clean_inner(s, e)?;
        if name.to_uppercase().contains("AVOCAT") {
            return None;
        }
        Some(name)
    }

    /// La position `e` est-elle immédiatement suivie d'une apposition d'avocat
    /// (« , avocat au barreau… ») ? Structure de conseil, pas une partie.
    fn followed_by_counsel(&self, e: usize) -> bool {
        let re = RE_AFTER_COUNSEL.get_or_init(|| Regex::new(r"^\s*,?\s*avocats?\b").unwrap());
        let end = (e + 30).min(self.len());
        re.is_match(&fold_stable(&self.text_slice(e, end)))
    }

    /// Fin de la chaîne d'associés « X, Y, Z » après `ne` — les cabinets
    /// énumèrent leurs associés par virgule avant l'apposition « , avocat » ;
    /// on prolonge sur un mot Capitalisé qui n'ouvre pas de token.
    fn chain_end(&self, mut ne: usize, to: usize) -> usize {
        while self.norm.chars.get(ne).copied() == Some(',') {
            let j = self.skip_spaces(ne + 1, to);
            let c = self.norm.chars.get(j).copied().unwrap_or(' ');
            if !c.is_uppercase() || self.tok_starting_at(j) {
                break;
            }
            let n2 = self.extend_name(j, to);
            if n2 <= j {
                break;
            }
            ne = n2;
        }
        ne
    }

    /// Une intro de conseil (« représentée par ») colle-t-elle (≤ 12 chars,
    /// l'article au plus) juste avant `pos` ? Un chiffre dans le gap = ligne
    /// de greffe (« toque 343 », « vestiaire : D0289 ») qui clôt le conseil
    /// de la partie PRÉCÉDENTE, pas une intro.
    fn counsel_intro_just_before(&self, pos: usize) -> bool {
        self.toks.iter().any(|c| {
            c.kind == Mk::CounselIntro
                && c.e <= pos
                && c.e + 12 > pos
                && !self.norm.chars[c.e..pos]
                    .iter()
                    .any(|ch| ch.is_ascii_digit())
        })
    }

    /// La position `start` ouvre-t-elle sur un terminateur (« la société dont
    /// le siège… ») ? Alors ce n'est pas un nom, c'est de la prose.
    fn opens_on_terminator(&self, start: usize) -> bool {
        self.toks.iter().any(|t| {
            t.s == start
                && matches!(
                    t.kind,
                    Mk::TrimAlways | Mk::TrimLower | Mk::CounselIntro | Mk::AvocatDe | Mk::Me
                )
        })
    }

    /// Génitif immédiat (« de la », « du », « des », « de l' ») devant `pos` :
    /// position du « de » s'il y en a un. Toute ponctuation coupe la chaîne
    /// (« En présence de : la société X » n'est pas un génitif).
    fn genitive_before(&self, pos: usize) -> Option<usize> {
        let chars = &self.norm.chars;
        let word_before = |mut i: usize| -> Option<(usize, String)> {
            while i > 0 && chars[i - 1] == ' ' {
                i -= 1;
            }
            let e = i;
            while i > 0 && (chars[i - 1].is_alphabetic() || chars[i - 1] == '\'') {
                i -= 1;
            }
            (i < e).then(|| (i, chars[i..e].iter().collect::<String>().to_lowercase()))
        };
        let (s1, w1) = word_before(pos)?;
        // « au/aux » : même valeur de complément que le génitif
        // (« hospitalisée au CENTRE HOSPITALIER DE X »)
        if matches!(w1.as_str(), "de" | "du" | "des" | "d'" | "au" | "aux") {
            return Some(s1);
        }
        if matches!(w1.as_str(), "la" | "le" | "les" | "l'") {
            let (s2, w2) = word_before(s1)?;
            if matches!(w2.as_str(), "de" | "d'") {
                return Some(s2);
            }
        }
        None
    }

    /// Un rôle indirect (« aux droits de », « assureur de ») s'ouvre-t-il
    /// juste avant `pos` ? L'entité qui suit n'est pas une partie.
    fn indirect_role_before(&self, pos: usize) -> bool {
        self.toks
            .iter()
            .any(|t| t.kind == Mk::IndirectRole && t.e <= pos && t.e + 40 > pos)
    }

    /// Récolte les personnes morales d'un segment `[from..to)`, en ordre.
    /// `inst_to` borne les têtes institutionnelles (admin : avant « demande »).
    pub fn harvest(&self, from: usize, to: usize, inst_to: usize) -> Vec<String> {
        self.harvest_in(from, to, inst_to, false)
    }

    /// Variante à skip génitif : une entité introduite par « de la » / « du » /
    /// « des » à l'intérieur du segment est un complément (« administrateur de
    /// la société X »), pas une entrée de bloc.
    fn harvest_in(
        &self,
        from: usize,
        to: usize,
        inst_to: usize,
        skip_genitive: bool,
    ) -> Vec<String> {
        let to = to.min(self.len());
        let mut out: Vec<String> = Vec::new();
        let mut consumed_until = from;
        // Fusion par contenance (préfixe/suffixe, casse pliée) : « CPAM » et
        // « CPAM DES PYRENEES ORIENTALES » sont la même entité — on garde la
        // plus longue.
        let push = |name: String, out: &mut Vec<String>| {
            let nu = name.to_uppercase();
            for o in out.iter_mut() {
                let ou = o.to_uppercase();
                if ou.starts_with(&nu) || ou.ends_with(&nu) {
                    return; // déjà couverte par une plus longue
                }
                if nu.starts_with(&ou) || nu.ends_with(&ou) {
                    *o = name; // remplace la plus courte
                    return;
                }
            }
            out.push(name);
        };
        for idx in 0..self.toks.len() {
            let t = self.toks[idx].clone();
            if t.s < from || t.s >= to || t.s < consumed_until {
                continue;
            }
            if matches!(
                t.kind,
                Mk::Form | Mk::Societe | Mk::InstHead | Mk::InstSigle | Mk::LawStruct
            ) && (self.indirect_role_before(t.s)
                || (skip_genitive && self.genitive_before(t.s).is_some_and(|s| s >= from)))
            {
                continue;
            }
            match t.kind {
                Mk::Form => {
                    // « représentée par la SARL Cabinet Briard » : la forme
                    // sociale collée à une intro de conseil est un cabinet
                    if self.counsel_intro_just_before(t.s) {
                        continue;
                    }
                    let form = self.text_slice(t.s, t.e);
                    let ns = self.skip_spaces(t.e, to);
                    let first = self.norm.chars.get(ns).copied().unwrap_or(' ');
                    if !(first.is_uppercase() || first.is_ascii_digit() || first == '[')
                        || self.opens_on_terminator(ns)
                    {
                        continue;
                    }
                    let ne = self.extend_name(ns, to);
                    let chain = self.chain_end(ne, to);
                    if self.followed_by_counsel(chain) {
                        consumed_until = chain;
                        continue;
                    }
                    if let Some(name) = self.clean(ns, ne) {
                        push(format!("{form} {name}"), &mut out);
                        consumed_until = ne;
                    }
                }
                Mk::Societe => {
                    let mut ns = self.skip_spaces(t.e, to);
                    let head_end = ns;
                    // consomme les qualificatifs (« anonyme », « de droit X »…)
                    loop {
                        let q = self
                            .toks
                            .iter()
                            .find(|q| q.s == ns && q.kind == Mk::Qualif)
                            .cloned();
                        match q {
                            Some(q) => {
                                ns = self.skip_spaces(q.e, to);
                                if self.text_slice(q.s, q.e).to_lowercase().ends_with("droit") {
                                    // « de droit <nationalité> » : un mot de plus
                                    while ns < to && self.norm.chars[ns].is_alphabetic() {
                                        ns += 1;
                                    }
                                    ns = self.skip_spaces(ns, to);
                                }
                            }
                            None => break,
                        }
                    }
                    let first = self.norm.chars.get(ns).copied().unwrap_or(' ');
                    let lower_start = first.is_lowercase();
                    if !(first.is_uppercase()
                        || first.is_ascii_digit()
                        || first == '['
                        || first == '(' // forme parenthésée « (SASU) Mayer »
                        || lower_start)
                        || self.opens_on_terminator(ns)
                    {
                        continue;
                    }
                    // attaque bas-de-casse : premier mot ≥ 4 chars
                    if lower_start {
                        let wlen = self.norm.chars[ns..to.min(ns + 8)]
                            .iter()
                            .take_while(|c| c.is_alphabetic() || **c == '\'' || **c == '-')
                            .count();
                        if wlen < 4 {
                            continue;
                        }
                    }
                    let ne = self.extend_name(ns, to);
                    if self.followed_by_counsel(ne) {
                        consumed_until = ne;
                        continue;
                    }
                    if let Some(name) = self.clean(ns, ne) {
                        if lower_start && !name.contains(' ') {
                            continue;
                        }
                        // Tête « société » NUE (aucun qualificatif, attaque
                        // majuscule/chiffre/[) = descripteur de greffe, rognée
                        // (spec 2026-07-09 : « la société Gondrand » ⇒
                        // « Gondrand »). Qualifiée (« société civile… »,
                        // attaque bas-de-casse) ou nom dont la tête fait
                        // partie (« Société Générale ») : tranche entière.
                        let bare = ns == head_end && !lower_start;
                        if bare && fold_stable(&name) != "generale" {
                            push(name, &mut out);
                        } else {
                            let head = self.text_slice(t.s, ns);
                            push(format!("{} {name}", head.trim_end()), &mut out);
                        }
                        consumed_until = ne;
                    }
                }
                Mk::InstHead | Mk::InstSigle => {
                    if t.s >= inst_to {
                        continue;
                    }
                    let ne = self.extend_name(t.e, to);
                    if self.followed_by_counsel(ne) {
                        consumed_until = ne;
                        continue;
                    }
                    if let Some(name) = self.clean(t.s, ne) {
                        // tête nue (« caisse », « banque ») = résidu de prose ;
                        // les têtes génériques 2-mots exigent un complément
                        if (!name.contains(' ') && !name.contains('[') && t.kind == Mk::InstHead)
                            || (ne <= t.e + 1
                                && matches!(
                                    fold_stable(&name).as_str(),
                                    "etablissement public" | "etablissement francais"
                                ))
                        {
                            continue;
                        }
                        push(name, &mut out);
                        consumed_until = ne;
                    }
                }
                Mk::LawStruct => {
                    // une SELARL/SCP est une PARTIE quand elle n'est pas
                    // introduite comme conseil (« représenté par Me X de la
                    // SCP Y ») : mandataires, liquidateurs, notaires assignés.
                    // « cabinet » n'ouvre jamais une partie.
                    if self.counsel_ctx_before(t.s)
                        || fold_stable(&self.text_slice(t.s, t.e)) == "cabinet"
                    {
                        continue;
                    }
                    let ns = self.skip_spaces(t.e, to);
                    let first = self.norm.chars.get(ns).copied().unwrap_or(' ');
                    if !(first.is_uppercase() || first.is_ascii_digit() || first == '[')
                        || self.opens_on_terminator(ns)
                    {
                        continue;
                    }
                    let ne = self.extend_name(ns, to);
                    let chain = self.chain_end(ne, to);
                    if self.followed_by_counsel(chain) {
                        consumed_until = chain;
                        continue;
                    }
                    // apposition ès qualités en aval (« SELARL X, mandataire
                    // judiciaire », « prise en la personne de Me Y, ès
                    // qualités de liquidateur de… ») : le mandataire de
                    // justice n'est jamais une partie, étiqueté par le
                    // greffe ou en prose — seul le débiteur représenté
                    // l'est (spec 2026-07-09).
                    let win = fold_stable(&self.text_slice(ne, (ne + 120).min(to)));
                    if [
                        "mandataire",
                        "liquidateur",
                        "administrateur judiciaire",
                        "es qualite",
                        "es-qualite",
                    ]
                    .iter()
                    .any(|k| win.contains(k))
                    {
                        consumed_until = ne;
                        continue;
                    }
                    if let Some(name) = self.clean_law_party(ns, ne) {
                        let kind = self.text_slice(t.s, t.e);
                        push(format!("{kind} {name}"), &mut out);
                        consumed_until = ne;
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Position du premier « demande(nt) » de prose dans `[from..to)` — au-delà,
    /// la requête cite l'administration défenderesse et les parties mises en
    /// cause (« …de condamner la société X… »), pas le requérant.
    fn demande_boundary(&self, from: usize, to: usize) -> usize {
        self.toks
            .iter()
            .find(|t| {
                t.s >= from && t.s < to && t.kind == Mk::TrimLower && {
                    let s: String = self.norm.chars[t.s..t.e].iter().collect();
                    let f = fold_stable(&s);
                    f == "demande" || f == "demandent"
                }
            })
            .map(|t| t.s)
            .unwrap_or(to)
    }

    /// Début des motifs (premier marqueur Motifs ou Dispositif) : fin de
    /// l'en-tête de la décision — les parties se nomment avant. Texte sans
    /// marqueur de zone (dégradé) : zone ouverte jusqu'à la fin.
    pub fn motifs_start(&self) -> usize {
        self.toks
            .iter()
            .find(|t| matches!(t.kind, Mk::Motifs | Mk::Dispositif))
            .map(|t| t.s)
            .unwrap_or(self.len())
    }

    /// Début du dispositif (« PAR CES MOTIFS », « DÉCIDE : »…).
    /// Début du dispositif : le premier « par ces motifs » quand il existe —
    /// les autres surfaces (« en conséquence », « décide », « arrête »)
    /// abondent dans les MOTIFS (« en conséquence, il y a lieu de… ») et
    /// ouvriraient la zone sur la narration, où les demandes des parties
    /// (« infirmer le jugement ») polluent la lecture d'outcome. Repli :
    /// premier token Dispositif (gabarit admin sans « par ces motifs »).
    pub fn dispositif_start(&self) -> usize {
        // Ouvreur FORT d'abord : « par ces motifs », les formes espacées de
        // greffe (« D É C I D E ») ou un verbe d'ouverture suivi de « : » —
        // la surface faible « en conséquence » traîne dans les motifs
        // (« annulée par voie de conséquence ») et ne sert que de fallback.
        let strong = |t: &&PTok| {
            t.kind == Mk::Dispositif && {
                let surf = fold_stable(&self.text_slice(t.s, t.e));
                // les formes verbe exigent la casse d'en-tête de greffe
                // (« DÉCIDE : », « D É C I D E ») : « Sur la légalité de
                // l'arrêté : » (l'acte attaqué en intertitre) n'ouvre pas
                // un dispositif
                let upper = is_upper_span(&self.norm.chars, t.s, t.e);
                surf.starts_with("par ces motifs")
                    || (upper
                        && (matches!(
                            surf.as_str(),
                            "d e c i d e" | "o r d o n n e" | "a r r e t e"
                        ) || (matches!(surf.as_str(), "decide" | "ordonne" | "arrete")
                            && self.span_after(t, 4).trim_start().starts_with(':'))))
            }
        };
        self.toks
            .iter()
            .find(strong)
            .or_else(|| self.toks.iter().find(|t| t.kind == Mk::Dispositif))
            .map(|t| t.s)
            .unwrap_or(self.len())
    }

    /// Fin de la zone dispositif : les MOYENS ANNEXÉS d'un arrêt CC suivent
    /// le dispositif et regorgent de « en ce qu'il » — ils n'en font pas
    /// partie.
    fn dispositif_end(&self, from: usize) -> usize {
        self.toks
            .iter()
            .find(|t| t.kind == Mk::MoyenPar && t.s > from)
            .map(|t| t.s)
            .unwrap_or(self.len())
    }

    /// Fin du bandeau d'en-tête de greffe (« CIV. 1 … NON-ADMISSION … ») :
    /// premier marqueur d'ouverture de parties/requête — les signaux de
    /// bandeau (QPC, non-admission) lus au-delà décrivent la procédure
    /// antérieure, pas la décision.
    fn bandeau_end(&self) -> usize {
        self.toks
            .iter()
            .find(|t| {
                matches!(
                    t.kind,
                    Mk::PivotNew
                        | Mk::PivotOld
                        | Mk::BlockApp
                        | Mk::BlockDef
                        | Mk::BlockOther
                        | Mk::AdminReq
                        | Mk::MemIntro
                )
            })
            .map(|t| t.s)
            .unwrap_or_else(|| self.motifs_start())
    }

    /// Tranche VERBATIM du bandeau (zone par tokens — remplace les `head(N)`
    /// legacy) : en-tête de greffe où vivent chambre/pôle.
    pub fn bandeau_text(&self) -> String {
        self.text_slice(0, self.bandeau_end())
    }

    /// Tranche VERBATIM de l'en-tête complet (jusqu'aux motifs) — le titre de
    /// juridiction peut suivre les mentions de greffe (« Copie aux
    /// demandeurs ») qui ferment le bandeau.
    pub fn header_text(&self) -> String {
        self.text_slice(0, self.motifs_start())
    }

    /// Fenêtres VERBATIM `[s-before, e+after)` autour des tokens filtrés,
    /// fusionnées quand elles se chevauchent (un même match de regex ne se
    /// compte pas deux fois sur deux ancres adjacentes).
    fn windows(&self, before: usize, after: usize, pred: impl Fn(&PTok) -> bool) -> Vec<String> {
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for t in self.toks.iter().filter(|t| pred(t)) {
            let s = t.s.saturating_sub(before);
            let e = (t.e + after).min(self.len());
            match spans.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => spans.push((s, e)),
            }
        }
        spans.iter().map(|&(s, e)| self.text_slice(s, e)).collect()
    }

    /// Fenêtres des ancres de date d'audience : tokens [`Mk::Audience`], plus
    /// les en-têtes « DÉBATS »/« L'affaire a été débattue » restés Stop (casse
    /// d'en-tête) — « Date des débats : 15/03/2021 » se lit là.
    pub fn audience_windows(&self) -> Vec<String> {
        self.windows(110, 190, |t| {
            t.kind == Mk::Audience
                || (t.kind == Mk::Stop && fold_stable(&self.text_slice(t.s, t.e)).contains("debat"))
        })
    }

    /// Composition à juge unique dite par le texte, en zone d'en-tête
    /// (« statuant en juge unique », « siégeant seul »).
    pub fn juge_unique_header(&self) -> bool {
        self.find_tok(&[Mk::ProcJugeUnique], 0, self.motifs_start())
            .is_some()
    }

    /// Votes de TERMES par domaine, comptés plein texte : familles `ProcDom*`
    /// (admin + judiciaire) et familles procédurales porteuses de matière
    /// (JEX, procédures collectives). Consommés par
    /// `domain::refine_with_terms` — clés du référentiel `legal_domain:*`.
    /// `admin` = ordre administratif : le vocabulaire travail des deux ordres
    /// (`ProcDomTravailMixte`) y vote fonction publique.
    pub fn domain_term_votes(&self, admin: bool) -> Vec<(&'static str, u32)> {
        let mut counts: std::collections::HashMap<&'static str, u32> =
            std::collections::HashMap::new();
        for t in &self.toks {
            let d = match t.kind {
                Mk::ProcDomTravail => "SOCIAL_DROIT_TRAVAIL",
                // vocabulaire travail des deux ordres : un licenciement
                // devant le juge administratif est de la fonction publique
                Mk::ProcDomTravailMixte => {
                    if admin {
                        "PUBLIC_DROIT_TRAVAIL"
                    } else {
                        "SOCIAL_DROIT_TRAVAIL"
                    }
                }
                // AT/MP = contentieux du travail (école gold, comme la
                // plage CSS livre 4) ; cotisations = SOCIAL nu terminal
                Mk::ProcDomSecu => {
                    if admin {
                        "PUBLIC_DROIT_TRAVAIL"
                    } else {
                        "SOCIAL_DROIT_TRAVAIL"
                    }
                }
                Mk::ProcDomCotisations => "SOCIAL",
                Mk::ProcDomFamille => "CIVIL_DROIT_PERSONNES_FAMILLE",
                Mk::ProcDomDivorce => "CIVIL_DIVORCE_SEPARATION_CORPS",
                Mk::ProcDomSuccessions => "CIVIL_DROIT_SUCCESSIONS",
                Mk::ProcDomLocatif => "CIVIL_DROIT_LOCATIF",
                Mk::ProcDomCopro => "CIVIL_DROIT_COPROPRIETE_PROPRIETE_IMMOBILIERE",
                Mk::ProcDomConstruction => "CIVIL_DROIT_IMMOBILIER_CONSTRUCTION",
                Mk::ProcDomSaisieImmo => "CIVIL_DROIT_SAISIE_IMMOBILIERE",
                Mk::ProcDomExecution => "CIVIL_PROCEDURES_CIVILES_EXECUTION",
                Mk::ProcDomAssurances => "CIVIL_DROIT_ASSURANCES",
                Mk::ProcDomBancaire => "CIVIL_DROIT_BANCAIRE_BOURSIER",
                Mk::ProcDomResp => "CIVIL_DROIT_RESPONSABILITE",
                Mk::ProcDomEntDiff | Mk::ProcCollective => {
                    "COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE"
                }
                Mk::ProcDomSocietes => "COMMERCIAL_DROIT_SOCIETES",
                Mk::ProcDomConcurrence => "COMMERCIAL_DROIT_CONCURRENCE",
                Mk::ProcDomConso => "COMMERCIAL_DROIT_CONSOMMATION",
                Mk::ProcDomPiLit => "PROPRIETE_INTELLECTUELLE_LITTERAIRE_ARTISTIQUE",
                Mk::ProcDomPiInd => "PROPRIETE_INTELLECTUELLE_INDUSTRIELLE",
                Mk::ProcDomFp => "PUBLIC_DROIT_TRAVAIL",
                Mk::ProcDomAide => "PUBLIC_DROIT_AIDE_ACTION_SOCIALE",
                Mk::ProcDomUrba => "PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC",
                Mk::ProcDomEtr => "PUBLIC_DROIT_ETRANGERS_NATIONALITE",
                Mk::ProcDomFisc => "FISCAL",
                Mk::ProcDomEnv => "PUBLIC_DROIT_ENVIRONNEMENT",
                Mk::ProcDomPenalPub => "PUBLIC_DROIT_PENAL_PUBLIC",
                // « saisie immobilière » partage le token JEX : la surface
                // départage la matière.
                Mk::ProcJex => {
                    if fold_stable(&self.text_slice(t.s, t.e)).starts_with("saisie") {
                        "CIVIL_DROIT_SAISIE_IMMOBILIERE"
                    } else {
                        "CIVIL_PROCEDURES_CIVILES_EXECUTION"
                    }
                }
                _ => continue,
            };
            *counts.entry(d).or_default() += 1;
        }
        let mut votes: Vec<_> = counts.into_iter().collect();
        votes.sort();
        votes
    }

    /// Un article de référé CJA ancré en zone d'en-tête ? Le juge des référés
    /// statue seul (L. 511-2 CJA). Garde CESEDA sur L. 551-* : la même
    /// surface d'article y désigne la rétention, pas le précontractuel.
    pub fn refere_article_header(&self) -> bool {
        let h = self.motifs_start();
        let has = |k: Mk| self.find_tok(&[k], 0, h).is_some();
        has(Mk::ProcRefSusp)
            || has(Mk::ProcRefLib)
            || has(Mk::ProcRefUtile)
            || has(Mk::ProcRefProv)
            || (has(Mk::ProcRefPrecontr) && !has(Mk::ProcImmig))
    }

    /// Fenêtres après « joint les pourvois » : la clause des numéros joints.
    pub fn joint_pourvois_windows(&self) -> Vec<String> {
        self.windows(0, 400, |t| t.kind == Mk::JointPourvois)
    }

    /// Fenêtres autour des ancres « sous le(s) n°/numéro(s) » (AdminReq) du
    /// RÉCIT DE PROCÉDURE (zone 0→liste de visas/motifs : les requêtes
    /// jointes s'annoncent là ; la liste de visas « Vu : - la requête… » et
    /// les motifs citent d'AUTRES affaires), avec assez d'avant-contexte pour
    /// les gardes (« pourvoi en cassation sous le n°… » = citation).
    pub fn docket_context_windows(&self) -> Vec<String> {
        let zone_end = self
            .toks
            .iter()
            .find(|t| t.kind == Mk::VisaList)
            .map(|t| t.s)
            .unwrap_or(self.len())
            .min(self.motifs_start());
        self.windows(170, 200, |t| {
            t.kind == Mk::AdminReq
                && t.s < zone_end
                && fold_stable(&self.text_slice(t.s, t.e)).starts_with("sous")
        })
    }

    /// Le petit span (chars) qui SUIT un token, plié — pour les exceptions
    /// positionnées (« lieu de statuer *sur les dépens* »).
    fn span_after(&self, t: &PTok, n: usize) -> String {
        fold_stable(&self.text_slice(t.e, (t.e + n).min(self.len())))
    }

    /// Le petit span qui PRÉCÈDE un token, plié (gardes « non/pas prescrit »).
    fn span_before(&self, t: &PTok, n: usize) -> String {
        fold_stable(&self.text_slice(t.s.saturating_sub(n), t.s))
    }

    /// Y a-t-il un token `kind` légitime dans la zone dispositif ?
    fn disp_has(&self, from: usize, kind: Mk) -> bool {
        self.disp_find(from, kind).is_some()
    }

    /// Premier token `kind` de la zone dispositif, filtré des faux positifs
    /// positionnels (non-lieu sur dépens, prescription niée, confirmation
    /// partielle).
    fn disp_find(&self, from: usize, kind: Mk) -> Option<&PTok> {
        let end = self.dispositif_end(from);
        self.toks.iter().find(|t| {
            t.s >= from
                && t.s < end
                && t.kind == kind
                && match kind {
                    Mk::OutNonLieu => {
                        let a = self.span_after(t, 24);
                        let a = a.trim_start();
                        !(a.starts_with("sur les depens") || a.starts_with("sur les frais"))
                    }
                    Mk::OutIrrec => {
                        let b = self.span_before(t, 6);
                        !(b.ends_with("non ") || b.ends_with("pas "))
                    }
                    Mk::OutConfirme => {
                        let a = self.span_after(t, 18);
                        let a = a.trim_start();
                        !(a.starts_with("partiellement") || a.starts_with("en partie"))
                    }
                    Mk::OutRejette => {
                        // le NOM « rejet » en énumération (« propositions
                        // d'admission ou de rejet ») n'est pas un prononcé
                        !(fold_stable(&self.text_slice(t.s, t.e)) == "rejet"
                            && self.span_before(t, 6).ends_with("ou de "))
                    }
                    Mk::OutNeutral => {
                        // l'OBJET d'une demande rejetée n'est pas le sort :
                        // « rejetons la demande de radiation », « exception
                        // d'incompétence, n'y fait pas droit », « avant dire
                        // droit, rejette la mesure d'expertise »
                        let b = self.span_before(t, 14);
                        let objet = b.ends_with("demande de ")
                            || b.ends_with("demande d'")
                            || b.ends_with("mesure de ")
                            || b.ends_with("mesure d'")
                            || b.ends_with("exception de ")
                            || b.ends_with("exception d'");
                        let intro_rejet = {
                            let a = self.span_after(t, 12);
                            let a = a.trim_start_matches([',', ' ']);
                            a.starts_with("rejette")
                                || a.starts_with("rejetons")
                                || a.starts_with("deboute")
                        };
                        !(objet || intro_rejet)
                    }
                    _ => true,
                }
        })
    }

    /// Un rejet/débouté SUBSTANTIEL dans la zone dispositif : sa clause ne
    /// vise ni l'accessoire (art. 700, dépens, frais) ni le reliquat
    /// (« surplus », « plus amples », « toute autre »). Sert à l'école
    /// « mixte irrecevable + mal fondée → REJET » (le fond absorbe) : seul
    /// un rejet de tête de demande absorbe l'irrecevabilité.
    fn substantive_rejet(&self, from: usize) -> bool {
        let end = self.dispositif_end(from);
        let accessoire = |s: &str| {
            s.contains("article 700")
                || s.contains("depens")
                || s.contains("frais irrep")
                || s.contains("surplus")
                || s.contains("plus ample")
                || s.contains("toute autre")
                || s.contains("761-1")
        };
        self.toks.iter().any(|t| {
            t.s >= from && t.s < end && t.kind == Mk::OutRejette && {
                // « M. [X] » : le point d'abréviation ne clôt pas la clause
                let a = self.span_after(t, 120).replace(" m. ", " m  ");
                let clause = a.split(['.', ';']).next().unwrap_or("");
                // au PASSIF (« le surplus des conclusions est rejeté »),
                // l'objet précède le verbe : mêmes exclusions sur le
                // segment avant
                let b = self.span_before(t, 140).replace(" m. ", " m  ");
                let avant = b.rsplit(['.', ';']).next().unwrap_or("");
                let passif = avant.ends_with("est ")
                    || avant.ends_with("sont ")
                    || avant.ends_with("seront ")
                    || avant.ends_with("etre ");
                !(self.span_before(t, 6).ends_with("ou de ")
                    || accessoire(clause)
                    || (passif && accessoire(avant)))
            }
        })
    }

    /// Marque de partialité dans la CLAUSE (même phrase) d'un
    /// confirme/infirme. Après un CONFIRME, « en ce qu'il a… / en ses
    /// dispositions soumises à la cour » ÉNUMÈRE les chefs confirmés sans
    /// les restreindre — seuls « sauf en ce qu », « mais seulement »,
    /// « en ses seules dispositions »… rendent la confirmation partielle ;
    /// après un INFIRME, « en ce qu'il » désigne les chefs infirmés et
    /// reste une partialité. « sauf À + infinitif » (rectifier une erreur
    /// matérielle, moduler l'astreinte) ajuste sans infirmer : exclu.
    fn restrictif_apres(&self, c: &PTok, apres_confirme: bool) -> bool {
        self.toks.iter().any(|s| {
            matches!(s.kind, Mk::OutSauf | Mk::OutPartial)
                && s.s > c.e
                && s.s < c.e + 200
                && !self.text_slice(c.e, s.s).contains('.')
                && !(s.kind == Mk::OutSauf && self.span_after(s, 3).trim_start().starts_with("a "))
                && !(apres_confirme && {
                    let surf = fold_stable(&self.text_slice(s.s, s.e));
                    matches!(
                        surf.as_str(),
                        "en ce qu'il"
                            | "en ce qu'elle"
                            | "en tant qu'il"
                            | "en tant qu'elle"
                            | "en ses dispositions"
                    )
                })
        })
    }

    /// Un rejet de DEMANDE dans la zone dispositif : le rejet d'une DÉFENSE
    /// (délais de paiement, suspension des effets de la clause résolutoire,
    /// exception de procédure, note écartée des débats) ne rend pas la
    /// satisfaction du demandeur partielle — il a tout obtenu.
    fn rejet_de_demande(&self, from: usize) -> bool {
        let end = self.dispositif_end(from);
        self.toks.iter().any(|t| {
            t.s >= from && t.s < end && t.kind == Mk::OutRejette && {
                let a = self.span_after(t, 120).replace(" m. ", " m  ");
                let clause = a.split(['.', ';']).next().unwrap_or("");
                let b = self.span_before(t, 100);
                let avant = b.rsplit(['.', ';']).next().unwrap_or("");
                !(avant.ends_with("ou de ")
                    || avant.contains("exception")
                    || clause.contains("delais de paiement")
                    || clause.contains("delai de paiement")
                    || clause.contains("suspension des effets")
                    || clause.contains("des debats"))
            }
        })
    }

    /// Clé solution-17 lue dans la ZONE dispositif, `true` = la détection
    /// PRIME le label du greffe (désistement constaté, pourvoi non admis).
    /// L'ordre de composition suit le gabarit (le vocabulaire d'une cour de
    /// cassation n'est pas celui d'une cour d'appel ni d'une requête admin).
    pub fn outcome(&self) -> Option<(&'static str, bool)> {
        // Pourvoi non admis : décisif PARTOUT (la formule 567-1-1 CPP n'a pas
        // toujours de marqueur de dispositif) — exige « pourvoi » dans la
        // surface ou juste avant (« M. X n'est pas admis au bénéfice de
        // l'aide juridictionnelle » exclu).
        let non_admis = self.toks.iter().any(|t| {
            t.kind == Mk::OutNonAdmis && {
                let surf = fold_stable(&self.text_slice(t.s, t.e));
                if surf.contains("admission")
                    || self
                        .span_after(t, 16)
                        .trim_start()
                        .starts_with("au benefice")
                {
                    false
                } else {
                    surf.contains("pourvoi") || self.span_before(t, 160).contains("pourvoi")
                }
            }
        });
        if non_admis {
            return Some(("IRRECEVABILITE", true));
        }
        let from = self.dispositif_start();
        if from >= self.len() {
            return None;
        }
        if self.disp_has(from, Mk::OutDesist) {
            return Some(("DESISTEMENT", true));
        }
        match self.gabarit() {
            Gabarit::Cc => {
                if self.disp_has(from, Mk::OutNonLieu) {
                    return Some(("NON_LIEU_A_STATUER", false));
                }
                if self.disp_has(from, Mk::OutCasse) {
                    let part = self.disp_has(from, Mk::OutPartial);
                    return Some((
                        if part {
                            "CASSATION_PARTIELLE"
                        } else {
                            "CASSATION"
                        },
                        false,
                    ));
                }
                if self.disp_has(from, Mk::OutNeutral) || self.disp_has(from, Mk::OutJonction) {
                    return Some(("AUTRE", false));
                }
                // mixte irrecevable + rejet au fond : le fond absorbe (école
                // gold) — « déclare irrecevable le pourvoi de X, rejette le
                // pourvoi de Y » = REJET ; le rejet ACCESSOIRE (art. 700,
                // dépens, surplus) n'absorbe pas.
                if self.disp_has(from, Mk::OutIrrec) && !self.substantive_rejet(from) {
                    return Some(("IRRECEVABILITE", false));
                }
                if self.disp_has(from, Mk::OutRejette) {
                    return Some(("REJET", false));
                }
                None
            }
            Gabarit::Blocs => {
                let end = self.dispositif_end(from);
                // condamnation substantielle (« à payer/verser…/somme de/
                // dommages » dans la clause) vs procédurale (art. 700/dépens)
                let cond = self.toks.iter().any(|t| {
                    t.s >= from && t.s < end && t.kind == Mk::OutCondamne && {
                        // « M. [X] » : le point d'abréviation ne clôt pas la clause
                        let a = self.span_after(t, 220).replace(" m. ", " m  ");
                        let clause = a.split(['.', ';']).next().unwrap_or("");
                        (clause.contains("a payer")
                            || clause.contains("a verser")
                            || clause.contains("a porter")
                            || clause.contains("somme de")
                            || clause.contains("dommages"))
                            && !clause.contains("article 700")
                            && !clause.contains("depens")
                            && !clause.contains("frais irrep")
                    }
                });
                // faire droit à la demande sans condamner : ouverture de
                // procédure collective, sanction de gestion, mainlevée JLD,
                // ordonnance commune / prorogation en référé (gold : la
                // demande accueillie = SATISFACTION, pas AUTRE)
                let grant = self.disp_has(from, Mk::OutGrant);
                let conf = self.disp_find(from, Mk::OutConfirme).cloned();
                // « confirme partiellement » = infirmation partielle en soi
                let conf_partial = self.toks.iter().any(|t| {
                    t.s >= from && t.s < end && t.kind == Mk::OutConfirme && {
                        let a = self.span_after(t, 18);
                        let a = a.trim_start();
                        a.starts_with("partiellement") || a.starts_with("en partie")
                    }
                });
                let inf = self.disp_find(from, Mk::OutInfirme).cloned();
                match (&conf, &inf) {
                    (Some(_), Some(_)) => return Some(("INFIRMATION_PARTIELLE", false)),
                    _ if conf_partial => return Some(("INFIRMATION_PARTIELLE", false)),
                    (Some(c), None) if self.restrictif_apres(c, true) => {
                        return Some(("INFIRMATION_PARTIELLE", false))
                    }
                    (None, Some(i)) if self.restrictif_apres(i, false) => {
                        return Some(("INFIRMATION_PARTIELLE", false))
                    }
                    (None, Some(_)) => return Some(("INFIRMATION", false)),
                    (Some(_), None) => return Some(("CONFIRMATION", false)),
                    (None, None) => {}
                }
                if self.disp_has(from, Mk::OutCasse) {
                    let part = self.disp_has(from, Mk::OutPartial);
                    return Some((
                        if part {
                            "CASSATION_PARTIELLE"
                        } else {
                            "CASSATION"
                        },
                        false,
                    ));
                }
                // non-lieu décisif seulement quand le dispositif n'infirme
                // ni ne confirme (catégorie la plus spécifique), ne condamne,
                // n'accueille ni ne rejette rien de substantiel par ailleurs
                // (« dit n'y avoir lieu à statuer sur X, condamne Y…,
                // déboute Z… » est un sort mixte, pas un non-lieu)
                if self.disp_has(from, Mk::OutNonLieu)
                    && !cond
                    && !grant
                    && !self.substantive_rejet(from)
                {
                    return Some(("NON_LIEU_A_STATUER", false));
                }
                // Irrecevabilité PRONONCÉE seulement : « recevable et non
                // prescrite » (action déclarée recevable) ne compte pas.
                let irrec = self.toks.iter().any(|t| {
                    t.s >= from && t.s < end && t.kind == Mk::OutIrrec && {
                        let b = self.span_before(t, 6);
                        !(b.ends_with("non ") || b.ends_with("pas "))
                    }
                });
                // mixte irrecevable + rejet au fond : le fond absorbe (école
                // gold) — « déclare irrecevable l'intervention…, déboute X de
                // l'ensemble de ses demandes » = REJET, pas IRRECEVABILITE ;
                // le rejet ACCESSOIRE (art. 700, dépens, surplus) n'absorbe pas.
                if irrec && !self.substantive_rejet(from) {
                    return Some(("IRRECEVABILITE", false));
                }
                let rej = self.disp_has(from, Mk::OutRejette);
                // cond/grant priment le neutre (un dispositif qui condamne à
                // payer ou accueille la demande n'est pas AUTRE parce qu'il
                // renvoie aussi à une mise en état) ; le rejet NU reste
                // derrière lui (« sursoit à statuer… rejette le surplus »
                // demeure AUTRE)
                if (cond || grant) && rej && self.rejet_de_demande(from) {
                    return Some(("SATISFACTION_PARTIELLE", false));
                }
                if cond || grant {
                    return Some(("SATISFACTION_TOTALE", false));
                }
                if self.disp_has(from, Mk::OutNeutral) {
                    return Some(("AUTRE", false));
                }
                if rej {
                    return Some(("REJET", false));
                }
                if self.disp_has(from, Mk::OutJonction) {
                    return Some(("AUTRE", false));
                }
                None
            }
            Gabarit::Admin => {
                // École gold : le dispositif se lit VERBATIM. Jugement ou acte
                // annulé, en tout ou partie → ANNULATION (« réformé » →
                // REFORMATION) ; requête rejetée → REJET, SAUF irrecevabilité
                // PRONONCÉE (école 2026-07-09 : « rejetée comme
                // (manifestement) irrecevable », motifs de clôture R. 222-1
                // — cf. `irrec_pronounced` ; le mixte fond + irrecevabilité
                // reste REJET) ; SATISFACTION_* = plein contentieux gagné
                // SANS annulation (condamnation à payer, décharge).
                if self.disp_has(from, Mk::OutAnnule) {
                    return Some(("ANNULATION", false));
                }
                // Ordonnances JUDICIAIRES sans étiquettes de bloc de greffe
                // (JLD rétention/hospitalisation, référés) routées ici faute
                // de pivot : « confirme/confirmons » n'existe pas dans un
                // dispositif administratif (gold : 166 CONFIRMATION
                // judiciaires, 0 admin) — lecture appel judiciaire, mêmes
                // règles de partialité que le gabarit Blocs.
                if let Some(c) = self.disp_find(from, Mk::OutConfirme).cloned() {
                    let a = self.span_after(&c, 18);
                    let a = a.trim_start();
                    let partial = a.starts_with("partiellement")
                        || a.starts_with("en partie")
                        || self.disp_has(from, Mk::OutInfirme)
                        || self.restrictif_apres(&c, true);
                    return Some((
                        if partial {
                            "INFIRMATION_PARTIELLE"
                        } else {
                            "CONFIRMATION"
                        },
                        false,
                    ));
                }
                if let Some(t) = self.disp_find(from, Mk::OutInfirme) {
                    let surf = fold_stable(&self.text_slice(t.s, t.e));
                    let key = if surf.starts_with("reform") {
                        "REFORMATION"
                    } else {
                        "ANNULATION"
                    };
                    return Some((key, false));
                }
                // non-lieu partiel + rejet SUBSTANTIEL = rejet ; le rejet
                // du seul accessoire (« le surplus des conclusions est
                // rejeté », art. 700/761-1) laisse le non-lieu principal
                if self.disp_has(from, Mk::OutNonLieu) {
                    if self.substantive_rejet(from) {
                        return Some(("REJET", false));
                    }
                    return Some(("NON_LIEU_A_STATUER", false));
                }
                let end = self.dispositif_end(from);
                let cond = self.toks.iter().any(|t| {
                    t.s >= from && t.s < end && t.kind == Mk::OutCondamne && {
                        // « M. [X] » : le point d'abréviation ne clôt pas la clause
                        let a = self.span_after(t, 220).replace(" m. ", " m  ");
                        let clause = a.split(['.', ';']).next().unwrap_or("");
                        (clause.contains("a payer")
                            || clause.contains("a verser")
                            || clause.contains("a porter")
                            || clause.contains("somme de")
                            || clause.contains("dommages"))
                            && !clause.contains("article 700")
                            && !clause.contains("761-1")
                            && !clause.contains("depens")
                            && !clause.contains("frais irrep")
                    }
                });
                let cond = cond || self.disp_has(from, Mk::OutGrant);
                let rej = self.disp_has(from, Mk::OutRejette);
                if cond && rej {
                    return Some(("SATISFACTION_PARTIELLE", false));
                }
                if cond {
                    return Some(("SATISFACTION_TOTALE", false));
                }
                if rej {
                    if self.irrec_pronounced(from) || self.closing_irrec(from) {
                        return Some(("IRRECEVABILITE", false));
                    }
                    return Some(("REJET", false));
                }
                if self.disp_has(from, Mk::OutIrrec) {
                    return Some(("IRRECEVABILITE", false));
                }
                if self.disp_has(from, Mk::OutNeutral) {
                    return Some(("AUTRE", false));
                }
                None
            }
        }
    }

    /// Irrecevabilité PRONONCÉE (école gold 2026-07-09) : un token
    /// d'irrecevabilité et un token de rejet dans la MÊME phrase, entre la
    /// clause de clôture des motifs (≤ 300 chars avant le dispositif) et la
    /// fin du dispositif — « manifestement irrecevable et doit être
    /// rejetée » (R. 222-1 motivé ou non), « rejette la requête comme
    /// irrecevable ». Les fins de non-recevoir discutées plus haut ne
    /// s'apparient pas : le mixte reste REJET, le fond absorbe. Frontière de
    /// phrase = « . » précédé d'autre chose qu'une majuscule (« R. 222-1 »,
    /// « M. » ne coupent pas).
    fn irrec_pronounced(&self, from: usize) -> bool {
        let end = self.dispositif_end(from);
        let zone = from.saturating_sub(300);
        // Même chaîne de prononcé : pas de frontière de phrase entre les
        // deux tokens — « . » ne coupe pas après une majuscule ou un point
        // (« R. 222-1 », « M. », ellipse « Mme A... »), ni quand la phrase
        // suivante enchaîne la conséquence (« irrecevable. Par suite, il y a
        // lieu de rejeter… »).
        let same_reasoning = |a: usize, b: usize| {
            let slice = self.text_slice(a, b);
            let chars: Vec<char> = slice.chars().collect();
            for i in 0..chars.len() {
                let prev = if i == 0 { ' ' } else { chars[i - 1] };
                if chars[i] != '.' || prev.is_ascii_uppercase() || prev == '.' {
                    continue;
                }
                let rest: String = chars[i + 1..].iter().collect();
                let rest = fold_stable(rest.trim_start());
                const CHAINE: &[&str] = &[
                    "par suite",
                    "des lors",
                    "il suit de la",
                    "il resulte",
                    "par consequent",
                    "il y a lieu",
                ];
                if !CHAINE.iter().any(|p| rest.starts_with(p)) {
                    return false;
                }
            }
            true
        };
        let irr: Vec<&PTok> = self
            .toks
            .iter()
            .filter(|t| {
                t.kind == Mk::OutIrrec && t.s >= zone && t.s < end && {
                    let b = self.span_before(t, 6);
                    // « créance prescrite » (prescription quadriennale) est un
                    // moyen de FOND : la surface prescrit* ne prononce pas
                    // d'irrecevabilité (arbitrage école 2026-07-09).
                    !(b.ends_with("non ")
                        || b.ends_with("pas ")
                        || fold_stable(&self.text_slice(t.s, t.e)).starts_with("prescrit"))
                }
            })
            .collect();
        if irr.is_empty() {
            return false;
        }
        self.toks
            .iter()
            .filter(|t| t.kind == Mk::OutRejette && t.s >= zone && t.s < end)
            .any(|r| {
                irr.iter().any(|i| {
                    let (a, b) = if i.e <= r.s { (i.e, r.s) } else { (r.e, i.s) };
                    a <= b && b - a <= 260 && same_reasoning(a, b)
                })
            })
    }

    /// Irrecevabilité prononcée dans les MOTIFS DE CLÔTURE (≤ 800 chars
    /// avant le dispositif, chaque phrase étendue à sa vraie frontière) —
    /// complète `irrec_pronounced` pour les ordonnances dont le prononcé
    /// (« la requête est manifestement irrecevable », « n'est pas
    /// recevable », « tardive », rejet au 4° de l'article R. 222-1) précède
    /// des paragraphes de conséquence/frais avant un dispositif « rejette »
    /// nu. La phrase doit viser l'objet contentieux (requête / conclusions /
    /// recours / pourvoi) ; sont exclus : l'irrecevabilité d'un MOYEN
    /// (mixte, le fond absorbe), la citation de la règle (« aux termes »),
    /// l'attribution au premier juge (« c'est à bon droit que… a rejeté » —
    /// l'appel confirmant une irrecevabilité de première instance est une
    /// école gold non tranchée, codée tantôt IRRECEVABILITE tantôt REJET),
    /// et tout marqueur de fond (« dépourvue de fondement », « mal fondée »)
    /// entre le prononcé et le dispositif.
    fn closing_irrec(&self, from: usize) -> bool {
        let chars = &self.norm.chars;
        let win = from.saturating_sub(800);
        let lo = from.saturating_sub(1200);
        // Frontière de phrase : « . » précédé d'un mot alphanumérique d'au
        // moins 2 chars — « R. 222-1 », « M. », « 2. » (numéro de point) et
        // « () ". » ne coupent pas.
        let is_boundary = |i: usize| {
            if chars[i] != '.' {
                return false;
            }
            let mut j = i;
            while j > lo && chars[j - 1].is_alphanumeric() {
                j -= 1;
            }
            i - j > 1
        };
        let mut starts = vec![lo];
        for i in lo..from {
            if is_boundary(i) && i + 1 < from {
                starts.push(i + 1);
            }
        }
        for (k, &sa) in starts.iter().enumerate() {
            let sb = starts.get(k + 1).map_or(from, |n| n - 1);
            if sb <= win {
                continue;
            }
            let sent = fold_stable(&self.text_slice(sa, sb));
            let vocab = ["irrecevab", "pas recevable", "non recevable", "tardiv"]
                .iter()
                .filter_map(|v| sent.find(v))
                .find(|&m| {
                    let pre = &sent[..m];
                    !(pre.ends_with("non ") || pre.ends_with("pas ") || pre.ends_with("nullement "))
                })
                .or_else(|| {
                    // renvoi explicite au 4° (requêtes manifestement
                    // irrecevables) dans la phrase qui rejette
                    (sent.contains("4° de l'article r. 222-1") && sent.contains("rejet"))
                        .then(|| sent.find("4°").unwrap())
                });
            let Some(_) = vocab else { continue };
            if !(sent.contains("requete")
                || sent.contains("conclusions")
                || sent.contains("recours")
                || sent.contains("pourvoi"))
            {
                continue;
            }
            const BLOCK: &[&str] = &[
                "moyen",
                "bon droit",
                "a tort",
                "premiers juges",
                "a rejete",
                "ont rejete",
                "a pu ",
                "ecarte",
                "aux termes",
                "peuvent, par ordonnance",
                "peuvent par ordonnance",
                "permettent de rejeter",
                "permet de rejeter",
            ];
            if BLOCK.iter().any(|b| sent.contains(b)) {
                continue;
            }
            let to_disp = fold_stable(&self.text_slice(sb, from));
            if to_disp.contains("sur les conclusions") {
                continue;
            }
            let span = fold_stable(&self.text_slice(sa, from));
            if span.contains("depourvue de fondement")
                || span.contains("depourvues de fondement")
                || span.contains("mal fonde")
                || span.contains("pas fonde")
            {
                continue;
            }
            return true;
        }
        false
    }

    /// La cassation est-elle PARTIELLE ? (indice dans le dispositif, ou
    /// « casse … <partiel> » dans les 300 chars du verbe, n'importe où.)
    /// « casse et annule, dans/en toutes ses dispositions » force la
    /// TOTALITÉ : la formule de transcription du greffe (« en marge … de
    /// l'arrêt partiellement cassé ») et le « en tant qu'il » d'une clause
    /// de rejet voisine ne la contredisent pas.
    pub fn cassation_partial(&self) -> bool {
        let from = self.dispositif_start();
        let end = self.dispositif_end(from);
        let toutes = self.toks.iter().any(|t| {
            t.s >= from && t.s < end && t.kind == Mk::OutCasse && {
                let a = fold_stable(self.span_after(t, 40).trim_start());
                a.starts_with(", dans toutes ses dispositions")
                    || a.starts_with(", en toutes ses dispositions")
                    || a.starts_with("dans toutes ses dispositions")
                    || a.starts_with("en toutes ses dispositions")
            }
        });
        if toutes {
            return false;
        }
        if self
            .toks
            .iter()
            .any(|t| t.s >= from && t.s < end && t.kind == Mk::OutPartial)
        {
            return true;
        }
        self.toks.iter().any(|t| {
            t.kind == Mk::OutCasse
                && t.s < end
                && self
                    .toks
                    .iter()
                    .any(|p| p.kind == Mk::OutPartial && p.s > t.e && p.s < t.e + 300)
        })
    }

    /// Le gabarit détecté est-il celui d'un pourvoi en cassation ? (Le
    /// vocabulaire de sortie de la solution en dépend : gagner = casser.)
    pub fn gabarit_cc(&self) -> bool {
        self.gabarit() == Gabarit::Cc
    }

    /// « pourvoi » dans l'EN-TÊTE — pourvoi en cassation administratif (CE)
    /// dont le gabarit reste Admin (aucun pivot judiciaire) : le vocabulaire
    /// de sortie devient celui de la cassation (annuler = casser).
    pub fn header_has_pourvoi(&self) -> bool {
        let end = self.motifs_start();
        fold_stable(&self.text_slice(0, end)).contains("pourvoi")
    }

    /// Signaux procéduraux lus dans le texte — combinés aux métadonnées par
    /// `extract::extract_procedure`. Zone en-tête sauf mention contraire.
    pub fn procedure_signals(&self) -> ProcSignals {
        let header = self.motifs_start();
        let bandeau = self.bandeau_end();
        let has_h = |k: Mk| self.find_tok(&[k], 0, header).is_some();
        let has_b = |k: Mk| self.find_tok(&[k], 0, bandeau).is_some();
        let has_any = |k: Mk| self.toks.iter().any(|t| t.kind == k);
        // R222-1 qualifié : « dernier alinéa / alinéa 4 » devant, ou
        // manifestement/irrecevable/tardive/hors délai/sans ministère
        // d'avocat dans les 180 chars suivants (n'importe où dans le doc,
        // les ordonnances de filtrage motivent en une phrase)
        let filtrage = self.toks.iter().any(|t| {
            (t.kind == Mk::ProcFiltrage && {
                let b = self.span_before(t, 100);
                let a = self.span_after(t, 180);
                b.contains("dernier alinea")
                    || b.contains("alinea 4")
                    // désignation « pour statuer par ordonnance en
                    // application de l'article R. 222-1 »
                    || b.contains("par ordonnance en application")
                    || a.contains("manifestement")
                    || a.contains("irrecevab")
                    || a.contains("tardivet")
                    || a.contains("hors delai")
                    || a.contains("sans le ministere d'avocat")
                    || a.contains("sans ministere d'avocat")
                    // citations élidées de l'article : « peuvent () par
                    // ordonnance, rejeter », 6° séries, 7° délégation
                    || a.contains("par ordonnance, rejeter")
                    || a.contains("relevant d'une serie")
                    || a.contains("\" 7°")
            })
                // « manifestement irrecevable » / « irrecevabilité manifeste »
                // sans citation R. 222-1 : la moitié des ordonnances de
                // filtrage motivent sans viser l'article (le gate ordonnance
                // ORTA_/ORCA_ vit dans `procedure_key`). Lu sur le token OutIrrec —
                // une surface composée volerait le token au leftmost-longest.
                || (t.kind == Mk::OutIrrec
                    && (self.span_before(t, 20).trim_end().ends_with("manifestement")
                        || self.span_after(t, 12).trim_start().starts_with("manifeste")))
        });
        // formule ACTIVE de désignation en tête : « le président de la cour /
        // du tribunal a désigné M./Mme X » suivie de l'objet de la délégation
        // (référés, R. 222-1, « pour statuer ») — c'est le juge unique qui
        // rend LA présente décision (le passif « désigné par » raconte le
        // jugement attaqué et ne produit pas ce token)
        let magdes_form = |who: &str| {
            self.toks.iter().any(|t| {
                t.s < header && t.kind == Mk::ProcMagdesForm && {
                    let b = self.span_before(t, 140);
                    let a = self.span_after(t, 220);
                    b.contains("president")
                        && b.contains(who)
                        && (a.contains("refere")
                            || a.contains("r. 222-1")
                            || a.contains("pour statuer")
                            || a.contains("pouvoirs prevus"))
                }
            })
        };
        ProcSignals {
            // bandeau seulement : cité plus bas, QPC/non-admission décrivent
            // la procédure antérieure (« demande la transmission d'une QPC »)
            qpc: has_b(Mk::ProcQpc),
            // formules PAPC DÉCISIVES seulement (« l'admission est refusée »,
            // « non spécialement motivé ») ; « NON-ADMISSION » au bandeau CC.
            // Les citations L822-1/R822-5 et « procédure préalable
            // d'admission » sont du boilerplate de visa (cassation des
            // référés) — jamais décisives seules. Gate juridiction (CC/CE)
            // dans `extract_procedure`.
            papc: self.toks.iter().any(|t| {
                t.kind == Mk::ProcPapc && {
                    let s = fold_stable(&self.text_slice(t.s, t.e));
                    s.starts_with("l'admission") || s.starts_with("non specialement")
                }
            }) || has_b(Mk::OutNonAdmis),
            filtrage,
            refere_suspension: has_h(Mk::ProcRefSusp),
            refere_liberte: has_h(Mk::ProcRefLib),
            refere_utiles: has_h(Mk::ProcRefUtile),
            refere_precontractuel: has_h(Mk::ProcRefPrecontr) && !has_h(Mk::ProcImmig),
            refere_provision: has_h(Mk::ProcRefProv),
            refere_cour: has_h(Mk::ProcRefCour),
            desist_bandeau: has_b(Mk::OutDesist),
            // référé judiciaire dit par le texte. « Vu l'assignation en
            // référé » en tête (visa des ordonnances de premier président —
            // l'assignation narrative désigne souvent un référé antérieur,
            // expertise…) ; « ordonnance de référé » nue au bandeau (titre de
            // la décision ou de la décision déférée) ou précédée de
            // appel d'une / confirme l' / réforme l' (l'instance EST la ligne
            // de référé, dispositif inclus)
            refere_civil: self.toks.iter().any(|t| {
                (t.kind == Mk::ProcRefCivil && {
                    let surf = fold_stable(&self.text_slice(t.s, t.e));
                    if surf == "assignation en refere" {
                        t.s < header && self.span_before(t, 6).ends_with("vu l'")
                    } else {
                        // « Par ordonnance de référé du …, X a été désigné » :
                        // récit d'un référé antérieur, même sous le bandeau
                        (t.s < bandeau && !self.span_before(t, 4).ends_with("par "))
                            || {
                            let b = self.span_before(t, 14);
                            b.ends_with("appel d'une ")
                                || b.ends_with("confirme l'")
                                || b.ends_with("reforme l'")
                        }
                    }
                })
                    // « a fait assigner X devant le juge des référés » en tête
                    || (t.kind == Mk::ProcJref
                        && t.s < header
                        && self.span_before(t, 70).contains("assigner")
                        && self.span_before(t, 11).ends_with("devant le "))
            }),
            retention: has_h(Mk::ProcRetention) || has_h(Mk::ProcImmig),
            retention_anywhere: has_any(Mk::ProcRetention),
            hospi: has_h(Mk::ProcHospi),
            hospi_anywhere: has_any(Mk::ProcHospi),
            rectification: has_h(Mk::ProcRectif),
            // « premier président de la cour » n'est un office que dans la
            // formule d'ordonnance « NOUS, X, Premier Président de la cour »
            premier_president: self.toks.iter().any(|t| {
                t.s < header && t.kind == Mk::ProcPremPres && {
                    let surf = fold_stable(&self.text_slice(t.s, t.e));
                    surf != "premier president de la cour"
                        || self.span_before(t, 60).contains("nous")
                }
            }),
            // « magistrat désigné » adjectival partout ; « président
            // désigné » seulement clos par ponctuation (signature) — la même
            // surface pliée couvre le verbe (« le président désigne Mme X »)
            magdes: self.toks.iter().any(|t| {
                t.kind == Mk::ProcMagdes && {
                    let surf = fold_stable(&self.text_slice(t.s, t.e));
                    !surf.starts_with("president")
                        || self
                            .span_after(t, 3)
                            .trim_start()
                            .starts_with([',', '.', ';', ')'])
                }
            }),
            // « …désigné » en pied de décision = signature du magistrat qui a
            // RENDU l'ordonnance ; les occurrences d'en-tête désignent le juge
            // du jugement attaqué
            magdes_tail: self
                .toks
                .iter()
                .any(|t| t.kind == Mk::ProcMagdes && t.s + 700 >= self.len()),
            magdes_form_cour: magdes_form("de la cour"),
            magdes_form_trib: magdes_form("du tribunal"),
            // « demande(nt) au juge des référés » en tête : la requête est
            // adressée AU juge des référés de la juridiction saisie — il
            // statue seul (L. 511-2 CJA). Le récit d'appel désigne l'autre
            // juridiction : gabarit première instance seulement, côté
            // `extract`.
            jref_demande: self.toks.iter().any(|t| {
                t.s < header && t.kind == Mk::ProcJref && {
                    let b = self.span_before(t, 14);
                    b.ends_with("demande au ") || b.ends_with("demandent au ")
                }
            }),
            // « au juge des référés du Conseil d'État » : premier ressort ou
            // appel L. 521-2 — le juge des référés du CE statue seul ; le
            // récit d'un pourvoi ne porte pas cette adresse.
            jref_conseil: self.toks.iter().any(|t| {
                t.s < header
                    && t.kind == Mk::ProcJref
                    && self
                        .span_after(t, 12)
                        .trim_start()
                        .starts_with("du conseil")
            }),
            // JEX opérationnel seulement : précédé de « jugement du / rendu
            // par le / décision du… », c'est la décision ATTAQUÉE ou
            // l'historique (cassation/appel d'un jugement JEX), pas l'office
            jex: self.toks.iter().any(|t| {
                t.kind == Mk::ProcJex && {
                    let b = self.span_before(t, 30);
                    let b = b.trim_end();
                    ![
                        "jugement du",
                        "jugement d'un",
                        "rendu par le",
                        "rendue par le",
                        "decision du",
                        "arret du",
                        "ordonnance du",
                        "a saisi le",
                        "de saisir le",
                    ]
                    .iter()
                    .any(|p| b.ends_with(p))
                }
            }) && !has_any(Mk::ProcJexFalse),
            jex_saisie_immo: self.toks.iter().any(|t| {
                t.kind == Mk::ProcJex && {
                    let s = fold_stable(&self.text_slice(t.s, t.e));
                    s.starts_with("saisie")
                }
            }),
            proc_collective: has_h(Mk::ProcCollective),
            // vocabulaire domaine en-tête (objet de la requête) ; immig
            // partout — OQTF/CESEDA sont univoques où qu'ils apparaissent
            dom_fp: has_h(Mk::ProcDomFp),
            dom_aide: has_h(Mk::ProcDomAide),
            dom_urba: has_h(Mk::ProcDomUrba),
            dom_etr: has_h(Mk::ProcDomEtr),
            dom_fisc: has_h(Mk::ProcDomFisc),
            dom_env: has_h(Mk::ProcDomEnv),
            dom_penal_pub: has_h(Mk::ProcDomPenalPub),
            immig_anywhere: has_any(Mk::ProcImmig),
        }
    }

    /// Gabarit admin : requérants de TOUS les blocs de requête de l'en-tête
    /// (« 1° Sous le n°…, par une requête…, X et Y demandent… 2° Sous le
    /// n°… »), chacun borné à son verbe « demande(nt) ». Sans ouverture de
    /// bloc détectée : en-tête simple borné au premier « demande ».
    pub fn admin_companies(&self) -> Vec<String> {
        let end = self.motifs_start();
        let reqs: Vec<usize> = self
            .toks
            .iter()
            .filter(|t| t.kind == Mk::AdminReq && t.s < end)
            .map(|t| t.e)
            .collect();
        if reqs.is_empty() {
            // corps de moyens CC sans préambule (« LA COUR DE CASSATION…
            // a rendu l'arrêt suivant : Sur le moyen unique : Attendu
            // que… ») : le texte ouvre sur la prose des motifs, il n'y a
            // pas d'en-tête de parties à moissonner.
            if self.toks.iter().any(|t| t.kind == Mk::DefEnd && t.s < 300) {
                return Vec::new();
            }
            let b = self.demande_boundary(0, end);
            return self.harvest(0, b, b);
        }
        let mut out: Vec<String> = Vec::new();
        for (i, &rs) in reqs.iter().enumerate() {
            let next = reqs.get(i + 1).copied().unwrap_or(end);
            let seg_end = self.demande_boundary(rs, next);
            for name in self.harvest(rs, seg_end, seg_end) {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out
    }

    /// Gabarit structurel du document, lu dans le flux de tokens de
    /// l'EN-TÊTE : un pivot de pourvoi cité dans les motifs d'une décision du
    /// fond ne doit pas router vers CC.
    fn gabarit(&self) -> Gabarit {
        let header_end = self.motifs_start();
        let pivot = self
            .find_tok(&[Mk::PivotNew, Mk::PivotOld], 0, header_end)
            .or_else(|| self.joint_pivot(header_end));
        let first_block = self
            .toks
            .iter()
            .find(|t| matches!(t.kind, Mk::BlockApp | Mk::BlockDef))
            .map(|t| t.s);
        match (pivot, first_block) {
            (Some(p), Some(b)) if p.s < b => Gabarit::Cc,
            (Some(_), None) => Gabarit::Cc,
            (_, Some(_)) => Gabarit::Blocs,
            (None, None) => Gabarit::Admin,
        }
    }

    /// Point d'entrée unique : (demandeurs, défendeurs) — le gabarit est
    /// détecté depuis le flux de tokens lui-même, pas depuis la source :
    /// pivot de pourvoi → CC ; étiquettes de bloc de greffe → judiciaire du
    /// fond ; ouvertures de requête → admin (défendeur = administration,
    /// jamais une société).
    pub fn companies(&self) -> (Vec<String>, Vec<String>) {
        use crate::registry::{Quality, Side};
        let r = self.party_registry();
        (
            r.view(Some(Side::Applicant), Quality::Party),
            r.view(Some(Side::Defendant), Quality::Party),
        )
    }

    /// Registre de parties mémoïsé (ADR 0175 V0) : construit UNE fois depuis
    /// les moissons par gabarit ; `companies`/`counsel`/`intervenors` en sont
    /// des projections.
    pub fn party_registry(&self) -> &crate::registry::PartyRegistry {
        self.registry_memo.get_or_init(|| {
            use crate::registry::{PartyRegistry, Quality, Side};
            let mut r = PartyRegistry::default();
            let (apps, defs) = self.companies_uncached();
            r.push(apps, Some(Side::Applicant), Quality::Party);
            r.push(defs, Some(Side::Defendant), Quality::Party);
            let c = self.counsel_uncached();
            r.push(
                c.applicant_names,
                Some(Side::Applicant),
                Quality::CounselName,
            );
            r.push(c.applicant_firms, Some(Side::Applicant), Quality::LawFirm);
            r.push(
                c.defendant_names,
                Some(Side::Defendant),
                Quality::CounselName,
            );
            r.push(c.defendant_firms, Some(Side::Defendant), Quality::LawFirm);
            r.push(self.intervenors_uncached(), None, Quality::Intervenor);
            r
        })
    }

    fn companies_uncached(&self) -> (Vec<String>, Vec<String>) {
        let (mut apps, mut defs) = match self.gabarit() {
            Gabarit::Cc => self.cc_companies(),
            Gabarit::Blocs => self.block_companies(),
            Gabarit::Admin => (self.admin_companies(), Vec::new()),
        };
        // arbitrage de qualité par force du signal PAR MENTION : une entité
        // dont toutes les mentions fortes sont côté cabinet n'est pas une
        // partie (les ambigus restent en place)
        apps.retain(|n| !self.drop_from_companies(n));
        defs.retain(|n| !self.drop_from_companies(n));
        (apps, defs)
    }

    /// Conseils (avocats + cabinets) par côté — mêmes gabarits structurels que
    /// [`Self::companies`], tranches verbatim.
    pub fn counsel(&self) -> CounselOut {
        use crate::registry::{Quality, Side};
        let r = self.party_registry();
        CounselOut {
            applicant_names: r.view(Some(Side::Applicant), Quality::CounselName),
            applicant_firms: r.view(Some(Side::Applicant), Quality::LawFirm),
            defendant_names: r.view(Some(Side::Defendant), Quality::CounselName),
            defendant_firms: r.view(Some(Side::Defendant), Quality::LawFirm),
        }
    }

    fn counsel_uncached(&self) -> CounselOut {
        let mut out = CounselOut::default();
        match self.gabarit() {
            Gabarit::Cc => self.cc_counsel(&mut out),
            Gabarit::Blocs => self.block_counsel(&mut out),
            Gabarit::Admin => self.admin_counsel(&mut out),
        }
        // moyens annexés CC (« Moyen produit par la SCP X … pour la société
        // Y ») : filet cabinet demandeur quand le préambule n'en nomme pas
        if out.applicant_firms.is_empty() {
            out.applicant_firms = self.moyen_par_firms();
        }
        // arbitrage de qualité par force du signal PAR MENTION : une entité
        // dont toutes les mentions fortes sont côté partie n'est pas un
        // cabinet (les ambigus restent en place)
        out.applicant_firms.retain(|f| !self.drop_from_firms(f));
        out.defendant_firms.retain(|f| !self.drop_from_firms(f));
        out
    }

    /// Longueur du texte scanné (chars).
    pub fn text_len(&self) -> usize {
        self.len()
    }

    fn skip_spaces(&self, mut i: usize, to: usize) -> usize {
        while i < to.min(self.len()) && self.norm.chars[i] == ' ' {
            i += 1;
        }
        i
    }

    /// Ancre de jonction ancienne « Joint les pourvois n° U formé par… » —
    /// pivot CC seulement quand la clause énumère ses demandeurs (« joint
    /// les pourvois n° T au n° W » nu ne nomme personne).
    fn joint_pivot(&self, end: usize) -> Option<&PTok> {
        let j = self.find_tok(&[Mk::JointPourvois], 0, end)?;
        let w = fold_stable(&self.text_slice(j.e, (j.e + 60).min(end)));
        (w.contains("forme par") || w.contains("formes par")).then_some(j)
    }

    /// Le pivot ouvre-t-il un pourvoi incident/provoqué ? (« a formé un
    /// pourvoi incident contre le même arrêt ») — ces pivots n'ouvrent pas
    /// de zone : les rôles restent ceux des pourvois principaux (convention
    /// gold).
    fn pivot_incident(&self, p: &PTok) -> bool {
        let tail: String = self.norm.chars[p.e..(p.e + 20).min(self.len())]
            .iter()
            .collect();
        let tail = tail.to_lowercase();
        tail.contains("incident") || tail.contains("provoqu")
    }

    /// Segments demandeurs/défendeurs du préambule CC — `None` = pas de
    /// pivot. Un arrêt de jonction porte plusieurs pourvois principaux
    /// (« I - Statuant sur le pourvoi n° X formé par… II - … ») : une zone
    /// demandeurs et une fenêtre défendeurs PAR pivot.
    fn cc_segments(&self) -> Option<CcSegs> {
        let end = self.motifs_start();
        let pivots: Vec<PTok> = self
            .toks
            .iter()
            .filter(|t| {
                t.s < end
                    && matches!(t.kind, Mk::PivotNew | Mk::PivotOld)
                    && !self.pivot_incident(t)
            })
            .cloned()
            .collect();
        if pivots.is_empty() {
            // jonction ancienne sans pivot : « Joint les pourvois n° U
            // formé par X, n° A formé par Y… ; » — l'ancre de jonction
            // ouvre la zone demandeurs, bornée à la PREMIÈRE frontière
            // structurelle (un « contre » de prose en aval ne la porte pas)
            let joint = self.joint_pivot(end)?.clone();
            let seg_end = [
                self.find_tok(&[Mk::Contre], joint.e, end),
                self.find_tok(&[Mk::Opposant, Mk::DefEnd], joint.e, end),
            ]
            .into_iter()
            .flatten()
            .map(|t| t.s)
            .min()
            .unwrap_or(end);
            let def_from = self
                .find_tok(&[Mk::Opposant], seg_end, end)
                .map(|t| t.e)
                .unwrap_or(seg_end);
            let def_to = self
                .find_tok(&[Mk::DefEnd], def_from, end)
                .map(|t| t.s)
                .unwrap_or(end);
            return Some(CcSegs {
                app: vec![(joint.e, seg_end)],
                def: vec![(def_from, def_to)],
            });
        }
        let mut app: Vec<(usize, usize)> = Vec::new();
        let mut def: Vec<(usize, usize)> = Vec::new();
        for (k, p) in pivots.iter().enumerate() {
            if let Some(&(_, prev_to)) = def.last() {
                // le préambule des pourvois joints s'arrête à la première
                // frontière d'observations (« Sur le rapport », « invoque »,
                // « Vu la communication »…) : un pivot au-delà est de la
                // prose (désistement rappelé…), pas un pourvoi joint —
                // « défendeurs à la cassation » entre deux pourvois joints
                // n'en est pas une
                let blocked = self.toks.iter().any(|t| {
                    t.kind == Mk::DefEnd && t.s >= prev_to && t.s < p.s && {
                        let surf = self.text_slice(t.s, t.e).to_lowercase();
                        !surf.starts_with("défende") && !surf.starts_with("defende")
                    }
                });
                if blocked {
                    break;
                }
            }
            let limit = pivots.get(k + 1).map(|n| n.s).unwrap_or(end);
            let (zone, opp_from) = if p.kind == Mk::PivotNew {
                // demandeurs AVANT le pivot : depuis la fin du segment
                // défendeurs du pourvoi précédent
                let start = if k == 0 {
                    0
                } else {
                    def.last().map(|&(_, to)| to).unwrap_or(0)
                };
                ((start, p.s), p.e)
            } else {
                // gabarit ancien : demandeurs APRÈS le pivot, jusqu'à la
                // frontière structurelle suivante
                let seg_end = self
                    .find_tok(&[Mk::Contre], p.e, limit)
                    .or_else(|| self.find_tok(&[Mk::Opposant, Mk::DefEnd], p.e, limit))
                    .map(|t| t.s)
                    .unwrap_or(limit);
                ((p.e, seg_end), seg_end)
            };
            let def_from = self
                .find_tok(&[Mk::Opposant], opp_from, limit)
                .map(|t| t.e)
                .unwrap_or(opp_from);
            let def_to = self
                .find_tok(&[Mk::DefEnd], def_from, limit)
                .map(|t| t.s)
                .unwrap_or(limit);
            app.push(zone);
            def.push((def_from, def_to));
        }
        Some(CcSegs { app, def })
    }

    /// Gabarits CC : (demandeurs, défendeurs) depuis le préambule du pourvoi.
    pub fn cc_companies(&self) -> (Vec<String>, Vec<String>) {
        let Some(segs) = self.cc_segments() else {
            return (Vec::new(), Vec::new());
        };
        let end = self.motifs_start();
        let mut applicants: Vec<String> = Vec::new();
        for &(from, to) in &segs.app {
            for name in self.harvest(from, to, to) {
                if !applicants.iter().any(|o| same_words(o, &name)) {
                    applicants.push(name);
                }
            }
        }
        let mut defendants: Vec<String> = Vec::new();
        for &(from, to) in &segs.def {
            for name in self.harvest(from, to, to) {
                if !defendants.iter().any(|o| same_words(o, &name)) {
                    defendants.push(name);
                }
            }
        }
        // demandeur nommé des deux côtés : le pivot (signal explicite) prime
        // la fenêtre défendeurs — jonction où le greffe re-liste les parties,
        // ou re-mention du demandeur dans la désignation du défendeur
        defendants.retain(|d| !applicants.iter().any(|a| same_words(a, d)));
        if defendants.is_empty() {
            // filet : « avocat de <partie> » des observations, moins les
            // demandeurs déjà connus (comparaison pliée + variantes de
            // graphie — le greffe accentue une occurrence et pas l'autre,
            // et abrège les raisons sociales)
            let apps_f: Vec<String> = applicants.iter().map(|a| fold_stable(a)).collect();
            for t in self.toks.iter().filter(|t| t.kind == Mk::AvocatDe) {
                if t.s >= end {
                    break;
                }
                let w_end = (t.e + 140).min(end);
                for c in self.harvest(t.e, w_end, w_end) {
                    let cf = fold_stable(&c);
                    if apps_f.iter().any(|a| a.contains(cf.as_str()))
                        || side_match(&cf, &apps_f, &[])
                    {
                        continue;
                    }
                    if !defendants.contains(&c) {
                        defendants.push(c);
                    }
                }
            }
        }
        (applicants, defendants)
    }

    /// Segments étiquetés du gabarit blocs : (étiquette, from, to) pour chaque
    /// bloc APPELANT/INTIMÉ de l'en-tête (tous, pas seulement les premiers),
    /// chacun borné à l'en-tête/stop voisin.
    ///
    /// Deux mises en page de greffe coexistent :
    /// - PRÉFIXE : « APPELANTE : SAS X … INTIMÉE : SA Y » — les parties
    ///   suivent leur étiquette ;
    /// - SUFFIXE : « M. X, représenté par Me A APPELANT ⁂ SELARL Y …
    ///   INTIMÉES » — les parties précèdent leur étiquette. Signature : une
    ///   intro de conseil (« représenté par », « avocat au barreau ») apparaît
    ///   AVANT la première étiquette.
    fn block_segments(&self) -> Vec<(Mk, usize, usize)> {
        let blocks: Vec<&PTok> = self
            .toks
            .iter()
            .filter(|t| {
                matches!(
                    t.kind,
                    Mk::BlockApp
                        | Mk::BlockDef
                        | Mk::BlockOther
                        | Mk::Stop
                        | Mk::Motifs
                        | Mk::Dispositif
                )
            })
            .collect();
        let first_label = blocks
            .iter()
            .find(|b| matches!(b.kind, Mk::BlockApp | Mk::BlockDef | Mk::BlockOther));
        // Layout suffixe (« Monsieur X … représenté par Me A ⏎ APPELANT ») :
        // l'intro de conseil COLLE à l'étiquette qu'elle précède. Fenêtre en
        // FIN de token — l'en-tête de notification de greffe (« Grosse
        // délivrée le : à : Me A, avocat au barreau… ») porte des intros de
        // conseil à 300+ chars de la première étiquette d'un layout préfixe.
        // « Assistée de Marion COBOS, Greffier. ⏎ DEMANDERESSE » : l'intro
        // qui présente le personnel judiciaire (juge assisté du greffier)
        // n'est pas un conseil de partie — elle ne vote pas pour le suffixe.
        let suffix_layout = first_label.is_some_and(|f| {
            self.toks.iter().any(|t| {
                t.kind == Mk::CounselIntro && t.e <= f.s && t.e + 150 > f.s && {
                    let we = (t.e + 80).min(f.s);
                    !self.folded_slice(t.e, we).contains("greffier")
                }
            })
        });
        let mut segs: Vec<(Mk, usize, usize)> = Vec::new();
        for (i, b) in blocks.iter().enumerate() {
            if !matches!(b.kind, Mk::BlockApp | Mk::BlockDef | Mk::BlockOther) {
                continue;
            }
            let (seg_start, seg_end) = if suffix_layout {
                // les parties de cette étiquette vivent ENTRE l'étiquette
                // précédente et celle-ci (première étiquette : depuis le
                // début du document — c'est l'en-tête)
                let prev_end = blocks[..i].last().map(|p| p.e).unwrap_or(0);
                (prev_end, b.s)
            } else {
                // jusqu'à l'étiquette/stop/zone suivante
                let next = blocks.get(i + 1).map(|n| n.s).unwrap_or(self.len());
                (b.e, next)
            };
            segs.push((b.kind, seg_start, seg_end));
        }
        segs
    }

    /// Gabarit blocs CA/TJ/TCOM : parties de tous les segments étiquetés.
    pub fn block_companies(&self) -> (Vec<String>, Vec<String>) {
        let mut applicants: Vec<String> = Vec::new();
        let mut defendants: Vec<String> = Vec::new();
        for (kind, from, to) in self.block_segments() {
            let side = match kind {
                Mk::BlockApp => &mut applicants,
                Mk::BlockDef => &mut defendants,
                _ => continue,
            };
            for name in self.harvest(from, to, to) {
                if !side.contains(&name) {
                    side.push(name);
                }
            }
        }
        (applicants, defendants)
    }

    /// Intervenants (ontologie 0180, rôle intervenant — clé gold
    /// `intervenors`) : segments étiquetés du greffe (« PARTIE
    /// INTERVENANTE : », « INTERVENANT(E)(S) »), apposition parenthésée
    /// (« (Intervenant forcé) »), mémoires en intervention (admin).
    pub fn intervenors(&self) -> Vec<String> {
        self.party_registry()
            .view(None, crate::registry::Quality::Intervenor)
    }

    fn intervenors_uncached(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        // Queue de récolte : connecteurs traînants (« CPAM DE CHARENTE DE »)
        // et placeholders d'adresse empilés (« X [Adresse 5] [Adresse 5] » —
        // un placeholder APRÈS un autre groupe fermé est du bloc adresse ; un
        // placeholder seul peut être le nom même : « RÉSIDENCE [Adresse 11] »).
        fn trim_tail(mut v: &str) -> &str {
            loop {
                let t = v.trim_end();
                if let Some(last) = t.rsplit(' ').next() {
                    if matches!(
                        last.to_lowercase().as_str(),
                        "de" | "du" | "des" | "et" | "la" | "le" | "les"
                    ) {
                        v = &t[..t.len() - last.len()];
                        continue;
                    }
                    if t.ends_with(']') {
                        if let Some(i) = t.rfind('[') {
                            let prev = t[..i].trim_end();
                            if prev.ends_with(']') {
                                v = prev;
                                continue;
                            }
                        }
                    }
                }
                return t;
            }
        }
        // dédup par contenance pliée (« Organisme [3] » ↔ « organisme [3] »)
        fn push(v: String, out: &mut Vec<String>) {
            let v = trim_tail(&v).to_string();
            if v.is_empty() {
                return;
            }
            let vu = v.to_uppercase();
            if !out.iter().any(|o| {
                let ou = o.to_uppercase();
                ou.starts_with(&vu) || ou.ends_with(&vu)
            }) {
                out.push(v);
            }
        }
        let hdr = self.motifs_start();
        for (kind, from, to) in self.block_segments() {
            if kind != Mk::BlockOther {
                continue;
            }
            let Some(label) = self
                .toks
                .iter()
                .find(|t| t.kind == Mk::BlockOther && (t.e == from || t.s == to))
            else {
                continue;
            };
            let lf = self.folded_slice(label.s, label.e);
            // Étiquette-mot : casse de greffe stricte (tout en capitales) — un
            // titre de section des motifs (« …des différents intervenants : »)
            // passe la règle du deux-points mais pas celle-ci. « autres
            // parties » borne les segments voisins sans être récoltée.
            let wordy = lf.starts_with("intervenant") || lf.starts_with("partie");
            if (wordy && !is_upper_span(&self.norm.chars, label.s, label.e))
                || lf == "autres parties"
                || label.s >= hdr
            {
                continue;
            }
            // Bloc de greffe compact : bornage aux motifs, à l'audience
            // (« Débats à l'audience… » clôt la zone parties) + garde-fou de
            // taille (sans Stop aval, le segment filerait dans la prose).
            let (from, to) = if label.s == to {
                (from.max(to.saturating_sub(900)), to)
            } else {
                let audience = self
                    .toks
                    .iter()
                    .find(|a| a.kind == Mk::Audience && a.s >= from)
                    .map(|a| a.s)
                    .unwrap_or(usize::MAX);
                (from, to.min(from + 900).min(hdr.max(from)).min(audience))
            };
            // Entrée TOUT-CAPS sans tête de forme collée à l'étiquette
            // (« INTERVENANT VOLONTAIRE ⏎ ALLIANZ IARD, [Adresse 1] ») : la
            // récolte ne voit que les têtes (Form/Societe/Inst*) — on prend le
            // nom de tête du bloc quand son premier mot est en capitales,
            // après les qualificatifs d'étiquette, puces et articles.
            if label.e == from {
                let mut ns = self.skip_spaces(from, to);
                loop {
                    while ns < to && !self.norm.chars[ns].is_alphanumeric() {
                        ns += 1;
                    }
                    let we = (ns..to)
                        .find(|&i| !self.norm.chars[i].is_alphabetic())
                        .unwrap_or(to);
                    // reste de pluriel de greffe « INTERVENANTE(S) » : la
                    // lettre seule entre parenthèses n'est pas une tête
                    if we == ns + 1
                        && ns > 0
                        && self.norm.chars[ns - 1] == '('
                        && self.norm.chars.get(we).copied() == Some(')')
                    {
                        ns = we + 1;
                        continue;
                    }
                    let w: String = self.norm.chars[ns..we]
                        .iter()
                        .collect::<String>()
                        .to_lowercase();
                    if matches!(
                        w.as_str(),
                        "volontaire"
                            | "volontaires"
                            | "forcé"
                            | "forcée"
                            | "forcés"
                            | "forcées"
                            | "la"
                            | "le"
                            | "les"
                            | "l"
                            | "société"
                            | "societe"
                    ) && we > ns
                    {
                        ns = we;
                        continue;
                    }
                    break;
                }
                let we = (ns..to)
                    .find(|&i| !self.norm.chars[i].is_alphanumeric())
                    .unwrap_or(to);
                if we > ns + 1
                    && is_upper_span(&self.norm.chars, ns, we)
                    && self.norm.chars[ns..we].iter().any(|c| c.is_alphabetic())
                {
                    let ne = self.extend_name(ns, to);
                    let after = self.skip_spaces(ne, self.len());
                    let colon = self.norm.chars.get(after).copied() == Some(':');
                    if !colon {
                        if let Some(name) = self.clean(ns, ne) {
                            push(name, &mut out);
                        }
                    }
                }
                // Entrées empilées du greffe (« FIVA, demeurant [Adresse 6]
                // ⏎ non comparant ⏎ DRJSCS, demeurant [Adresse 1] ») : un nom
                // TOUT-CAPS sans tête de forme par entrée, signé par
                // « , demeurant » — invisible de la récolte par têtes.
                let (bs, be) = (self.norm.char2byte[from], self.norm.char2byte[to]);
                for (off, _) in self.norm.folded[bs..be].match_indices(", demeurant") {
                    let comma = self.norm.byte2char[bs + off];
                    let mut e = comma;
                    while e > from && self.norm.chars[e - 1] == ' ' {
                        e -= 1;
                    }
                    // marche arrière mot-à-mot : seuls des mots TOUT-CAPS
                    // s'agrègent (« non comparant  DRJSCS » s'arrête à DRJSCS
                    // — les sauts de ligne sont des espaces dans norm)
                    let mut s = e;
                    loop {
                        let mut ws = s;
                        while ws > from && self.norm.chars[ws - 1] == ' ' {
                            ws -= 1;
                        }
                        let wend = ws;
                        while ws > from
                            && (self.norm.chars[ws - 1].is_alphanumeric()
                                || matches!(self.norm.chars[ws - 1], '-' | '&' | '\''))
                        {
                            ws -= 1;
                        }
                        if wend == ws || !is_upper_span(&self.norm.chars, ws, wend) {
                            break;
                        }
                        s = ws;
                    }
                    let mut s = self.skip_spaces(s, e);
                    // article de tête (« LA VILLE DE PARIS ») : même rognage
                    // que la tête de bloc
                    loop {
                        let we = (s..e)
                            .find(|&i| !self.norm.chars[i].is_alphabetic())
                            .unwrap_or(e);
                        let w: String = self.norm.chars[s..we]
                            .iter()
                            .collect::<String>()
                            .to_lowercase();
                        if matches!(w.as_str(), "la" | "le" | "les" | "l") && we < e {
                            s = self.skip_spaces(we, e);
                        } else {
                            break;
                        }
                    }
                    if e > s + 1
                        && is_upper_span(&self.norm.chars, s, e)
                        && self.norm.chars[s..e].iter().any(|c| c.is_alphabetic())
                    {
                        if let Some(name) = self.clean(s, e) {
                            push(name, &mut out);
                        }
                    }
                }
            }
            for name in self.harvest_in(from, to, to, true) {
                push(name, &mut out);
            }
        }
        // apposition : l'entité précède la parenthèse
        for t in self.toks.iter().filter(|t| t.kind == Mk::BlockOther) {
            let before = (0..t.s)
                .rev()
                .map(|i| self.norm.chars[i])
                .find(|c| *c != ' ');
            if before != Some('(') {
                continue;
            }
            let from = t.s.saturating_sub(160);
            if let Some(name) = self.harvest_in(from, t.s, t.s, true).pop() {
                push(name, &mut out);
            }
        }
        // mémoire en intervention : les entités de la fenêtre aval (« présenté
        // pour la fédération X et pour le club Y »)
        for t in self.toks.iter().filter(|t| t.kind == Mk::IntervIntro) {
            // ancre entre guillemets (« requête intitulée " mémoire en
            // intervention volontaire " ») : un intitulé cité — souvent
            // requalifié par le juge —, pas une intervention reçue
            let before = (0..t.s)
                .rev()
                .map(|i| self.norm.chars[i])
                .find(|c| *c != ' ');
            if matches!(before, Some('"' | '«' | '“')) {
                continue;
            }
            let to = (t.e + 220).min(self.len());
            for name in self.harvest_in(t.e, to, to, true) {
                push(name, &mut out);
            }
        }
        // Re-casse d'élision : « L'organisme [3] » livre « organisme [3] »,
        // la majuscule mangée par l'article. Si TOUTES les occurrences
        // minuscules du document sont élidées (précédées d'une apostrophe)
        // et que la chaîne existe avec initiale majuscule (liste de
        // notification du greffe), c'est elle le nom — verbatim ancré,
        // jamais de casse fabriquée.
        let mut text: Option<String> = None;
        for v in &mut out {
            let Some(first) = v.chars().next().filter(|c| c.is_lowercase()) else {
                continue;
            };
            let cap: String = first.to_uppercase().chain(v.chars().skip(1)).collect();
            let text = text.get_or_insert_with(|| self.norm.chars.iter().collect());
            let all_elided = text.match_indices(v.as_str()).all(|(i, _)| {
                text[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c == '\'' || c == '\u{2019}')
            });
            if all_elided && text.contains(&cap) {
                *v = cap;
            }
        }
        out
    }

    /// Gabarit blocs : les conseils de chaque segment vont au côté de son
    /// étiquette (le conseil d'une partie est nommé dans son bloc).
    fn block_counsel(&self, out: &mut CounselOut) {
        for (kind, from, to) in self.block_segments() {
            let (names, firms) = match kind {
                Mk::BlockApp => (&mut out.applicant_names, &mut out.applicant_firms),
                Mk::BlockDef => (&mut out.defendant_names, &mut out.defendant_firms),
                _ => continue,
            };
            self.counsel_in(from, to, names, firms);
        }
    }

    /// Gabarit admin : l'en-tête se segmente aux ouvertures de requête
    /// (AdminReq) et de mémoire (MemIntro/DefIntro). Un segment est en
    /// DÉFENSE si son ouverture l'affiche (« mémoire en défense ») ou s'il
    /// « conclut au rejet » (marqueur rétroactif — le conseil se nomme AVANT
    /// le verbe : « X, représenté par Me Y, conclut au rejet ») ; sinon les
    /// conseils vont au requérant (requêtes, répliques).
    ///
    /// Une entrée suivie de « avocat de <partie> » (chaîne CE « La parole
    /// ayant été donnée… à la SCP X, avocat de <requérant> et à la SARL Y,
    /// avocat de <défendeur> ») se rattache à sa PARTIE, pas au segment :
    /// requérant si la partie est un requérant connu, défendeur sinon ; sans
    /// requérant connu (personnes physiques), ordre de greffe — première
    /// entrée « avocat de » = requérant.
    fn admin_counsel(&self, out: &mut CounselOut) {
        let end = self.motifs_start();
        let bounds: Vec<(usize, usize, Mk)> = self
            .toks
            .iter()
            .filter(|t| t.s < end && matches!(t.kind, Mk::AdminReq | Mk::MemIntro | Mk::DefIntro))
            .map(|t| (t.s, t.e, t.kind))
            .collect();
        let mut segs: Vec<(usize, usize, bool)> = Vec::new();
        let mut cursor = 0usize;
        let mut opener: Option<Mk> = None;
        for &(s, e, kind) in &bounds {
            if s > cursor {
                segs.push((cursor, s, opener == Some(Mk::DefIntro)));
            }
            cursor = e;
            opener = Some(kind);
        }
        segs.push((cursor, end, opener == Some(Mk::DefIntro)));
        let seg_def = |pos: usize| {
            segs.iter()
                .find(|(from, to, _)| pos >= *from && pos < *to)
                .map(|&(from, to, open_def)| {
                    open_def
                        || self
                            .find_tok(&[Mk::DefIntro, Mk::DefConclu], from, to)
                            .is_some()
                })
                .unwrap_or(false)
        };
        let apps_f: Vec<String> = self
            .admin_companies()
            .iter()
            .map(|a| fold_stable(a))
            .collect();
        let ctoks: Vec<PTok> = self
            .toks
            .iter()
            .filter(|t| t.s < end && matches!(t.kind, Mk::Me | Mk::LawStruct | Mk::Form))
            .cloned()
            .collect();
        let mut first_avde_seen = false;
        for (i, t) in ctoks.iter().enumerate() {
            let entry = match t.kind {
                Mk::Me => self.person_after_me(t.e, end).map(|(v, ne)| (v, ne, false)),
                _ => self.firm_entry(t, end).map(|(v, ne)| (v, ne, true)),
            };
            let Some((v, ne, is_firm)) = entry else {
                continue;
            };
            let win_end = ctoks.get(i + 1).map(|n| n.s).unwrap_or(end);
            let avde = self
                .toks
                .iter()
                .find(|a| a.kind == Mk::AvocatDe && a.s >= ne && a.s < win_end);
            let is_def = match avde {
                Some(a) => {
                    // petit span positionné par le token
                    let party = fold_stable(&self.text_slice(a.e, win_end.min(a.e + 120)));
                    let d = if side_match(&party, &apps_f, &[]) {
                        false
                    } else if apps_f.is_empty() {
                        first_avde_seen
                    } else {
                        true
                    };
                    first_avde_seen = true;
                    d
                }
                None => seg_def(t.s),
            };
            let (names, firms) = if is_def {
                (&mut out.defendant_names, &mut out.defendant_firms)
            } else {
                (&mut out.applicant_names, &mut out.applicant_firms)
            };
            let list = if is_firm { firms } else { names };
            if !list.contains(&v) {
                list.push(v);
            }
        }
    }

    /// Gabarit CC : conseils des segments demandeurs/défendeurs, puis chaîne
    /// des observations (« les observations de la SCP A, avocat de <partie>,
    /// de la SCP B… ») — le côté suit la partie citée après « avocat de » ;
    /// sans rattachement, la première entrée défend le demandeur (ordre de
    /// greffe, spec legacy conservée).
    fn cc_counsel(&self, out: &mut CounselOut) {
        let Some(segs) = self.cc_segments() else {
            return;
        };
        let end = self.motifs_start();
        for &(from, to) in &segs.app {
            self.counsel_in(from, to, &mut out.applicant_names, &mut out.applicant_firms);
        }
        for &(from, to) in &segs.def {
            self.counsel_in(from, to, &mut out.defendant_names, &mut out.defendant_firms);
        }
        // région observations : après le dernier segment du préambule
        let obs_from = segs
            .app
            .iter()
            .chain(segs.def.iter())
            .map(|&(_, to)| to)
            .max()
            .unwrap_or(0);
        let (apps, defs) = self.cc_companies();
        let apps_f: Vec<String> = apps.iter().map(|a| fold_stable(a)).collect();
        let defs_f: Vec<String> = defs.iter().map(|d| fold_stable(d)).collect();
        let mut starts: Vec<usize> = Vec::new();
        let mut entries: Vec<(usize, bool, String)> = Vec::new(); // (fin, cabinet?, valeur)
        for t in &self.toks {
            if t.s < obs_from || t.s >= end {
                continue;
            }
            let entry = match t.kind {
                Mk::Me => self.person_after_me(t.e, end).map(|(v, ne)| (ne, false, v)),
                Mk::LawStruct | Mk::Form => self.firm_entry(t, end).map(|(v, ne)| (ne, true, v)),
                _ => None,
            };
            if let Some(en) = entry {
                starts.push(t.s);
                entries.push(en);
            }
        }
        for (i, (ne, is_firm, v)) in entries.iter().enumerate() {
            let win_end = starts.get(i + 1).copied().unwrap_or(end);
            // partie rattachée : « avocat de <partie> » juste après l'entrée
            let side_def = self
                .toks
                .iter()
                .find(|a| a.kind == Mk::AvocatDe && a.s >= *ne && a.s < win_end)
                .and_then(|a| {
                    // petit span positionné par le token : la partie se nomme
                    // juste après « avocat de », pas en fin de préambule
                    let party = fold_stable(&self.text_slice(a.e, win_end.min(a.e + 120)));
                    if side_match(&party, &apps_f, &defs_f) {
                        Some(false)
                    } else if side_match(&party, &defs_f, &apps_f) {
                        Some(true)
                    } else {
                        None
                    }
                });
            let (names, firms) = if side_def.unwrap_or(i > 0) {
                (&mut out.defendant_names, &mut out.defendant_firms)
            } else {
                (&mut out.applicant_names, &mut out.applicant_firms)
            };
            let list = if *is_firm { firms } else { names };
            if !list.contains(v) {
                list.push(v.clone());
            }
        }
    }

    /// Cabinets des « Moyen(s) produit(s) par <cabinet> » annexés à l'arrêt
    /// CC : filet demandeur — un seul cabinet distinct exigé (plusieurs =
    /// pourvois croisés, attribution ambiguë). Les moyens incidents portent
    /// une incise (« Moyens produits, au pourvoi incident, par… ») qui les
    /// écarte du marqueur.
    fn moyen_par_firms(&self) -> Vec<String> {
        let mut firms: Vec<String> = Vec::new();
        for t in self.toks.iter().filter(|t| t.kind == Mk::MoyenPar) {
            let Some(ft) = self
                .toks
                .iter()
                .find(|f| f.kind == Mk::LawStruct && f.s >= t.e && f.s < t.e + 12)
            else {
                continue;
            };
            if let Some((v, _)) = self.firm_after_struct(ft, self.len()) {
                if !firms.contains(&v) {
                    firms.push(v);
                }
            }
        }
        if firms.len() == 1 {
            firms
        } else {
            Vec::new()
        }
    }

    /// Récolte les conseils d'un segment `[from..to)` : personnes après
    /// « Me », cabinets via [`Self::firm_entry`].
    fn counsel_in(&self, from: usize, to: usize, names: &mut Vec<String>, firms: &mut Vec<String>) {
        let to = to.min(self.len());
        for t in &self.toks {
            if t.s < from || t.s >= to {
                continue;
            }
            match t.kind {
                Mk::Me => {
                    if let Some((p, _)) = self.person_after_me(t.e, to) {
                        if !names.contains(&p) {
                            names.push(p);
                        }
                    }
                }
                Mk::LawStruct | Mk::Form => {
                    if let Some((f, _)) = self.firm_entry(t, to) {
                        if !firms.contains(&f) {
                            firms.push(f);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Tranche cabinet si le token désigne un CONSEIL :
    /// - structure d'avocats (SCP, SELARL, cabinet…) en contexte conseil ou
    ///   suivie d'une apposition « , avocat… » — sinon partie ès qualités ;
    /// - forme commerciale (SARL, SAS…) uniquement sur intro de conseil
    ///   explicite sans marqueur de bloc interposé (« représentée par la
    ///   SARL Le Prado - Gilbert ») ou apposition — sinon c'est une partie.
    fn firm_entry(&self, t: &PTok, to: usize) -> Option<(String, usize)> {
        let (f, ne) = self.firm_after_struct(t, to)?;
        let ok = match t.kind {
            Mk::LawStruct => self.counsel_ctx_before(t.s) || self.followed_by_counsel(ne),
            Mk::Form => {
                let intro_ctx = self.toks.iter().any(|c| {
                    c.kind == Mk::CounselIntro
                        && c.e <= t.s
                        && c.e + 60 > t.s
                        && !self.toks.iter().any(|b| {
                            b.s >= c.e
                                && b.s < t.s
                                && matches!(
                                    b.kind,
                                    Mk::BlockApp | Mk::BlockDef | Mk::BlockOther | Mk::Stop
                                )
                        })
                });
                intro_ctx || self.followed_by_counsel(ne)
            }
            _ => false,
        };
        ok.then_some((f, ne))
    }

    /// Nom de personne après « Me »/« Maître » : tranche verbatim jusqu'à la
    /// ponctuation ou au prochain token fermant. `None` = « Me » de prose.
    ///
    /// Un nom de personne = mots Capitalisés (+ particules de/du/van…) : la
    /// prose en bas-de-casse qui suit sans ponctuation (« Me B... à assister
    /// et représenter… ») et le groupe parenthésé (cabinet) coupent le nom.
    fn person_after_me(&self, e: usize, to: usize) -> Option<(String, usize)> {
        let ns = self.skip_spaces(e, to);
        let first = self.norm.chars.get(ns).copied().unwrap_or(' ');
        if !(first.is_uppercase() || first == '[') || self.opens_on_terminator(ns) {
            return None;
        }
        let ne = self.extend_name(ns, to);
        let mut cut = ns;
        let mut i = ns;
        while i < ne {
            let ws = i;
            while i < ne && self.norm.chars[i] != ' ' {
                i += 1;
            }
            let w: String = self.norm.chars[ws..i].iter().collect();
            let fw = fold_stable(&w);
            let particle = matches!(
                fw.as_str(),
                "de" | "du" | "des" | "la" | "le" | "les" | "von" | "van" | "el" | "ben" | "al"
            ) || ((fw.starts_with("d'") || fw.starts_with("l'"))
                && self
                    .norm
                    .chars
                    .get(ws + 2)
                    .is_some_and(|c| c.is_uppercase()));
            let starts_low = w.chars().next().is_some_and(|c| c.is_lowercase());
            // un ouvrant d'entité (« Me X de la SARL Y », « de l'ASSOCIATION
            // Z ») n'appartient jamais au nom d'une personne
            let opener_tok = self.toks.iter().any(|t| {
                t.s == ws
                    && matches!(
                        t.kind,
                        Mk::Form | Mk::InstHead | Mk::InstSigle | Mk::LawStruct | Mk::Societe
                    )
            });
            if w.starts_with('(') || opener_tok || (starts_low && !particle) {
                break;
            }
            cut = i;
            while i < ne && self.norm.chars[i] == ' ' {
                i += 1;
            }
        }
        self.clean_inner(ns, cut).map(|v| (v, cut))
    }

    /// Cabinet après une structure d'avocats (SCP, SELARL…) : les associés
    /// s'enchaînent par virgule (« SCP Célice, Texidor, Périer ») — on
    /// prolonge sur un mot Capitalisé qui n'ouvre pas de token.
    fn firm_after_struct(&self, t: &PTok, to: usize) -> Option<(String, usize)> {
        let ns = self.skip_spaces(t.e, to);
        let first = self.norm.chars.get(ns).copied().unwrap_or(' ');
        if !(first.is_uppercase() || first.is_ascii_digit() || first == '[')
            || self.opens_on_terminator(ns)
        {
            return None;
        }
        let ne = self.chain_end(self.extend_name(ns, to), to);
        let name = self.clean_inner(ns, ne)?;
        Some((format!("{} {}", self.text_slice(t.s, t.e), name), ne))
    }

    /// Un token ouvre-t-il exactement à `pos` ?
    fn tok_starting_at(&self, pos: usize) -> bool {
        self.toks.iter().any(|t| t.s == pos)
    }

    /// Un contexte conseil (« représenté par », « Me », « avocat de »)
    /// finit-il dans les 60 chars avant `pos` ?
    fn counsel_ctx_before(&self, pos: usize) -> bool {
        self.toks.iter().any(|c| {
            matches!(c.kind, Mk::CounselIntro | Mk::Me | Mk::AvocatDe)
                && c.e <= pos
                && c.e + 60 > pos
        })
    }

    // ── arbitrage de qualité (cabinet vs partie) par force du signal PAR
    // MENTION : post-pass des sorties publiques companies()/counsel() ────────

    /// Occurrences pliées de `value` dans le document (frontières de mot),
    /// blancs élastiques : la valeur sort de [`Self::text_slice`] (runs de
    /// blancs collapsés), le texte plié garde ses runs. Bornes en CHARS.
    fn folded_mentions(&self, value: &str) -> Vec<(usize, usize)> {
        let needle = fold_stable(value);
        let words: Vec<&str> = needle.split_whitespace().collect();
        let Some((first, rest)) = words.split_first() else {
            return Vec::new();
        };
        let bytes = self.norm.folded.as_bytes();
        let mut out = Vec::new();
        for (bs, _) in self.norm.folded.match_indices(first) {
            if bs > 0 && bytes[bs - 1].is_ascii_alphanumeric() {
                continue;
            }
            let mut be = bs + first.len();
            let mut ok = true;
            for w in rest {
                let mut k = be;
                while k < bytes.len() && bytes[k] == b' ' {
                    k += 1;
                }
                if k == be || !self.norm.folded[k..].starts_with(w) {
                    ok = false;
                    break;
                }
                be = k + w.len();
            }
            if !ok || (be < bytes.len() && bytes[be].is_ascii_alphanumeric()) {
                continue;
            }
            out.push((self.norm.byte2char[bs], self.norm.byte2char[be]));
        }
        out
    }

    /// L'intro de conseil (pliée) introduit-elle le CONSEIL après elle ?
    /// « représentée par X » : X est le conseil. « représentant X » (participe
    /// présent, « représentant légal » compris) : X est la PARTIE représentée.
    fn intro_precedes_counsel(f: &str) -> bool {
        !f.starts_with("representant")
            && (f.starts_with("represente")
                || f.starts_with("assiste")
                || f.starts_with("substitue")
                || f.starts_with("ayant pour avocat")
                || f.starts_with("comparant par")
                || f.starts_with("au cabinet de")
                || f.starts_with("les observations de")
                || f.starts_with("avocat(s)")
                || f.starts_with("rep/"))
    }

    /// Signal CABINET fort sur la mention `[s..e)` : apposition « , avocat »
    /// ou « , représentant <partie> » immédiate, intro de conseil collée
    /// (≤ 12 chars sans chiffre — « toque 343 » clôt le conseil précédent),
    /// « Me X de la <structure> » (≤ 40 chars sans virgule ni chiffre),
    /// « Moyen produit par <structure> ».
    fn mention_cabinet_strong(&self, s: usize, e: usize) -> bool {
        if self.followed_by_counsel(e) {
            return true;
        }
        let before = self.toks.iter().any(|t| {
            let win = match t.kind {
                Mk::CounselIntro | Mk::MoyenPar => 12,
                Mk::Me => 40,
                _ => return false,
            };
            if !(t.e <= s && t.e + win > s) {
                return false;
            }
            if t.kind == Mk::CounselIntro {
                let f = fold_stable(&self.text_slice(t.s, t.e));
                // « Représentant : la SCP X » (label de greffe, colonisé)
                // introduit le conseil ; « représentant X » de prose
                // introduit la partie ; « avocat au barreau de X (cabinet
                // Y) » n'introduit que la parenthèse cabinet.
                let ok_dir = Self::intro_precedes_counsel(&f)
                    || (f.starts_with("representant")
                        && (f.contains(':') || self.norm.chars[t.e..s].contains(&':')))
                    || (f.starts_with("avocat") && self.norm.chars[t.e..s].contains(&'('));
                if !ok_dir {
                    return false;
                }
            }
            !self.norm.chars[t.e..s]
                .iter()
                .any(|c| c.is_ascii_digit() || *c == ',' || *c == ';')
        });
        if before {
            return true;
        }
        // « <cabinet>, représentant la société X » : la mention représente —
        // sans colonisation (« REPRESENTANT(S) : Me X » = la mention est la
        // partie représentée).
        self.apposition_after(e).is_some_and(|g| {
            self.toks.iter().any(|t| {
                t.s == g && t.kind == Mk::CounselIntro && {
                    let f = fold_stable(&self.text_slice(t.s, t.e));
                    (f == "representant" || f == "representants") && !self.colon_after(t.e)
                }
            })
        })
    }

    /// Un « : » suit-il la position `e` (espaces et « (s) » enjambés) ?
    fn colon_after(&self, e: usize) -> bool {
        self.norm.chars[e..(e + 6).min(self.len())]
            .iter()
            .find(|c| !matches!(c, ' ' | '(' | ')' | 's' | 'S'))
            == Some(&':')
    }

    /// Premier contenu après la mention `[s..e)`, au-delà des espaces,
    /// virgules, astérisques et placeholders « [Adresse 11] » — position du
    /// token d'apposition, ou `None` si rien d'appariable dans les 100 chars.
    fn apposition_after(&self, e: usize) -> Option<usize> {
        let mut g = e;
        let to = (e + 100).min(self.len());
        while g < to {
            match self.norm.chars[g] {
                ' ' | ',' | '*' => g += 1,
                '[' => match self.norm.chars[g..to].iter().position(|&c| c == ']') {
                    Some(off) => g += off + 1,
                    None => return None,
                },
                _ => break,
            }
        }
        (g < to).then_some(g)
    }

    /// Signal PARTIE fort sur la mention `[s..e)` : étiquette de bloc de
    /// greffe collée avant, cible d'un « l'opposant à » / « avocat de » /
    /// « pourvoi formé par » / « représentant <partie> », sujet d'un pivot de
    /// pourvoi en aval, ou apposition de greffe après la mention (au-delà des
    /// placeholders d'adresse) : descripteur de partie (« défaillante »,
    /// « immatriculée », RCS, ès qualités en gabarit blocs…). Quand
    /// `weak_repr` : « , représentée par Me X » après la mention compte aussi
    /// (une structure d'avocats porte la même formule pour ELLE-MÊME — le
    /// signal ne vaut que pour les formes commerciales).
    fn mention_party_strong(&self, s: usize, e: usize, weak_repr: bool) -> bool {
        let label_before = self.toks.iter().any(|t| {
            matches!(t.kind, Mk::BlockApp | Mk::BlockDef)
                && t.e <= s
                && t.e + 20 > s
                // gap de ponctuation de greffe (« DEMANDEUR (S) : », « 1° »)
                && !self.norm.chars[t.e..s]
                    .iter()
                    .any(|c| c.is_alphabetic() && !matches!(c, 's' | 'S'))
        });
        if label_before {
            return true;
        }
        let target_before = self.toks.iter().any(|t| {
            if !(t.e <= s && t.e + 12 > s) {
                return false;
            }
            matches!(t.kind, Mk::Opposant | Mk::AvocatDe | Mk::PivotOld)
                || (t.kind == Mk::CounselIntro && {
                    let f = fold_stable(&self.text_slice(t.s, t.e));
                    // « représentant <partie> » de prose, jamais colonisé
                    f.starts_with("representant")
                        && !f.contains(':')
                        && !self.norm.chars[t.e..s].contains(&':')
                })
        });
        if target_before {
            return true;
        }
        let pivot_after = self.toks.iter().any(|t| {
            t.kind == Mk::PivotNew
                && t.s >= e
                && t.s < e + 60
                && !self.norm.chars[e..t.s]
                    .iter()
                    .any(|c| *c == '.' || *c == ';')
        });
        if pivot_after {
            return true;
        }
        let Some(g) = self.apposition_after(e) else {
            return false;
        };
        self.toks.iter().filter(|t| t.s == g).any(|t| {
            let f = fold_stable(&self.text_slice(t.s, t.e));
            match t.kind {
                Mk::TrimAlways => {
                    f.starts_with("defaillant")
                        || f.starts_with("non comparant")
                        || f.starts_with("ni comparant")
                        || f.starts_with("immatricul")
                        || f.starts_with("inscrit")
                        || f == "rcs"
                        || f.ends_with("siret")
                        || f.starts_with("prise en la personne")
                        || f.starts_with("pris en la personne")
                        || f.starts_with("domicili")
                        || f == "demeurant"
                        || (f.starts_with("es qualite") || f.starts_with("es-qualite"))
                            && self.gabarit() == Gabarit::Blocs
                }
                Mk::CounselIntro => {
                    weak_repr
                        && (Self::intro_precedes_counsel(&f)
                            || (f.starts_with("representant")
                                && (f.contains(':') || self.colon_after(t.e))))
                }
                _ => false,
            }
        })
    }

    /// Force du signal de qualité de `value` : (mentions cabinet-fort,
    /// mentions partie-fort) sur tout le document. `weak_repr` étend le
    /// signal partie à « représentée par » en aval de la mention.
    fn quality_evidence(&self, value: &str, weak_repr: bool) -> (u32, u32) {
        let mut cab = 0u32;
        let mut par = 0u32;
        for (s, e) in self.folded_mentions(value) {
            if self.mention_cabinet_strong(s, e) {
                cab += 1;
            }
            if self.mention_party_strong(s, e, weak_repr) {
                par += 1;
            }
        }
        (cab, par)
    }

    /// L'entité émise comme PARTIE est en réalité un cabinet : au moins une
    /// mention cabinet-fort et aucune mention partie-fort.
    fn drop_from_companies(&self, name: &str) -> bool {
        let (cab, par) = self.quality_evidence(name, true);
        cab > 0 && par == 0
    }

    /// L'entité émise comme CABINET est en réalité une partie : au moins une
    /// mention partie-fort et aucune mention cabinet-fort. « représentée
    /// par » en aval ne compte partie que hors structures d'avocats (une
    /// SELARL de conseil porte la même formule pour elle-même).
    fn drop_from_firms(&self, name: &str) -> bool {
        let law = is_law_structure(name);
        let (cab, par) = self.quality_evidence(name, !law);
        if par > 0 && cab == 0 {
            return true;
        }
        // prior de forme : une forme commerciale (SAS, SARL…) émise cabinet
        // sans AUCUNE mention cabinet-forte est une partie
        !law && cab == 0
    }
}

// ── appariement tolérant des noms de parties (côté des conseils) ────────────

/// Mots génériques du nommage des parties (pliés) : formes sociales,
/// épithètes juridiques et institutionnelles — jamais distinctifs entre deux
/// graphies d'une même entité.
const PARTY_GENERIC: &[&str] = &[
    "societe",
    "societes",
    "anonyme",
    "sarl",
    "sas",
    "sasu",
    "sci",
    "scm",
    "scp",
    "snc",
    "sccv",
    "scea",
    "gaec",
    "earl",
    "eurl",
    "gie",
    "sel",
    "selarl",
    "selas",
    "seleurl",
    "selafa",
    "selca",
    "responsabilite",
    "limitee",
    "simplifiee",
    "unipersonnelle",
    "actions",
    "capital",
    "variable",
    "exercice",
    "liberal",
    "liberale",
    "cooperative",
    "civile",
    "civiles",
    "commerciale",
    "commercial",
    "professionnelle",
    "professionnel",
    "professionnels",
    "agricole",
    "immobiliere",
    "immobilier",
    "immobilieres",
    "compagnie",
    "groupe",
    "groupement",
    "cabinet",
    "office",
    "etablissement",
    "etablissements",
    "entreprise",
    "entreprises",
    "exploitation",
    "agence",
    "association",
    "syndicat",
    "syndicale",
    "federation",
    "union",
    "fondation",
    "institut",
    "centre",
    "comite",
    "caisse",
    "caisses",
    "primaire",
    "regionale",
    "departementale",
    "nationale",
    "national",
    "generale",
    "general",
    "mutuelle",
    "mutuelles",
    "mutualite",
    "assurance",
    "assurances",
    "banque",
    "credit",
    "garantie",
    "france",
    "francais",
    "francaise",
    "francaises",
    "europeenne",
    "europeen",
    "internationale",
    "international",
    "commune",
    "ville",
    "departement",
    "region",
    "etat",
    "ministre",
    "ministere",
    "prefet",
    "prefecture",
    "directeur",
    "direction",
    "monsieur",
    "madame",
    "mademoiselle",
    "epoux",
    "epouse",
    "consorts",
    "veuve",
    "heritiers",
    "indivision",
    "les",
    "des",
    "aux",
    "representee",
    "represente",
    "adresse",
    "localite",
    "dont",
    "siege",
    "social",
    "qualite",
    "personne",
    "droit",
    "prive",
    "publique",
    "public",
];

fn is_generic(w: &str) -> bool {
    PARTY_GENERIC.contains(&w)
}

/// Mots d'appariement d'un nom plié : runs alphanumériques d'au moins 3
/// chars portant au moins une lettre (les nombres nus — années, numéros —
/// collisionnent avec la prose).
fn match_words(folded: &str) -> impl Iterator<Item = &str> {
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && w.chars().any(|c| c.is_alphabetic()))
}

fn word_in(list: &[String], w: &str) -> bool {
    list.iter().any(|n| match_words(n).any(|x| x == w))
}

/// Le texte plié `party` (fenêtre après « avocat de », ou candidat du filet
/// défendeurs) désigne-t-il l'un des noms pliés de `side`, en tolérant les
/// variantes de graphie (« la société FIDAL » ↔ « société anonyme Fiduciaire
/// juridique et fiscale de France (FIDAL) ») ? Match : containment du nom
/// entier, ou partage d'un mot distinctif — hors génériques et hors mots
/// présents aussi dans les noms de `other` (l'autre côté, ambigus).
fn side_match(party: &str, side: &[String], other: &[String]) -> bool {
    if side.iter().any(|x| party.contains(x.as_str())) {
        return true;
    }
    let pw: Vec<&str> = match_words(party).collect();
    side.iter()
        .any(|n| match_words(n).any(|w| !is_generic(w) && pw.contains(&w) && !word_in(other, w)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_stop_recasts_to_audience_in_prose() {
        // « l'affaire a été débattue » (Stop composite) avale son
        // « débattue » (Audience) au leftmost-longest ; en casse de prose le
        // recast Stop→Audience rend l'ancre — sinon la fenêtre de date
        // d'audience disparaît (formule CA standard).
        let w = scan("L'affaire a été débattue le 4 septembre 2012, en chambre du conseil")
            .audience_windows();
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("4 septembre 2012"));
    }

    #[test]
    fn prefix_closure_marker_survives_garbled_alias() {
        // Flux unifié (ADR 0160 §2) : au leftmost-longest, un alias OCR
        // tronqué en plein mot (« code de l'entree et du sejour des etr »,
        // link_aliases.tsv) gagne sur le marqueur ProcImmig qu'il préfixe.
        // Le rôle marqueur hérité par clôture de préfixe se gate aux
        // frontières du PRÉFIXE, pas du motif entier — sinon le signal
        // rétention/étrangers meurt sur les titres complets CESEDA.
        let sig = scan("Vu : - le code de l'entrée et du séjour des étrangers et du droit d'asile ; - le code de justice administrative.")
            .procedure_signals();
        assert!(sig.retention);
    }

    #[test]
    fn admin_single_request_header() {
        let text = "Vu la procédure suivante : Par une requête et un mémoire, enregistrés les 2 février 2021 et 15 juillet 2021, la SAS Gaillot Distribution, représentée par Me Bouyssou, demande à la cour : 1°) d'annuler l'arrêté du 22 décembre 2020 du maire de Saint-Priest refusant de lui délivrer un permis de construire ;";
        let (apps, defs) = scan(text).companies();
        assert_eq!(apps, vec!["SAS Gaillot Distribution".to_string()]);
        assert!(defs.is_empty());
    }

    #[test]
    fn zones_admin_header_ends_at_considerant() {
        let text = "Vu la procédure suivante : Par une requête enregistrée le 3 mars 2023, la SAS Alpha, représentée par Me X, demande à la cour : 1°) d'annuler le jugement ; Considérant ce qui suit : 1. La SAS Beta exploite un magasin. PAR CES MOTIFS, la cour DECIDE : Article 1er : la requête est rejetée.";
        let s = scan(text);
        assert!(s.motifs_start() < s.dispositif_start());
        assert!(s.dispositif_start() < s.text_len());
        // la SAS Beta (motifs) ne pollue pas les requérants
        let (apps, _) = s.companies();
        assert_eq!(apps, vec!["SAS Alpha".to_string()]);
    }

    #[test]
    fn zones_cc_modern_preamble() {
        let text = "La société Gamma, société anonyme, a formé le pourvoi n° X 21-12.345 contre l'arrêt rendu le 5 mai 2021 par la cour d'appel de Lyon, dans le litige l'opposant à la société Delta, défenderesse à la cassation. Faits et procédure 1. Selon l'arrêt attaqué, la société Epsilon a conclu un contrat.";
        let (apps, defs) = scan(text).companies();
        // Convention gold (ADR 0180 §3) : le descripteur nu est rogné.
        assert_eq!(apps, vec!["Gamma".to_string()]);
        assert_eq!(defs, vec!["Delta".to_string()]);
    }

    #[test]
    fn zones_cc_old_template() {
        let text = "Sur le pourvoi formé par la société Alpha Métal, société anonyme, dont le siège est à Paris, contre l'arrêt rendu le 2 février 1995 par la cour d'appel de Douai, au profit de la caisse primaire d'assurance maladie de Lille, défenderesse à la cassation ; Attendu que la société Beta soutient un moyen unique.";
        let (apps, defs) = scan(text).companies();
        assert_eq!(apps, vec!["Alpha Métal".to_string()]);
        assert_eq!(
            defs,
            vec!["caisse primaire d'assurance maladie de Lille".to_string()]
        );
    }

    #[test]
    fn zones_cc_jonction_multi_pourvois() {
        // deux pourvois principaux joints : demandeur de l'un re-listé
        // défendeur de l'autre = demandeur du dossier ; le vrai défendeur
        // est celui qui n'a formé aucun pourvoi
        let text = "I - Statuant sur le pourvoi n° P 18-21.405 formé par : 1°/ la société Alpha, société anonyme, 2°/ la société MMA IARD, société anonyme, contre l'arrêt rendu le 15 juin 2018 par la cour d'appel de Versailles, dans le litige les opposant : 1°/ à la société Cincinnatus assurance, 2°/ à la société BRC Investissement, défendeurs à la cassation ; II - Statuant sur le pourvoi n° E 18-23.168 formé par la société Cincinnatus assurance, contre le même arrêt rendu, dans le litige l'opposant : 1°/ à la société BRC Investissement, 2°/ à la société Alpha, 3°/ à la société MMA IARD, défendeurs à la cassation ; Faits et procédure 1. Selon l'arrêt attaqué.";
        let (apps, defs) = scan(text).companies();
        assert_eq!(
            apps,
            vec![
                "Alpha".to_string(),
                "MMA IARD".to_string(),
                "Cincinnatus assurance".to_string(),
            ]
        );
        assert_eq!(defs, vec!["BRC Investissement".to_string()]);
    }

    #[test]
    fn zones_cc_pourvoi_incident_ne_bascule_pas() {
        // le pourvoi incident ne fait pas du défendeur un demandeur
        let text = "La société Magna, société par actions simplifiée, a formé le pourvoi n° F 14-29.616 contre l'arrêt rendu le 5 novembre 2014 par la cour d'appel de Nancy, dans le litige l'opposant à la société Factum finance, défenderesse à la cassation ; La société Factum finance, défenderesse au pourvoi principal a formé un pourvoi incident contre le même arrêt ; Faits et procédure 1. Selon l'arrêt attaqué.";
        let (apps, defs) = scan(text).companies();
        assert_eq!(apps, vec!["Magna".to_string()]);
        assert_eq!(defs, vec!["Factum finance".to_string()]);
    }

    #[test]
    fn zones_block_suffix_layout() {
        let text = "COUR D'APPEL DE PARIS ARRET DU 3 MAI 2022 SAS Omega, représentée par Me Y, avocat au barreau de Paris APPELANTE SA Sigma, représentée par Me Z INTIMEE COMPOSITION DE LA COUR : M. A, président EXPOSE DU LITIGE La SARL Tau est intervenue à l'instance.";
        let (apps, defs) = scan(text).companies();
        assert_eq!(apps, vec!["SAS Omega".to_string()]);
        assert_eq!(defs, vec!["SA Sigma".to_string()]);
    }

    #[test]
    fn counsel_blocks_suffix_layout() {
        let text = "COUR D'APPEL DE PARIS ARRET DU 3 MAI 2022 SAS Omega, représentée par Me Jean VALJEAN de la SCP Fabantou Thénardier, avocat au barreau de Paris APPELANTE SA Sigma, représentée par Me Zoé COSETTE INTIMEE COMPOSITION DE LA COUR : M. A, président EXPOSE DU LITIGE La cour statue.";
        let out = scan(text).counsel();
        assert_eq!(out.applicant_names, vec!["Jean VALJEAN".to_string()]);
        assert_eq!(
            out.applicant_firms,
            vec!["SCP Fabantou Thénardier".to_string()]
        );
        assert_eq!(out.defendant_names, vec!["Zoé COSETTE".to_string()]);
        assert!(out.defendant_firms.is_empty());
    }

    #[test]
    fn counsel_cc_observations_chain() {
        let text = "La société Gamma a formé le pourvoi n° X 21-12.345 contre l'arrêt rendu le 5 mai 2021 par la cour d'appel de Lyon, dans le litige l'opposant à la société Delta, défenderesse à la cassation. Sur le rapport de Mme Vaissette, conseiller, les observations de la SCP Célice, Texidor, Périer, avocat de la société Gamma, de la SCP Boré et Salve de Bruneton, avocat de la société Delta, après débats et délibéré. Faits et procédure 1. Selon l'arrêt attaqué.";
        let out = scan(text).counsel();
        assert_eq!(
            out.applicant_firms,
            vec!["SCP Célice, Texidor, Périer".to_string()]
        );
        assert_eq!(
            out.defendant_firms,
            vec!["SCP Boré et Salve de Bruneton".to_string()]
        );
        assert!(out.applicant_names.is_empty());
        assert!(out.defendant_names.is_empty());
    }

    #[test]
    fn counsel_admin_sides() {
        let text = "Vu la procédure suivante : Par une requête enregistrée le 3 mars 2023, la SAS Alpha, représentée par Me Bouyssou, demande à la cour d'annuler l'arrêté du maire. Par un mémoire en défense, enregistré le 4 avril 2023, la commune de Machilly, représentée par Me Rotoumba, conclut au rejet de la requête. Considérant ce qui suit : 1. La requête est rejetée.";
        let out = scan(text).counsel();
        assert_eq!(out.applicant_names, vec!["Bouyssou".to_string()]);
        assert_eq!(out.defendant_names, vec!["Rotoumba".to_string()]);
        assert!(out.applicant_firms.is_empty());
        assert!(out.defendant_firms.is_empty());
    }

    #[test]
    fn counsel_cc_moyen_annexe_fallback() {
        let text = "La société Gamma a formé le pourvoi n° A 22-10.000 contre l'arrêt rendu par la cour d'appel de Paris, dans le litige l'opposant à la société Delta, défenderesse à la cassation. Sur le rapport de M. Ponsot, après délibération. PAR CES MOTIFS, la Cour : REJETTE le pourvoi. MOYEN ANNEXE au présent arrêt Moyen produit par la SCP Waquet, Farge et Hazan, avocat aux conseils, pour la société Gamma.";
        let out = scan(text).counsel();
        assert_eq!(
            out.applicant_firms,
            vec!["SCP Waquet, Farge et Hazan".to_string()]
        );
    }

    #[test]
    fn quality_arbitrage_form_after_counsel_line_is_party_not_firm() {
        // Deux parties dans le même bloc de greffe : la seconde (« SA Grdf »)
        // suit la ligne de conseil de la première (« avocat au barreau de
        // PARIS » à ≤ 60 chars) et serait récoltée comme cabinet — l'arbitrage
        // par mention la rejette (forme commerciale sans aucune mention
        // cabinet-forte, « défaillante » en apposition). La vraie structure
        // (« Représentant : la SCP ») reste un cabinet.
        let text = "COUR D'APPEL DE PARIS ARRET DU 3 MAI 2022 APPELANT : M. X Représentant : Me A, avocat au barreau de LYON INTIMEES : SA Enedis [Adresse 4] Représentant : la SCP Vrai Conseil, avocat au barreau de PARIS SA Grdf [Adresse 3] défaillante COMPOSITION DE LA COUR : M. B, président EXPOSE DU LITIGE La cour statue.";
        let s = scan(text);
        let out = s.counsel();
        assert_eq!(out.defendant_firms, vec!["SCP Vrai Conseil".to_string()]);
        let (_, defs) = s.companies();
        assert!(defs.iter().any(|d| d.starts_with("SA Enedis")));
    }

    #[test]
    fn quality_arbitrage_representant_precedes_party() {
        // « représentant l'<entité> » (participe présent) désigne la PARTIE
        // représentée — pas une intro de conseil vers l'entité : la caisse
        // reste une partie malgré l'intro à ≤ 12 chars.
        let text = "Vu la procédure suivante : Par une requête enregistrée le 3 mars 2023, la caisse locale des assurances mutuelles agricoles de Paris demande à la cour d'annuler l'arrêté. Ont été entendues les observations de Me Estene, représentant la caisse locale des assurances mutuelles agricoles de Paris. Considérant ce qui suit : 1. La requête est rejetée.";
        let (apps, _) = scan(text).companies();
        assert_eq!(
            apps,
            vec!["caisse locale des assurances mutuelles agricoles de Paris".to_string()]
        );
    }

    #[test]
    fn admin_multi_request_blocks() {
        let text = "Vu les procédures suivantes : 1° Sous le n°438686, par une requête, un mémoire complémentaire et deux mémoires en réplique, enregistrés les 14 février 2020 et 7 octobre 2021 au secrétariat du contentieux du Conseil d'Etat, la ville de Genève et la ville de Carouge demandent au Conseil d'Etat : 1°) d'annuler pour excès de pouvoir le décret du 24 décembre 2019 ; 2° Sous le n°439020, par une requête et quatre mémoires en réplique, enregistrés les 24 février 2020 et 12 décembre 2021, l'association Vivre à Machilly et la commune de Machilly demandent au Conseil d'Etat : 1°) d'annuler ce décret ;";
        let (apps, _) = scan(text).companies();
        assert_eq!(
            apps,
            vec![
                "ville de Genève".to_string(),
                "ville de Carouge".to_string(),
                "association Vivre à Machilly".to_string(),
                "commune de Machilly".to_string(),
            ]
        );
    }

    #[test]
    fn intervenors_prose_admin_et_recase_elision() {
        // IntervIntro multi-entités : « club » tête institutionnelle, la
        // fenêtre aval livre les deux intervenants (spec campagne 2026-07-09).
        let sc = scan("Par une intervention enregistrée le 25 août 2025, la fédération des chasseurs et le club international des chasseurs de bécassines, représentés par Me Bonzy, concluent au rejet.");
        assert_eq!(
            sc.intervenors(),
            vec![
                "fédération des chasseurs".to_string(),
                "club international des chasseurs de bécassines".to_string(),
            ]
        );
        // Ancre entre guillemets : intitulé cité, pas une intervention reçue.
        let sc = scan("Par une requête intitulée \" mémoire en intervention volontaire \" enregistrée le 17 juin, la société Veolia Eau a fait valoir son intérêt.");
        assert!(sc.intervenors().is_empty());
    }
}

#[cfg(test)]
mod ecole_solution_formation {
    use super::scan;

    /// École 2026-07-09 : l'irrecevabilité PRONONCÉE prime le « rejetée » nu
    /// du dispositif — même phrase de clôture (R. 222-1) ou enchaînement de
    /// conséquence ; la prescription de créance (fond) ne s'apparie pas.
    #[test]
    fn solution_irrecevabilite_prononcee() {
        let sc = scan("Dès lors, la requête de M. A est manifestement irrecevable et doit être rejetée, en application de l'article R. 222-1 du code de justice administrative.\nO R D O N N E :\nArticle 1er : La requête de M. A est rejetée.");
        assert_eq!(sc.outcome(), Some(("IRRECEVABILITE", false)));
        let sc = scan("Sa requête est manifestement irrecevable. Par suite, il y a lieu de rejeter ses conclusions.\nO R D O N N E :\nArticle 1er : La requête est rejetée.");
        assert_eq!(sc.outcome(), Some(("IRRECEVABILITE", false)));
        let sc = scan("La créance dont se prévaut le requérant étant prescrite, ses conclusions indemnitaires ne peuvent donc qu'être rejetées.\nO R D O N N E :\nArticle 1er : La requête est rejetée.");
        assert_eq!(sc.outcome(), Some(("REJET", false)));
    }

    /// Requête adressée au juge des référés (self) : signaux d'en-tête pour
    /// la formation JUGE_UNIQUE (première instance / Conseil d'État).
    #[test]
    fn jref_demande_et_conseil_header() {
        let sc = scan("Par une requête, enregistrée le 4 mai 2022, Mme A demande au juge des référés, statuant en application de l'article L. 521-3 du code de justice administrative, la suspension de la décision.\nConsidérant ce qui suit : 1. La requête est rejetée.");
        assert!(sc.procedure_signals().jref_demande);
        let sc = scan("M. B demande au juge des référés du Conseil d'Etat, statuant sur le fondement de l'article L. 521-2 du code de justice administrative, l'annulation de l'ordonnance.\nConsidérant ce qui suit : 1. La requête est rejetée.");
        assert!(sc.procedure_signals().jref_conseil);
    }
}
