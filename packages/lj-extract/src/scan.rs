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
    /// En-tête de bloc tiers (PARTIE INTERVENANTE…) : borne, ne récolte pas.
    BlockOther,
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
    ("pourvoi forme par", Mk::PivotOld), ("pourvois formes par", Mk::PivotOld),
    ("l'opposant", Mk::Opposant), ("en cassation contre", Mk::Opposant),
    ("contre l'arret", Mk::Contre), ("contre le jugement", Mk::Contre),
    ("contre l'ordonnance", Mk::Contre), ("contre la decision", Mk::Contre),
    ("contre un arret", Mk::Contre), ("contre un jugement", Mk::Contre),
    ("contre une ordonnance", Mk::Contre), ("contre deux arrets", Mk::Contre),
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
    // En-tête de greffe CA : « Rep/assistant : la SCP … » — l'intro de
    // conseil au plus près, la classification ne dépend plus d'un
    // « représentant légal » lointain en bord de fenêtre (60 chars).
    ("rep/assistant", Mk::CounselIntro), ("rep/assistants", Mk::CounselIntro),
    ("ayant pour avocat", Mk::CounselIntro), ("comparant par", Mk::CounselIntro),
    ("plaidant", Mk::CounselIntro), ("postulant", Mk::CounselIntro),
    ("substitue par", Mk::CounselIntro), ("substituee par", Mk::CounselIntro),
    ("avocat au barreau", Mk::CounselIntro), ("avocats au barreau", Mk::CounselIntro),
    ("avocat aux conseils", Mk::CounselIntro), ("avocats aux conseils", Mk::CounselIntro),
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
    ("n'admet pas le pourvoi", Mk::OutNonAdmis), ("pourvoi non admis", Mk::OutNonAdmis),
    ("non-admission", Mk::OutNonAdmis), ("non admission", Mk::OutNonAdmis),
    ("n'y a pas lieu de statuer", Mk::OutNonLieu), ("n'y a plus lieu de statuer", Mk::OutNonLieu),
    ("n'y a pas lieu a statuer", Mk::OutNonLieu), ("n'y a plus lieu a statuer", Mk::OutNonLieu),
    ("n'y avoir lieu a statuer", Mk::OutNonLieu), ("n'y avoir lieu de statuer", Mk::OutNonLieu),
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
    ("commission de recours des militaires", Mk::ProcDomFp),
    ("france travail", Mk::ProcDomFp), ("pole emploi", Mk::ProcDomFp),
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
    ("prestation de compensation du handicap", Mk::ProcDomAide),
    ("permis de construire", Mk::ProcDomUrba),
    ("plan local d'urbanisme", Mk::ProcDomUrba),
    ("code de l'urbanisme", Mk::ProcDomUrba),
    ("expropriation", Mk::ProcDomUrba), ("preemption", Mk::ProcDomUrba),
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
    ("impot sur le revenu", Mk::ProcDomFisc),
    ("impot sur les societes", Mk::ProcDomFisc),
    ("taxe sur la valeur ajoutee", Mk::ProcDomFisc),
    ("taxe fonciere", Mk::ProcDomFisc), ("taxe d'habitation", Mk::ProcDomFisc),
    ("cotisation fonciere des entreprises", Mk::ProcDomFisc),
    ("jugement d'ouverture", Mk::ProcCollective),
    ("redressement judiciaire", Mk::ProcCollective),
    ("liquidation judiciaire", Mk::ProcCollective),
    ("juge des referes de la cour", Mk::ProcRefCour),
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

/// Segments structurels du préambule CC (bornes en chars).
struct CcSegs {
    /// Demandeurs au pourvoi.
    app: (usize, usize),
    /// Défendeurs à la cassation.
    def: (usize, usize),
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
    pub refere_suspension: bool,
    pub refere_liberte: bool,
    pub refere_utiles: bool,
    pub refere_precontractuel: bool,
    pub refere_provision: bool,
    pub refere_cour: bool,
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
    DocScan { norm, toks }
}

/// Assemblage depuis le scan fusionné (`compiled::doc_extract`) — les champs
/// de [`DocScan`] restent privés au module.
pub(crate) fn docscan_from_parts(norm: Norm, toks: Vec<PTok>) -> DocScan {
    DocScan { norm, toks }
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
    /// Tranche verbatim ; les runs de blancs (mappés `' '` par [`Norm`]) se
    /// collapsent en un espace — même forme de sortie que la convention GT
    /// (espaces collapsés), les offsets du scan restent ceux du texte.
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
            if kept
                .chars()
                .last()
                .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
            {
                name = kept.to_string();
            }
        }
        if let Some(m) = re_trailing_addr().find(&name) {
            let kept = name[..m.start()].trim_end().to_string();
            if kept
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
            ) && self.indirect_role_before(t.s)
            {
                continue;
            }
            match t.kind {
                Mk::Form => {
                    let form = self.text_slice(t.s, t.e);
                    let ns = self.skip_spaces(t.e, to);
                    let first = self.norm.chars.get(ns).copied().unwrap_or(' ');
                    if !(first.is_uppercase() || first.is_ascii_digit() || first == '[')
                        || self.opens_on_terminator(ns)
                    {
                        continue;
                    }
                    let ne = self.extend_name(ns, to);
                    if self.followed_by_counsel(ne) {
                        consumed_until = ne;
                        continue;
                    }
                    if let Some(name) = self.clean(ns, ne) {
                        push(format!("{form} {name}"), &mut out);
                        consumed_until = ne;
                    }
                }
                Mk::Societe => {
                    let mut ns = self.skip_spaces(t.e, to);
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
                        push(name, &mut out);
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
                    if self.followed_by_counsel(ne) {
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
    pub fn dispositif_start(&self) -> usize {
        self.toks
            .iter()
            .find(|t| t.kind == Mk::Dispositif)
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
                    _ => true,
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
                if surf.contains("admission") {
                    false
                } else {
                    surf.contains("pourvoi") || self.span_before(t, 80).contains("pourvoi")
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
                if self.disp_has(from, Mk::OutIrrec) {
                    return Some(("IRRECEVABILITE", false));
                }
                if self.disp_has(from, Mk::OutRejette) {
                    return Some(("REJET", false));
                }
                None
            }
            Gabarit::Blocs => {
                if self.disp_has(from, Mk::OutNonLieu) {
                    return Some(("NON_LIEU_A_STATUER", false));
                }
                let end = self.dispositif_end(from);
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
                // « sauf / à l'exception de / mais seulement » dans la CLAUSE
                // (même phrase) d'un confirme/infirme = partialité — OutPartial
                // inclus : « sauf en ce qu » lui revient (leftmost-longest)
                let sauf_near = |c: &PTok| {
                    self.toks.iter().any(|s| {
                        matches!(s.kind, Mk::OutSauf | Mk::OutPartial)
                            && s.s > c.e
                            && s.s < c.e + 200
                            && !self.text_slice(c.e, s.s).contains('.')
                    })
                };
                match (&conf, &inf) {
                    (Some(_), Some(_)) => return Some(("INFIRMATION_PARTIELLE", false)),
                    _ if conf_partial => return Some(("INFIRMATION_PARTIELLE", false)),
                    (Some(c), None) if sauf_near(c) => {
                        return Some(("INFIRMATION_PARTIELLE", false))
                    }
                    (None, Some(i)) if sauf_near(i) => {
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
                if self.disp_has(from, Mk::OutNeutral) {
                    return Some(("AUTRE", false));
                }
                if self.disp_has(from, Mk::OutIrrec) {
                    return Some(("IRRECEVABILITE", false));
                }
                // condamnation substantielle (« à payer/verser…/somme de/
                // dommages » dans la clause) vs procédurale (art. 700/dépens)
                let cond = self.toks.iter().any(|t| {
                    t.s >= from && t.s < end && t.kind == Mk::OutCondamne && {
                        let a = self.span_after(t, 220);
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
                let rej = self.disp_has(from, Mk::OutRejette);
                if cond && rej {
                    return Some(("SATISFACTION_PARTIELLE", false));
                }
                if cond {
                    return Some(("SATISFACTION_TOTALE", false));
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
                // REFORMATION) ; requête rejetée → REJET, même motivée par
                // l'irrecevabilité (filtrage R. 222-1) ; SATISFACTION_* =
                // plein contentieux gagné SANS annulation (condamnation à
                // payer, décharge).
                if self.disp_has(from, Mk::OutAnnule) {
                    return Some(("ANNULATION", false));
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
                // non-lieu partiel + rejet du surplus = rejet
                if self.disp_has(from, Mk::OutNonLieu) {
                    if self.disp_has(from, Mk::OutRejette) {
                        return Some(("REJET", false));
                    }
                    return Some(("NON_LIEU_A_STATUER", false));
                }
                let end = self.dispositif_end(from);
                let cond = self.toks.iter().any(|t| {
                    t.s >= from && t.s < end && t.kind == Mk::OutCondamne && {
                        let a = self.span_after(t, 220);
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
                let rej = self.disp_has(from, Mk::OutRejette);
                if cond && rej {
                    return Some(("SATISFACTION_PARTIELLE", false));
                }
                if cond {
                    return Some(("SATISFACTION_TOTALE", false));
                }
                if rej {
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

    /// La cassation est-elle PARTIELLE ? (indice dans le dispositif, ou
    /// « casse … <partiel> » dans les 300 chars du verbe, n'importe où.)
    pub fn cassation_partial(&self) -> bool {
        let from = self.dispositif_start();
        let end = self.dispositif_end(from);
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
                    || a.contains("manifestement")
                    || a.contains("irrecevab")
                    || a.contains("tardivet")
                    || a.contains("hors delai")
                    || a.contains("sans le ministere d'avocat")
                    || a.contains("sans ministere d'avocat")
            })
                // « manifestement irrecevable » / « irrecevabilité manifeste »
                // sans citation R. 222-1 : la moitié des ordonnances de
                // filtrage motivent sans viser l'article (le gate ordonnance
                // ORTA_/ORCA_ vit dans `voie_key`). Lu sur le token OutIrrec —
                // une surface composée volerait le token au leftmost-longest.
                || (t.kind == Mk::OutIrrec
                    && (self.span_before(t, 20).trim_end().ends_with("manifestement")
                        || self.span_after(t, 12).trim_start().starts_with("manifeste")))
        });
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
        let pivot = self.find_tok(&[Mk::PivotNew, Mk::PivotOld], 0, header_end);
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
        match self.gabarit() {
            Gabarit::Cc => self.cc_companies(),
            Gabarit::Blocs => self.block_companies(),
            Gabarit::Admin => (self.admin_companies(), Vec::new()),
        }
    }

    /// Conseils (avocats + cabinets) par côté — mêmes gabarits structurels que
    /// [`Self::companies`], tranches verbatim.
    pub fn counsel(&self) -> CounselOut {
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

    /// Segments demandeurs/défendeurs du préambule CC — `None` = pas de pivot.
    fn cc_segments(&self) -> Option<CcSegs> {
        let end = self.motifs_start();
        let pivot_new = self.find_tok(&[Mk::PivotNew], 0, end).cloned();
        let pivot_old = self.find_tok(&[Mk::PivotOld], 0, end).cloned();
        // le pivot le plus PRÉCOCE gagne (un pourvoi incident en aval ne doit
        // pas voler le rôle du gabarit ancien qui ouvre le préambule)
        let pivot_new = match (&pivot_new, &pivot_old) {
            (Some(n), Some(o)) if o.s < n.s => None,
            _ => pivot_new,
        };
        let (app, opp_from) = match (&pivot_new, &pivot_old) {
            (Some(p), _) => ((0, p.s), p.e),
            (None, Some(p)) => {
                // sans « contre l'arrêt » : la frontière structurelle
                // suivante (début des défendeurs ou fin du préambule)
                let seg_end = self
                    .find_tok(&[Mk::Contre], p.e, end)
                    .or_else(|| self.find_tok(&[Mk::Opposant, Mk::DefEnd], p.e, end))
                    .map(|t| t.s)
                    .unwrap_or(end);
                ((p.e, seg_end), seg_end)
            }
            (None, None) => return None,
        };
        let def_from = self
            .find_tok(&[Mk::Opposant], opp_from, end)
            .map(|t| t.e)
            .unwrap_or(opp_from);
        let def_to = self
            .find_tok(&[Mk::DefEnd], def_from, end)
            .map(|t| t.s)
            .unwrap_or(end);
        Some(CcSegs {
            app,
            def: (def_from, def_to),
        })
    }

    /// Gabarits CC : (demandeurs, défendeurs) depuis le préambule du pourvoi.
    pub fn cc_companies(&self) -> (Vec<String>, Vec<String>) {
        let Some(segs) = self.cc_segments() else {
            return (Vec::new(), Vec::new());
        };
        let end = self.motifs_start();
        let applicants = self.harvest(segs.app.0, segs.app.1, segs.app.1);
        let mut defendants = self.harvest(segs.def.0, segs.def.1, segs.def.1);
        if defendants.is_empty() {
            // filet : « avocat de <partie> » des observations, moins les
            // demandeurs déjà connus
            let apps_up: Vec<String> = applicants.iter().map(|a| a.to_uppercase()).collect();
            for t in self.toks.iter().filter(|t| t.kind == Mk::AvocatDe) {
                if t.s >= end {
                    break;
                }
                let w_end = (t.e + 140).min(end);
                for c in self.harvest(t.e, w_end, w_end) {
                    let cu = c.to_uppercase();
                    if apps_up
                        .iter()
                        .any(|a| a.contains(cu.as_str()) || cu.contains(a.as_str()))
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
        let suffix_layout = first_label.is_some_and(|f| {
            self.toks
                .iter()
                .any(|t| t.kind == Mk::CounselIntro && t.s < f.s && t.s + 400 > f.s)
        });
        let mut segs: Vec<(Mk, usize, usize)> = Vec::new();
        for (i, b) in blocks.iter().enumerate() {
            if !matches!(b.kind, Mk::BlockApp | Mk::BlockDef) {
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
            let side = if kind == Mk::BlockApp {
                &mut applicants
            } else {
                &mut defendants
            };
            for name in self.harvest(from, to, to) {
                if !side.contains(&name) {
                    side.push(name);
                }
            }
        }
        (applicants, defendants)
    }

    /// Gabarit blocs : les conseils de chaque segment vont au côté de son
    /// étiquette (le conseil d'une partie est nommé dans son bloc).
    fn block_counsel(&self, out: &mut CounselOut) {
        for (kind, from, to) in self.block_segments() {
            let (names, firms) = if kind == Mk::BlockApp {
                (&mut out.applicant_names, &mut out.applicant_firms)
            } else {
                (&mut out.defendant_names, &mut out.defendant_firms)
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
                    let d = if apps_f.iter().any(|x| party.contains(x.as_str())) {
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
        self.counsel_in(
            segs.app.0,
            segs.app.1,
            &mut out.applicant_names,
            &mut out.applicant_firms,
        );
        self.counsel_in(
            segs.def.0,
            segs.def.1,
            &mut out.defendant_names,
            &mut out.defendant_firms,
        );
        // région observations : après la fin des défendeurs, avant les motifs
        let (apps, defs) = self.cc_companies();
        let apps_f: Vec<String> = apps.iter().map(|a| fold_stable(a)).collect();
        let defs_f: Vec<String> = defs.iter().map(|d| fold_stable(d)).collect();
        let mut starts: Vec<usize> = Vec::new();
        let mut entries: Vec<(usize, bool, String)> = Vec::new(); // (fin, cabinet?, valeur)
        for t in &self.toks {
            if t.s < segs.def.1 || t.s >= end {
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
                    if apps_f.iter().any(|x| party.contains(x.as_str())) {
                        Some(false)
                    } else if defs_f.iter().any(|x| party.contains(x.as_str())) {
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
        let mut ne = self.extend_name(ns, to);
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
}
