//! Scan du document : le vocabulaire des valeurs est ouvert, celui des MARQUEURS
//! qui structurent une décision est fermé. Le catalogue est compilé une fois en
//! automate Aho-Corasick leftmost-longest ; chaque document est scanné en une
//! passe sur texte plié **longueur-stable** (la casse d'origine reste lisible
//! pour les contrôles de légitimité et les tranches verbatim), et les composeurs
//! par champ rejouent le même flux de tokens ([`DocScan`]).
//!
//! Les zones sont bornées par tokens. Gabarits de parties auto-détectés :
//! - CC moderne : « <demandeurs> a/ont formé le/un pourvoi … l'opposant
//!   <défendeurs> … défendeurs à la cassation » ;
//! - CC ancien : « pourvoi formé par <demandeurs> contre l'arrêt … » ;
//! - CA/TJ/TCOM : blocs d'en-tête APPELANT/INTIMÉ ;
//! - admin : blocs de requête (« Par une requête…, X, …, demande »).
//!
//! Un nom d'entité est la tranche verbatim entre un marqueur ouvrant (forme
//! sociale, « société » + qualificatifs, tête institutionnelle) et le premier
//! token fermant ou ponctuation de liste ; les regex résiduelles opèrent sur de
//! petits spans positionnés par les tokens.

use regex::Regex;
use std::sync::OnceLock;

use crate::compiled::Norm;
use crate::registry::Mention;
use lj_fold::fold_chars;

// ── catalogue de marqueurs (surfaces PLIÉES : minuscules, sans accents) ─────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mk {
    /// En-tête de bloc demandeur (APPELANT, DEMANDEUR…).
    BlockApp,
    /// En-tête de bloc défendeur (INTIMÉ, DÉFENDEUR…).
    BlockDef,
    /// En-tête de bloc tiers (PARTIE INTERVENANTE…) : borne les segments.
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
    /// « Sur le rapport »…).
    DefEnd,
    /// Forme sociale de partie (SAS, S.A.R.L.…).
    Form,
    /// « société » : le nom suit, après les qualificatifs.
    Societe,
    /// Qualificatif de forme consommé sans être capturé (« anonyme », « par
    /// actions simplifiée »…).
    Qualif,
    /// Tête institutionnelle : le marqueur + son complément EST le nom
    /// (« caisse primaire d'assurance maladie (CPAM) de l'Isère »).
    InstHead,
    /// Tête institutionnelle sigle (CPAM, CAF, AGS…).
    InstSigle,
    /// Structure d'avocats (SELARL, SCP…) : jamais une partie, sauf ès
    /// qualités (mandataire, liquidateur, notaire).
    LawStruct,
    /// Intro de conseil (« représenté par », « avocat au barreau »…) :
    /// termine un nom de partie, ouvre le contexte conseil.
    CounselIntro,
    /// « avocat de » : rattache une partie au conseil.
    AvocatDe,
    /// « Me » / « Maître ».
    Me,
    /// Terminateur de nom, toutes casses (« dont », « agissant », « N° SIRET »,
    /// civilités…).
    TrimAlways,
    /// Terminateur de nom en bas-de-casse uniquement (verbes et articles de
    /// prose : « a », « est », « demande »…).
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
    /// Marqueur rétroactif de défense (« conclut au rejet ») : qualifie le
    /// mémoire qui le porte sans le borner.
    DefConclu,
    /// Clause de notification du dispositif (« sera notifié à … ») : elle
    /// ÉNUMÈRE les parties de l'instance. La copie pour information, non.
    NotifPartie,
    /// Tête d'autorité administrative (« ministre », « préfet », « recteur ») :
    /// son titre porte ses virgules, d'où une extension propre.
    AutoriteHead,
    /// « Moyen produit par <cabinet> … pour <partie> » (moyens annexés CC).
    MoyenPar,
    /// Début des motifs (« Considérant ce qui suit », « EXPOSÉ DU LITIGE »,
    /// « Faits et procédure ») : fin de l'en-tête.
    Motifs,
    /// Début du dispositif (« PAR CES MOTIFS », « DÉCIDE : », « D E C I D E »).
    Dispositif,
    /// Ancre de date d'audience (« audience », « débats », « plaidoiries »,
    /// « débattue », « appelée »).
    Audience,
    /// « joint les pourvois » (jonction CC) : ancre de la fenêtre des numéros
    /// de pourvois joints.
    JointPourvois,
    /// Ouverture de la liste de visas admin (« Vu : », « Vu les autres pièces »).
    VisaList,
    // ── issue du litige (lue dans la zone dispositif) ──
    /// Désistement constaté : prime le label du greffe.
    OutDesist,
    /// Non-admission du pourvoi (filtrage cassation) : prime le label.
    OutNonAdmis,
    /// « n'y a pas/plus lieu de statuer ».
    OutNonLieu,
    /// Demande accueillie sans verbe de condamnation : ouverture de procédure
    /// collective, sanction de gestion, mainlevée d'une mesure (JLD), ordonnance
    /// commune ou prorogation en référé.
    OutGrant,
    /// Issue neutre → AUTRE : renvoi entre juridictions, incompétence,
    /// radiation, caducité/péremption, rectification, interruption,
    /// médiation/conciliation, mesures d'instruction, procédure collective.
    OutNeutral,
    /// Jonction : neutre, testée en dernier.
    OutJonction,
    /// Irrecevabilité (dont prescription/forclusion).
    OutIrrec,
    /// « casse et annule ».
    OutCasse,
    /// Indice de partialité (« mais seulement », « sauf en ce que »,
    /// « partiellement », « statuant à nouveau de ce seul chef »…).
    OutPartial,
    /// « confirme ».
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
    /// « condamne » — substantiel si « à payer/verser », « somme de » ou
    /// « dommages » figure dans la clause, procédural si article 700 ou dépens
    /// seulement.
    OutCondamne,
    // ── signaux procéduraux (voie/office/domaine) ──
    /// « question prioritaire de constitutionnalité ».
    ProcQpc,
    /// Filtrage cassation au texte (« procédure préalable d'admission »,
    /// « l'admission est refusée », « non spécialement motivé », L822-1,
    /// R822-5).
    ProcPapc,
    /// R222-1 : filtrage des ordonnances TA/CAA.
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
    /// Contexte droit des étrangers (CESEDA, OQTF…).
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
    /// « désigné M./Mme X » : formule active de désignation en tête (« le
    /// président de la cour a désigné … pour statuer »).
    ProcMagdesForm,
    /// « juge/conseiller de la mise en état » : the judge of the mise en état.
    ProcMee,
    /// Composition à juge unique dite par le texte (« statuant en juge unique »,
    /// « siégeant seul »).
    ProcJugeUnique,
    /// « la présente ordonnance »: the decision is an order.
    ProcOrdonnance,
    /// « la formation prévue à » : before « l'article 567-1-1 », the criminal
    /// chamber's formation restreinte.
    ProcRestreinte,
    /// « siégeaient »: the closing clause names the judges who deliberated.
    ProcSiege,
    /// « juge des référés » nu.
    ProcJref,
    /// Référé judiciaire dit par le texte : assignation en référé, appel ou
    /// confirmation d'une ordonnance de référé, titre « ordonnance de référé »
    /// au bandeau.
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
    /// Domaines judiciaires par vocabulaire du corps : votes de termes comptés
    /// plein texte ([`DocScan::domain_term_votes`]), consommés par
    /// `domain::refine_with_terms`.
    ProcDomTravail,
    /// Vocabulaire travail valable dans les deux ordres (licenciement, heures
    /// supplémentaires…) : vote SOCIAL_DROIT_TRAVAIL en judiciaire,
    /// PUBLIC_DROIT_TRAVAIL en admin.
    ProcDomTravailMixte,
    /// AT/MP (accident du travail, maladie professionnelle, faute inexcusable,
    /// taux d'incapacité) : contentieux du travail dans les deux ordres.
    ProcDomSecu,
    /// Cotisations et recouvrement (URSSAF) : SOCIAL nu.
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

/// Genre d'un atome pour le lecteur de parties personnes physiques.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Atome {
    /// Trace de pseudonymisation : lettre seule, lettre suivie de points de
    /// suspension, groupe crocheté (« A », « X... », « [FE] »).
    Pseudo,
    /// Mot capitalisé en clair : prénom conservé, patronyme non pseudonymisé.
    Mot,
    /// Civilité : consommée, jamais imprimée.
    Civilite,
    /// Se traverse à l'intérieur d'un nom déjà ouvert.
    Virgule,
    /// Mention matrimoniale : elle ferme le nom, et ce qui la suit renomme la
    /// même personne au lieu d'en ouvrir une autre.
    Lien,
    /// Ferme le nom.
    Stop,
}

/// Borne du bandeau de greffe quand aucun marqueur d'ouverture ne le ferme, en
/// caractères normalisés. Le nom de chambre y tient dans les 200 premiers.
const BANDEAU_MAX: usize = 600;

/// Marqueurs qui ferment un nom de personne. Le lexique du scan porte déjà ces
/// coupes, le lecteur de personnes les emprunte au lieu de tenir sa propre liste.
const COUPE_NOM: &[Mk] = &[
    Mk::TrimAlways,
    Mk::TrimLower,
    Mk::CounselIntro,
    Mk::AvocatDe,
    Mk::Me,
    Mk::LawStruct,
    Mk::Form,
    Mk::Societe,
    Mk::InstHead,
    Mk::InstSigle,
    Mk::AutoriteHead,
    Mk::IndirectRole,
    Mk::BlockApp,
    Mk::BlockDef,
    Mk::BlockOther,
    Mk::Stop,
    Mk::Motifs,
    Mk::Dispositif,
];

/// Counsel holds the side of the party it represents and cannot be on both.
/// « Emmanuel Feuilly » for the claimant and « Feuilly » for the defendant are the
/// same lawyer, the second mention echoing the prose. The fullest spelling carries
/// the edge, hence the side; on a tie, the claimant's, whose heading names counsel
/// with its « représenté par ».
fn un_seul_cote(apps: &mut Vec<Mention>, defs: &mut Vec<Mention>) {
    if apps.is_empty() || defs.is_empty() {
        return;
    }
    let variante = |a: &str, b: &str| {
        let (x, y) = (mots_personne(a), mots_personne(b));
        let (court, long) = if x.len() <= y.len() {
            (&x, &y)
        } else {
            (&y, &x)
        };
        !court.is_empty() && (long.starts_with(&court[..]) || long.ends_with(&court[..]))
    };
    let mut i = 0;
    while i < defs.len() {
        match apps.iter().position(|a| variante(&a.value, &defs[i].value)) {
            Some(j)
                if mots_personne(&apps[j].value).len() >= mots_personne(&defs[i].value).len() =>
            {
                defs.remove(i);
            }
            Some(j) => {
                apps.remove(j);
                i += 1;
            }
            None => i += 1,
        }
    }
}

/// Mots d'identité d'une personne pseudonymisée : les marques de
/// pseudonymisation ne la portent pas (« [FE] [XL] » = « FE XL »).
fn mots_personne(v: &str) -> Vec<String> {
    fold_chars(v)
        .split(|c: char| {
            c.is_whitespace() || matches!(c, '.' | '\u{2026}' | '[' | ']' | ',' | '-' | '\'')
        })
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Mentions matrimoniales pliées : le gold coupe le nom devant elles 304 fois
/// contre 23.
#[rustfmt::skip]
const LIENS: &[&str] = &[
    "epouse", "epoux", "veuve", "veuf", "nee", "ne", "divorce", "divorcee", "dit", "dite",
    "anciennement",
];

/// Civilités pliées.
#[rustfmt::skip]
const CIVILITES: &[&str] = &[
    "m",
    "m.",
    "mm",
    "mm.",
    "mme",
    "mmes",
    "mlle",
    "mlles",
    "melle",
    "melles",
    "monsieur",
    "messieurs",
    "madame",
    "mesdames",
    "mademoiselle",
];

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
    // cartouche de notification du greffe : borne le premier segment suffixe et
    // clôt l'extension des noms qui le précèdent
    ("expedition delivree", Mk::Stop), ("grosse delivree", Mk::Stop),
    ("copie executoire", Mk::Stop),
    // frontières de zone : motifs (fin d'en-tête) et dispositif
    ("considerant ce qui suit", Mk::Motifs), ("considerant que", Mk::Motifs),
    ("considerant qu'", Mk::Motifs),
    ("expose du litige", Mk::Motifs), ("faits et procedure", Mk::Motifs),
    ("expose des faits", Mk::Motifs), ("motifs de la decision", Mk::Motifs),
    ("expose de la procedure", Mk::Motifs),
    ("par ces motifs", Mk::Dispositif), ("en consequence", Mk::Dispositif),
    ("decide", Mk::Dispositif),
    ("d e c i d e", Mk::Dispositif), ("ordonne", Mk::Dispositif),
    ("o r d o n n e", Mk::Dispositif), ("a r r e t e", Mk::Dispositif),
    ("arrete", Mk::Dispositif),
    // ancres de date d'audience
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
    // jonction : « Statuant sur le pourvoi n° X 00-00.000 formé par : »
    ("statuant sur le pourvoi", Mk::PivotOld), ("statuant sur les pourvois", Mk::PivotOld),
    ("l'opposant", Mk::Opposant), ("les opposant", Mk::Opposant),
    ("en cassation contre", Mk::Opposant),
    // gabarit ancien : « … au profit de <défendeur>, défenderesse à la cassation »
    ("au profit de", Mk::Opposant), ("au profit du", Mk::Opposant),
    ("au profit des", Mk::Opposant), ("au profit d'", Mk::Opposant),
    ("en cassation d'un arret", Mk::Contre), ("en cassation d'un jugement", Mk::Contre),
    ("contre l'arret", Mk::Contre), ("contre le jugement", Mk::Contre),
    ("contre l'ordonnance", Mk::Contre), ("contre la decision", Mk::Contre),
    ("contre un arret", Mk::Contre), ("contre un jugement", Mk::Contre),
    ("contre une ordonnance", Mk::Contre), ("contre deux arrets", Mk::Contre),
    // A numbered list of joined pourvois; a paragraph break leaves two spaces.
    ("contre : 1) l'arret", Mk::Contre), ("contre :  1) l'arret", Mk::Contre),
    ("contre : 1 ) l'arret", Mk::Contre), ("contre :  1 ) l'arret", Mk::Contre),
    ("contre: 1 ) l'arret", Mk::Contre), ("contre:  1 ) l'arret", Mk::Contre),
    ("contre : 1° l'arret", Mk::Contre), ("contre :  1° l'arret", Mk::Contre),
    ("contre  1) l'arret", Mk::Contre),
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
    // formes sociales de parties
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
    // en-tête de greffe CA : « Rep/assistant : la SCP … »
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
    // authority heads: the vocabulary of the annotated administrative defendants,
    // by decreasing weight
    ("ministre", Mk::AutoriteHead), ("ministere", Mk::AutoriteHead),
    ("prefet", Mk::AutoriteHead), ("prefete", Mk::AutoriteHead),
    ("prefecture", Mk::AutoriteHead), ("garde des sceaux", Mk::AutoriteHead),
    ("recteur", Mk::AutoriteHead), ("rectrice", Mk::AutoriteHead),
    ("maire", Mk::AutoriteHead), ("region", Mk::AutoriteHead),
    ("universite", Mk::AutoriteHead), ("directeur", Mk::AutoriteHead),
    ("directrice", Mk::AutoriteHead), ("conseil national", Mk::AutoriteHead),
    ("conseil departemental", Mk::AutoriteHead),
    ("conseil regional", Mk::AutoriteHead), ("agence nationale", Mk::AutoriteHead),
    ("agence regionale", Mk::AutoriteHead),
    ("service departemental", Mk::AutoriteHead),
    ("centre national", Mk::AutoriteHead), ("assistance publique", Mk::AutoriteHead),
    ("sera notifie", Mk::NotifPartie), ("sera notifiee", Mk::NotifPartie),
    ("seront notifies", Mk::NotifPartie), ("seront notifiees", Mk::NotifPartie),
    ("est notifie", Mk::NotifPartie), ("est notifiee", Mk::NotifPartie),
    ("moyen produit par", Mk::MoyenPar), ("moyens produits par", Mk::MoyenPar),
    ("moyen annexe produit par", Mk::MoyenPar), ("moyens annexes produits par", Mk::MoyenPar),
    // en-têtes sans « produit par » (« MOYENS ANNEXES au present arret », puis
    // « moyens produits AU POURVOI PRINCIPAL par… »)
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
    // quantifieur : jamais dans un nom propre (« la commune de Richelieu et
    // plusieurs particuliers »)
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
    // « se désiste » ferme les noms et vit aussi en OutDesist (issue)
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
    // issue du litige — zone dispositif
    ("desistement", Mk::OutDesist), ("se desiste", Mk::OutDesist),
    ("n'est pas admis", Mk::OutNonAdmis), ("ne sont pas admis", Mk::OutNonAdmis),
    ("n'est pas admise", Mk::OutNonAdmis), ("ne sont pas admises", Mk::OutNonAdmis),
    ("pourvois non admis", Mk::OutNonAdmis),
    ("n'admet pas le pourvoi", Mk::OutNonAdmis), ("pourvoi non admis", Mk::OutNonAdmis),
    ("non-admission", Mk::OutNonAdmis), ("non admission", Mk::OutNonAdmis),
    ("n'y a pas lieu de statuer", Mk::OutNonLieu), ("n'y a plus lieu de statuer", Mk::OutNonLieu),
    ("n'y a pas lieu a statuer", Mk::OutNonLieu), ("n'y a plus lieu a statuer", Mk::OutNonLieu),
    ("n'y avoir lieu a statuer", Mk::OutNonLieu), ("n'y avoir lieu de statuer", Mk::OutNonLieu),
    // QPC non renvoyée
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
    // octrois sans condamnation
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
    // annulation bornée (« annulée en tant qu'elle a statué sur… ») et cassation
    // par retranchement
    ("en tant qu'il", Mk::OutPartial), ("en tant qu'elle", Mk::OutPartial),
    ("dans cette mesure", Mk::OutPartial),
    ("dans ses seules dispositions", Mk::OutPartial),
    ("en ses dispositions", Mk::OutPartial),
    // passif fléchi (« la décision est confirmée/infirmée/annulée »)
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
    // signaux procéduraux (les articles L521-*/R222-1… sont générés au build du
    // vocabulaire)
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
    // the judges signing a CA or TJ order: a delegated magistrate instructing the
    // case, or the judge of the mise en état
    ("magistrat charge d'instruire", Mk::ProcMagdes),
    ("juge de la mise en etat", Mk::ProcMee),
    ("magistrat de la mise en etat", Mk::ProcMee),
    ("conseiller de la mise en etat", Mk::ProcMee),
    ("conseillere de la mise en etat", Mk::ProcMee),
    ("charge de la mise en etat", Mk::ProcMee),
    ("chargee de la mise en etat", Mk::ProcMee),
    ("ordonnance du juge de la mise en etat", Mk::ProcMee),
    ("juge unique", Mk::ProcJugeUnique),
    ("statuant seul", Mk::ProcJugeUnique), ("statuant seule", Mk::ProcJugeUnique),
    ("siegeant seul", Mk::ProcJugeUnique), ("siegeant seule", Mk::ProcJugeUnique),
    ("magistrat unique", Mk::ProcJugeUnique),
    ("presente ordonnance", Mk::ProcOrdonnance),
    // stops short of the article, which the citation lexer reads
    ("formation prevue a", Mk::ProcRestreinte),
    ("siegeaient", Mk::ProcSiege),
    // domaines admin par vocabulaire d'en-tête
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
    // domaines judiciaires par vocabulaire du corps : termes canoniques de la
    // matière, jamais de noms de parties institutionnelles
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

/// Articles procéduraux (CJA) : la lettre et le numéro se déclinent en
/// variantes de surface (« L. 521-2 », « L.521-2 », « L 521-2 », « L521-2 »,
/// tiret ou tiret demi-cadratin) au build du vocabulaire.
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

/// Surfaces marqueurs (pliées) et leur type, consommées par l'automate de
/// `compiled`.
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
    // mémo : le registre de parties se construit une fois, les champs NER en sont
    // des vues
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
    /// The judge of the mise en état signs, or speaks as « Nous, … ».
    pub mee_signed: bool,
    /// « ORDONNANCE DU JUGE DE LA MISE EN ÉTAT » in the header.
    pub mee_banner: bool,
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
    /// « ARRÊT DE DÉSISTEMENT » en titre : l'instance s'interrompt, aucune voie.
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

/// Les mêmes quatre cellules, mentions ancrées : ce que les composeurs par
/// gabarit remplissent avant projection.
#[derive(Debug, Clone, Default)]
struct CounselMentions {
    applicant_names: Vec<Mention>,
    applicant_firms: Vec<Mention>,
    defendant_names: Vec<Mention>,
    defendant_firms: Vec<Mention>,
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

/// Variante à préparation fournie : le [`Norm`] est construit une fois par
/// document et partagé entre les moteurs. Le `DocScan` en prend possession.
pub fn scan_norm(norm: Norm) -> DocScan {
    let toks = crate::compiled::scan_marks(&norm);
    DocScan {
        norm,
        toks,
        registry_memo: std::cell::OnceCell::new(),
    }
}

impl DocScan {
    /// The folded text (`lj_fold`), its offsets going back through `byte2char`.
    pub fn folded(&self) -> &str {
        &self.norm.folded
    }

    pub fn chars(&self) -> &[char] {
        &self.norm.chars
    }

    pub fn byte2char(&self) -> &[usize] {
        &self.norm.byte2char
    }

    /// The line breaks of the input text, which `chars` mapped to a space.
    pub fn breaks(&self) -> &[bool] {
        &self.norm.breaks
    }

    pub fn verbatim(&self, s: usize, e: usize) -> String {
        self.text_slice(s, e)
    }

    /// Bornes (chars) du dispositif : son ouvreur, puis les moyens annexés ou la fin.
    pub fn dispositif_span(&self) -> (usize, usize) {
        let s = self.dispositif_start();
        (s, self.dispositif_end(s))
    }

    /// The outcome verbs between `s` and `e`, each opening a clause.
    pub fn clause_starts_in(&self, s: usize, e: usize) -> Vec<usize> {
        self.toks
            .iter()
            .filter(|t| t.s >= s && t.s < e)
            .filter(|t| {
                matches!(
                    t.kind,
                    Mk::OutDesist
                        | Mk::OutNonAdmis
                        | Mk::OutNonLieu
                        | Mk::OutGrant
                        | Mk::OutNeutral
                        | Mk::OutJonction
                        | Mk::OutIrrec
                        | Mk::OutCasse
                        | Mk::OutPartial
                        | Mk::OutConfirme
                        | Mk::OutInfirme
                        | Mk::OutSauf
                        | Mk::OutRejette
                        | Mk::OutAnnule
                        | Mk::OutCondamne
                )
            })
            .map(|t| t.s)
            .collect()
    }
}

/// Assemblage depuis le scan de `compiled::doc_extract` — les champs de
/// [`DocScan`] restent privés au module.
pub(crate) fn docscan_from_parts(norm: Norm, toks: Vec<PTok>) -> DocScan {
    DocScan {
        norm,
        toks,
        registry_memo: std::cell::OnceCell::new(),
    }
}

/// Gate de légitimité d'un match marqueur (frontières de mot, casse, recast
/// « débats »), appelé par le scan de `compiled` pour chaque match de motif
/// marqueur. `None` = match rejeté, le texte n'est pas consommé.
pub(crate) fn marker_token(
    chars: &[char],
    folded: &str,
    byte2char: &[usize],
    bs: usize,
    be: usize,
    kind: Mk,
) -> Option<PTok> {
    let bytes = folded.as_bytes();
    let before_ok =
        !bytes[bs].is_ascii_alphanumeric() || bs == 0 || !bytes[bs - 1].is_ascii_alphanumeric();
    let after_ok = !bytes[be - 1].is_ascii_alphanumeric()
        || be >= bytes.len()
        || !bytes[be].is_ascii_alphanumeric();
    if !before_ok || !after_ok {
        return None;
    }
    let (s, e) = (byte2char[bs], byte2char[be]);
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
        // structures d'avocats : capitale initiale suffit, les greffes les écrivent
        // aussi en casse mixte (« Selarl DNL Avocats »). « cabinet » est un mot de
        // prose : sa légitimité de conseil se joue au contexte.
        Mk::LawStruct => chars[s].is_uppercase() || &folded[bs..be] == "cabinet",
        // cartouche de notification (« Expédition délivrée à Me X ») : capitale
        // initiale suffit
        Mk::Stop
            if matches!(
                &folded[bs..be],
                "expedition delivree" | "grosse delivree" | "copie executoire"
            ) =>
        {
            chars[s].is_uppercase()
        }
        Mk::BlockApp | Mk::BlockDef | Mk::BlockOther | Mk::Stop => header_case(chars, s, e),
        // dispositif : casse d'en-tête, ou « Par ces motifs Confirme… » — capitale
        // initiale suivie d'un verbe de dispositif capitalisé
        Mk::Dispositif => {
            header_case(chars, s, e)
                || (chars[s].is_uppercase()
                    && chars[e..]
                        .iter()
                        .find(|c| **c != ' ')
                        .is_some_and(|c| c.is_uppercase()))
        }
        Mk::Motifs => chars[s].is_uppercase(),
        // prose : bas-de-casse exigé. Le pattern « a » vise le verbe avoir ; la
        // préposition « à » (même pliage, accent dans l'original) vit dans les noms
        // (« Vivre à Machilly »).
        Mk::TrimLower => is_lower_span(chars, s, e) && (&folded[bs..be] != "a" || chars[s] == 'a'),
        Mk::Me => chars[s].is_uppercase(),
        // « M. » suivi d'autres points = placeholder anonymisé « M... », un nom de
        // partie
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

/// Mêmes mots pliés, ordre libre : le greffe d'un arrêt de jonction permute
/// parfois les patronymes d'un pourvoi à l'autre (« J... F..., Y... L... » ↔
/// « Y... L..., J... F... »).
fn same_words(a: &str, b: &str) -> bool {
    let words = |s: &str| {
        let f = lj_fold::fold_chars(s);
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
    /// tout-CAPS, cherche dans le texte plié une autre occurrence des mêmes chars
    /// pliés, en frontières de mot, dont la tranche source est mieux cassée
    /// (cf. [`crate::extract::common::better_cased`]). La sortie reste une tranche
    /// verbatim du texte, jamais une réécriture.
    pub(crate) fn best_cased_twin(&self, value: &str) -> Option<String> {
        let needle = lj_fold::fold_chars(value);
        if needle.is_empty() {
            return None;
        }
        let bytes = self.norm.folded.as_bytes();
        for (bs, _) in self.norm.folded.match_indices(&needle) {
            let be = bs + needle.len();
            let boundary = (bs == 0 || !bytes[bs - 1].is_ascii_alphanumeric())
                && (be >= bytes.len() || !bytes[be].is_ascii_alphanumeric());
            if !boundary {
                continue;
            }
            let cs = self.norm.byte2char[bs];
            let ce = self.norm.byte2char[be];
            let cand: String = self.norm.chars[cs..ce].iter().collect();
            if crate::extract::common::better_cased(value, &cand) {
                return Some(cand);
            }
        }
        None
    }

    /// The source char under folded byte `off` of a folded slice opened at
    /// source char `from`.
    fn source_after(&self, from: usize, off: usize) -> usize {
        self.norm.byte2char[self.norm.char2byte[from] + off]
    }

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
        let mut tok_end = hard_end;
        for t in &self.toks {
            if t.s <= start || t.s >= tok_end {
                continue;
            }
            // les marqueurs ouvrants (Form/InstHead/InstSigle) ne ferment pas
            // l'extension : les noms institutionnels composés en contiennent en plein
            // milieu (« caisse régionale de crédit agricole mutuel… »). La séparation des
            // énumérations passe par les connecteurs (« et la/et le… », TrimLower) et la
            // virgule.
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
                    let close = self.norm.chars[i..tok_end.min(i + 42)]
                        .iter()
                        .position(|&c| c == ']');
                    match close {
                        Some(off) => i += off + 1,
                        None => break,
                    }
                }
                '(' => {
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
        // « M... » / « X... » : les points finaux d'un placeholder anonymisé font
        // partie du nom
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
        // placeholders d'adresse de greffe : coupe au premier « [Adresse » /
        // « [Localité » précédé d'une capitale ou d'un chiffre. Liés par la prose
        // (« résidence [Adresse 3] ») ou seuls porteurs, ils restent.
        let addr_at = name
            .find(" [Adresse")
            .or_else(|| name.find(" [Localité").or_else(|| name.find(" [Localite")));
        if let Some(pos) = addr_at {
            let kept = name[..pos].trim_end();
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
            // connecteur en capitales + placeholder unique : le placeholder est le nom
            // même ; un run de placeholders est un bloc adresse collé au nom
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
        re.is_match(&fold_chars(&self.text_slice(e, end)))
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

    /// Une intro de conseil qui introduit (« représentée par ») colle-t-elle
    /// (≤ 12 chars, l'article au plus) juste avant `pos` ? Un chiffre dans le gap
    /// est une ligne de greffe (« toque 343 », « vestiaire : D0289 ») qui clôt le
    /// conseil de la partie précédente. Même règle de direction que
    /// [`Self::mention_cabinet_strong`] : « avocat au barreau de <ville> » ne
    /// précède jamais un conseil, sauf parenthèse (« avocat au barreau de X
    /// (cabinet Y) »).
    fn counsel_intro_just_before(&self, pos: usize) -> bool {
        self.toks.iter().any(|c| {
            c.kind == Mk::CounselIntro && c.e <= pos && c.e + 12 > pos && {
                let f = fold_chars(&self.text_slice(c.s, c.e));
                let ok_dir = Self::intro_precedes_counsel(&f)
                    || (f.starts_with("representant")
                        && (f.contains(':') || self.norm.chars[c.e..pos].contains(&':')))
                    || (f.starts_with("avocat") && self.norm.chars[c.e..pos].contains(&'('));
                ok_dir
                    && !self.norm.chars[c.e..pos]
                        .iter()
                        .any(|ch| ch.is_ascii_digit())
            }
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
    pub fn harvest(&self, from: usize, to: usize, inst_to: usize) -> Vec<Mention> {
        self.harvest_in(from, to, inst_to, false)
    }

    /// Atome du lecteur de personnes physiques, à partir du premier caractère
    /// non blanc en `i` : la fin de la tranche lue et son genre.
    fn atome(&self, i: usize, to: usize) -> (usize, Atome) {
        let chars = &self.norm.chars;
        if chars[i] == ',' {
            return (i + 1, Atome::Virgule);
        }
        if chars[i] == '[' {
            let mut j = i + 1;
            while j < to && j < i + 6 && chars[j].is_ascii_uppercase() {
                j += 1;
            }
            if j > i + 1 && j < to && chars[j] == ']' {
                return (j + 1, Atome::Pseudo);
            }
            return (i + 1, Atome::Stop);
        }
        if !chars[i].is_alphabetic() {
            return (i + 1, Atome::Stop);
        }
        let mut j = i;
        while j < to && (chars[j].is_alphabetic() || matches!(chars[j], '\'' | '\u{2019}' | '-')) {
            j += 1;
        }
        let mut e = j;
        if j < to && chars[j] == '.' {
            e = j + 1;
            while e < to && chars[e] == '.' {
                e += 1;
            }
        } else if j < to && chars[j] == '\u{2026}' {
            e = j + 1;
        }
        if !chars[i].is_uppercase() {
            if LIENS.contains(&self.folded_slice(i, j)) {
                return (e, Atome::Lien);
            }
            return (e, Atome::Stop);
        }
        if CIVILITES.contains(&self.folded_slice(i, j))
            || CIVILITES.contains(&self.folded_slice(i, e))
        {
            return (e, Atome::Civilite);
        }
        if j == i + 1 {
            return (e, Atome::Pseudo);
        }
        (e, Atome::Mot)
    }

    /// Parties personnes physiques d'un segment `[from..to)`.
    ///
    /// Le discriminant est la trace de pseudonymisation (lettre seule, lettre
    /// suivie de points de suspension, groupe crocheté), jamais ce qui précède :
    /// le greffe imprime « M. A B », « X... Jacques » ou « [FE] [XL] » nu. Une
    /// suite d'atomes capitalisés sans aucune trace est un cabinet ou une
    /// juridiction, pas une partie.
    ///
    /// La civilité n'entre pas dans la valeur, et la mention matrimoniale la
    /// ferme (« Régine X..., épouse Y... » ⇒ « Régine X... » : le gold coupe
    /// 304 fois contre 23).
    fn persons_in(&self, from: usize, to: usize) -> Vec<Mention> {
        let to = to.min(self.len());
        let mut out: Vec<Mention> = Vec::new();
        let mut coupes: Vec<usize> = self
            .toks
            .iter()
            .filter(|t| t.s >= from && t.s < to && COUPE_NOM.contains(&t.kind))
            .map(|t| t.s)
            .collect();
        coupes.sort_unstable();
        let coupe = |pos: usize| coupes.binary_search(&pos).is_ok();
        let mut atomes: Vec<(usize, usize, Atome)> = Vec::new();
        let mut i = self.skip_spaces(from, to);
        while i < to {
            let (e, g) = self.atome(i, to);
            atomes.push((i, e, g));
            i = self.skip_spaces(e.max(i + 1), to);
        }
        let mut k = 0;
        while k < atomes.len() {
            // « Régine Y..., épouse Z... » : « Z... » renomme la personne déjà
            // lue, la sauter est ce que le gold annote
            if atomes[k].2 == Atome::Lien {
                k += 1;
                while atomes
                    .get(k)
                    .is_some_and(|a| matches!(a.2, Atome::Mot | Atome::Pseudo | Atome::Virgule))
                {
                    k += 1;
                }
                continue;
            }
            if !matches!(atomes[k].2, Atome::Mot | Atome::Pseudo) || coupe(atomes[k].0) {
                k += 1;
                continue;
            }
            let (debut, mut j, mut vu, mut fin) = (k, k, false, k);
            while j < atomes.len() {
                match atomes[j].2 {
                    Atome::Pseudo => {
                        vu = true;
                        fin = j;
                        j += 1;
                    }
                    // un prénom en clair suivi d'un pseudonyme, après un
                    // pseudonyme déjà lu, ouvre la personne suivante
                    // (« Pierre X... Danièle X... »)
                    Atome::Mot
                        if coupe(atomes[j].0)
                            || (vu && atomes.get(j + 1).is_some_and(|n| n.2 == Atome::Pseudo)) =>
                    {
                        break
                    }
                    Atome::Mot => {
                        fin = j;
                        j += 1;
                    }
                    Atome::Virgule
                        if vu
                            && atomes
                                .get(j + 1)
                                .is_some_and(|n| matches!(n.2, Atome::Mot | Atome::Pseudo)) =>
                    {
                        j += 1
                    }
                    _ => break,
                }
            }
            if vu {
                let (s, e) = (atomes[debut].0, atomes[fin].1);
                let v = self.text_slice(s, e).trim().to_string();
                if !v.is_empty() {
                    out.push(Mention::new(s, e, v));
                }
            }
            k = j.max(debut + 1);
        }
        out
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
    ) -> Vec<Mention> {
        let to = to.min(self.len());
        let mut out: Vec<Mention> = Vec::new();
        let mut consumed_until = from;
        // fusion par contenance (préfixe/suffixe, casse pliée) : on garde la plus
        // longue
        let push = |m: Mention, out: &mut Vec<Mention>| {
            let nu = m.value.to_uppercase();
            for o in out.iter_mut() {
                let ou = o.value.to_uppercase();
                if ou.starts_with(&nu) || ou.ends_with(&nu) {
                    return;
                }
                if nu.starts_with(&ou) || nu.ends_with(&ou) {
                    *o = m;
                    return;
                }
            }
            out.push(m);
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
                    // la forme sociale collée à une intro de conseil est un cabinet
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
                        push(Mention::new(t.s, ne, format!("{form} {name}")), &mut out);
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
                        // tête « société » nue (aucun qualificatif, attaque majuscule/chiffre/[) =
                        // descripteur de greffe, rognée (« la société Gondrand » ⇒ « Gondrand »).
                        // Qualifiée ou nom dont la tête fait partie (« Société Générale ») : tranche
                        // entière.
                        let bare = ns == head_end && !lower_start;
                        if bare && fold_chars(&name) != "generale" {
                            push(Mention::new(ns, ne, name), &mut out);
                        } else {
                            let head = self.text_slice(t.s, ns);
                            let v = format!("{} {name}", head.trim_end());
                            push(Mention::new(t.s, ne, v), &mut out);
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
                                    fold_chars(&name).as_str(),
                                    "etablissement public" | "etablissement francais"
                                ))
                        {
                            continue;
                        }
                        push(Mention::new(t.s, ne, name), &mut out);
                        consumed_until = ne;
                    }
                }
                Mk::LawStruct => {
                    // une SELARL/SCP est une partie quand elle n'est pas introduite comme conseil
                    // (« représenté par Me X de la SCP Y ») : mandataires, liquidateurs, notaires
                    // assignés. « cabinet » n'ouvre jamais une partie.
                    if self.counsel_ctx_before(t.s)
                        || fold_chars(&self.text_slice(t.s, t.e)) == "cabinet"
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
                    // apposition ès qualités en aval (« SELARL X, mandataire judiciaire »,
                    // « prise en la personne de Me Y, ès qualités de liquidateur de… ») : le
                    // mandataire de justice n'est jamais une partie, seul le débiteur représenté
                    // l'est
                    let win = fold_chars(&self.text_slice(ne, (ne + 120).min(to)));
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
                        push(Mention::new(t.s, ne, format!("{kind} {name}")), &mut out);
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
                    let f = fold_chars(&s);
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

    /// Début du dispositif : le premier « par ces motifs » quand il existe, sinon
    /// le premier token Dispositif (gabarit admin).
    pub fn dispositif_start(&self) -> usize {
        // ouvreur fort d'abord : « par ces motifs », les formes espacées de greffe
        // (« D É C I D E ») ou un verbe d'ouverture suivi de « : ». La surface faible
        // « en conséquence » ne sert que de repli.
        let strong = |t: &&PTok| {
            t.kind == Mk::Dispositif && {
                let surf = fold_chars(&self.text_slice(t.s, t.e));
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
            .or_else(|| self.capital_verb_line())
            .or_else(|| self.abstract_closing().map(|(s, _)| s))
            .or_else(|| self.article_one_line())
            .unwrap_or(self.len())
    }

    /// The « Article 1er : » of the last third that opens an old administrative operative
    /// part with no « décide », on the subject of a ruling or numbered up to its notification.
    fn article_one_line(&self) -> Option<usize> {
        const SUBJECTS: &[&str] = &[
            "la requete",
            "les requetes",
            "le recours",
            "les recours",
            "le pourvoi",
            "les pourvois",
            "le jugement",
            "les jugements",
            "la decision",
            "les decisions",
            "l'arret",
            "les arrets",
            "l'ordonnance",
            "les ordonnances",
            "les conclusions",
            "la demande",
            "les demandes",
            "l'intervention",
            "les interventions",
            "la question",
            "la deliberation",
            "il n'y a pas lieu",
            "il n'y a plus lieu",
            "il est donne acte",
            "il est sursis",
            "il est accorde",
        ];
        let chars = &self.norm.chars;
        let n = self.len();
        let from = n - n / 3;
        let base = self.norm.char2byte[from];
        let lines: Vec<usize> = self
            .folded_slice(from, n)
            .match_indices("article")
            .map(|(b, _)| self.norm.byte2char[base + b])
            .filter(|&i| {
                chars[i] == 'A'
                    && (0..i).rev().find(|&j| chars[j] != ' ').is_none_or(|j| {
                        matches!(chars[j], ';' | '.' | '-')
                            || self.norm.breaks[j + 1..i].contains(&true)
                    })
            })
            .collect();
        let first_article = |i: usize| {
            let rest = self.folded_slice(i + 7, (i + 80).min(n)).trim_start();
            ["1er", "ler", "premier", "1"]
                .iter()
                .find_map(|p| rest.strip_prefix(p))
                .and_then(|r| r.trim_start().strip_prefix([':', '-', '\u{2013}']))
                .map(|r| r.trim_start().to_owned())
        };
        // A text quoted in the motifs meets the operative part's own « Article 1er » first.
        let closes_on_notification = |k: usize| {
            lines[k + 1..]
                .iter()
                .take_while(|&&j| first_article(j).is_none())
                .any(|&j| {
                    let article = self.folded_slice(j, (j + 200).min(n));
                    article.contains("notifi")
                        && [
                            "presente decision",
                            "present arret",
                            "presente ordonnance",
                            "present jugement",
                        ]
                        .iter()
                        .any(|own| article.contains(own))
                })
        };
        lines
            .iter()
            .enumerate()
            .find(|&(k, &i)| {
                first_article(i).is_some_and(|r| {
                    SUBJECTS.iter().any(|s| r.starts_with(s)) || closes_on_notification(k)
                })
            })
            .map(|(_, &i)| i)
    }

    /// A line of the second half opening on a ruling verb in capitals
    /// (« REJETTE le pourvoi ; »), the dispositif of a text that announces it no
    /// other way.
    fn capital_verb_line(&self) -> Option<usize> {
        const VERBS: &[&str] = &[
            "rejette",
            "casse",
            "annule",
            "confirme",
            "infirme",
            "reforme",
            "condamne",
            "declare",
            "dit",
            "ordonne",
            "renvoie",
            "deboute",
            "constate",
            "prononce",
            "donne",
            "met",
            "remet",
            "rabat",
            "accueille",
        ];
        let chars = &self.norm.chars;
        let mut i = self.len() / 2;
        while i < self.len() {
            if i == 0 || self.norm.breaks[i - 1] {
                let s = (i..self.len()).find(|&j| chars[j] != ' ')?;
                let e = (s..self.len())
                    .find(|&j| !chars[j].is_alphabetic())
                    .unwrap_or(self.len());
                if e > s + 1
                    && is_upper_span(chars, s, e)
                    && VERBS.contains(&fold_chars(&self.text_slice(s, e)).as_str())
                {
                    return Some(s);
                }
            }
            i += 1;
        }
        None
    }

    /// The closing of an old Conseil d'État abstract, a run of clauses each opening
    /// on a noun of the ruling (« ; Rejet avec dépens. », « Annulation ; décharge ;
    /// remboursement des frais de timbre. »): its start, and the outcome its first
    /// noun names.
    fn abstract_closing(&self) -> Option<(usize, &'static str)> {
        const NOUNS: &[(&str, Option<&str>)] = &[
            ("rejet", Some("REJET")),
            ("annulation", Some("ANNULATION")),
            ("reformation", Some("REFORMATION")),
            ("non-lieu", Some("NON_LIEU_A_STATUER")),
            ("non lieu", Some("NON_LIEU_A_STATUER")),
            ("desistement", Some("DESISTEMENT")),
            ("decharge", Some("SATISFACTION_TOTALE")),
            ("reduction", Some("SATISFACTION_PARTIELLE")),
            ("sursis", Some("AUTRE")),
            ("expertise", Some("AUTRE")),
            ("renvoi", Some("AUTRE")),
            ("depens", None),
            ("ceux", None),
            ("remboursement", None),
            ("frais", None),
        ];
        let chars = &self.norm.chars;
        let n = self.len();
        let tail = n.saturating_sub(3000);
        let mut clauses: Vec<(usize, usize)> = Vec::new();
        let mut start = tail;
        for i in tail..n {
            let c = chars[i];
            let dot_end =
                c == '.' && (i + 1 == n || chars[i + 1] == ' ') && (i == 0 || chars[i - 1] != '.');
            if c == ';' || dot_end || self.norm.breaks[i] {
                clauses.push((start, i));
                start = i + 1;
            }
        }
        clauses.push((start, n));
        let mut found: Option<(usize, &'static str)> = None;
        for &(s, e) in clauses.iter().rev() {
            let Some(s) = (s..e).find(|&j| !matches!(chars[j], ' ' | '-')) else {
                continue;
            };
            let clause = fold_chars(&self.text_slice(s, e.min(s + 40)));
            let noun = NOUNS.iter().find(|(w, _)| {
                clause.starts_with(w)
                    && !clause[w.len()..].starts_with(|c: char| c.is_alphanumeric())
            });
            match noun {
                Some((_, Some(key))) => found = Some((s, key)),
                Some((_, None)) => {}
                None => break,
            }
        }
        found
    }

    /// Fin de la zone dispositif : les MOYENS ANNEXÉS d'un arrêt CC le suivent et
    /// regorgent de « en ce qu'il » — ils n'en font pas partie.
    pub fn dispositif_end(&self, from: usize) -> usize {
        self.toks
            .iter()
            .find(|t| t.kind == Mk::MoyenPar && t.s > from)
            .map(|t| t.s)
            .unwrap_or(self.len())
    }

    /// Fin du bandeau d'en-tête de greffe (« CIV. 1 … NON-ADMISSION … ») : premier
    /// marqueur d'ouverture de parties ou de requête. Un texte abîmé n'en tokenise
    /// aucun ; le repli sur les motifs ouvre alors le bandeau au récit, où la
    /// formation d'une décision CITÉE se lit comme la sienne, d'où [`BANDEAU_MAX`].
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
            .unwrap_or_else(|| self.motifs_start().min(BANDEAU_MAX))
    }

    /// Tranche verbatim du bandeau : en-tête de greffe où vivent chambre et pôle.
    pub fn bandeau_text(&self) -> String {
        self.text_slice(0, self.bandeau_end())
    }

    /// Tranche verbatim de l'en-tête complet, jusqu'aux motifs.
    pub fn header_text(&self) -> String {
        self.text_slice(0, self.motifs_start())
    }

    /// Fenêtres VERBATIM `[s-before, e+after)` autour des tokens filtrés, fusionnées
    /// quand elles se chevauchent.
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

    /// Fenêtres des ancres de date d'audience : tokens [`Mk::Audience`], plus les
    /// en-têtes « DÉBATS » / « L'affaire a été débattue » restés Stop.
    pub fn audience_windows(&self) -> Vec<String> {
        self.windows(110, 190, |t| {
            t.kind == Mk::Audience
                || (t.kind == Mk::Stop && fold_chars(&self.text_slice(t.s, t.e)).contains("debat"))
        })
    }

    /// Composition à juge unique dite par le texte, en zone d'en-tête
    /// (« statuant en juge unique », « siégeant seul »).
    pub fn juge_unique_header(&self) -> bool {
        self.find_tok(&[Mk::ProcJugeUnique], 0, self.motifs_start())
            .is_some()
    }

    /// An order: « la présente ordonnance » in the closing part, or the spaced
    /// « O R D O N N E » of the operative part.
    pub fn ordonnance(&self) -> bool {
        self.toks.iter().any(|t| {
            (t.kind == Mk::ProcOrdonnance && t.s + 3000 >= self.len())
                || (t.kind == Mk::Dispositif
                    && fold_chars(&self.text_slice(t.s, t.e)).starts_with("o r d o"))
        })
    }

    /// The Cour de cassation's formation as its decision prints it: the header's
    /// code (« F-D », « FS-P+B », « FP-B+R »), or the criminal chamber's « formation
    /// prévue à l'article 567-1-1 ».
    pub fn cc_formation(&self) -> Option<&'static str> {
        let header = self.text_slice(0, self.motifs_start().min(2000));
        let code = header.split_whitespace().find_map(|w| {
            let (code, marks) = w.trim_matches([',', ';', '(', ')']).split_once('-')?;
            let printed = !marks.is_empty()
                && marks
                    .chars()
                    .all(|c| matches!(c, 'P' | 'B' | 'R' | 'I' | 'D' | '+'));
            match code {
                "F" if printed => Some("formation:RESTREINTE"),
                "FS" if printed => Some("formation:SECTION"),
                "FP" if printed => Some("formation:PLENIERE"),
                _ => None,
            }
        });
        code.or_else(|| {
            self.toks
                .iter()
                .any(|t| t.kind == Mk::ProcRestreinte && self.span_after(t, 30).contains("567-1-1"))
                .then_some("formation:RESTREINTE")
        })
    }

    /// Judges the closing « siégeaient » clause names, the greffier left out. The
    /// clause runs to the sentence ending its line.
    pub fn siege_judges(&self) -> Option<usize> {
        const SINGLE: &[&str] = &["m.", "mme", "mlle", "monsieur", "madame"];
        const PLURAL: &[&str] = &["mm.", "mmes", "mlles", "messieurs", "mesdames"];
        let t = self
            .toks
            .iter()
            .rev()
            .find(|t| t.kind == Mk::ProcSiege && t.s + 2500 >= self.len())?;
        let end = (t.e + 600).min(self.len());
        let stop = (t.e..end)
            .find(|&i| self.norm.chars[i] == '.' && self.norm.breaks.get(i + 1).is_none_or(|b| *b))
            .unwrap_or(end);
        let mut clause = fold_chars(&self.text_slice(t.e, stop));
        for end in [
            "rendu public",
            "lu en audience",
            "le rapporteur",
            "la rapporteure",
            "la republique",
        ] {
            if let Some(i) = clause.find(end) {
                clause.truncate(i);
            }
        }
        let words: Vec<&str> = clause.split_whitespace().collect();
        // « M.D » : a pseudonymised name glued to its civility
        let civility = |w: &str| {
            let w = w.trim_start_matches(['-', ':']);
            let glued =
                w.len() > 2 && w.starts_with("m.") && w[2..].starts_with(char::is_alphabetic);
            (SINGLE.contains(&w) || PLURAL.contains(&w) || glued).then(|| PLURAL.contains(&w))
        };
        let mut judges = 0;
        let mut i = 0;
        while i < words.len() {
            let Some(plural) = civility(words[i]) else {
                i += 1;
                continue;
            };
            let named: Vec<&str> = words[i + 1..]
                .iter()
                .take_while(|w| civility(w).is_none())
                .copied()
                .collect();
            let segment = named.join(" ");
            if !segment
                .chars()
                .take(40)
                .collect::<String>()
                .contains("greff")
            {
                judges += if plural {
                    segment
                        .split(',')
                        .next()
                        .unwrap_or("")
                        .matches(" et ")
                        .count()
                        + 1
                } else {
                    1
                };
            }
            i += 1 + named.len();
        }
        Some(judges)
    }

    /// Votes de termes par domaine, comptés plein texte : familles `ProcDom*`
    /// (admin et judiciaire) et familles procédurales porteuses de matière (JEX,
    /// procédures collectives). Consommés par `domain::refine_with_terms` — clés
    /// du référentiel `legal_domain:*`. `admin` = ordre administratif : le
    /// vocabulaire travail des deux ordres (`ProcDomTravailMixte`) y vote fonction
    /// publique.
    pub fn domain_term_votes(&self, admin: bool) -> Vec<(&'static str, u32)> {
        let mut counts: std::collections::HashMap<&'static str, u32> =
            std::collections::HashMap::new();
        for t in &self.toks {
            let d = match t.kind {
                Mk::ProcDomTravail => "SOCIAL_DROIT_TRAVAIL",
                // un licenciement devant le juge administratif est de la fonction publique
                Mk::ProcDomTravailMixte => {
                    if admin {
                        "PUBLIC_DROIT_TRAVAIL"
                    } else {
                        "SOCIAL_DROIT_TRAVAIL"
                    }
                }
                // AT/MP = contentieux du travail ; cotisations = SOCIAL nu terminal
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
                    if fold_chars(&self.text_slice(t.s, t.e)).starts_with("saisie") {
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
    /// statue seul (L. 511-2 CJA). Garde CESEDA sur L. 551-* : la même surface
    /// d'article y désigne la rétention, pas le précontractuel.
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

    /// Fenêtres depuis « contre l'arrêt / en cassation d'un arrêt » du préambule,
    /// où se nomme la décision attaquée. Une fenêtre par token, sans fusion :
    /// l'appelant ancre sa regex sur la surface.
    pub fn contre_windows(&self) -> Vec<String> {
        self.contre_toks()
            .map(|t| self.text_slice(t.s, (t.e + 300).min(self.len())))
            .collect()
    }

    /// The `n` chars before the `i`-th contre window, where « contre l'arrêt de ladite cour »
    /// finds the court it refers to.
    pub fn contre_lead(&self, i: usize, n: usize) -> Option<String> {
        self.contre_toks()
            .nth(i)
            .map(|t| self.text_slice(t.s.saturating_sub(n), t.s))
    }

    fn contre_toks(&self) -> impl Iterator<Item = &PTok> {
        let fin = self.motifs_start().min(4_000);
        self.toks
            .iter()
            .filter(move |t| t.kind == Mk::Contre && t.s < fin)
    }

    /// Fenêtres autour des ancres « sous le(s) n°/numéro(s) » (AdminReq) du récit
    /// de procédure (zone 0 → liste de visas ou motifs), avec assez
    /// d'avant-contexte pour les gardes (« pourvoi en cassation sous le n°… » =
    /// citation).
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
                && fold_chars(&self.text_slice(t.s, t.e)).starts_with("sous")
        })
    }

    /// Le petit span (chars) qui SUIT un token, plié — pour les exceptions
    /// positionnées (« lieu de statuer *sur les dépens* »).
    fn span_after(&self, t: &PTok, n: usize) -> String {
        fold_chars(&self.text_slice(t.e, (t.e + n).min(self.len())))
    }

    /// Le petit span qui PRÉCÈDE un token, plié (gardes « non/pas prescrit »).
    fn span_before(&self, t: &PTok, n: usize) -> String {
        fold_chars(&self.text_slice(t.s.saturating_sub(n), t.s))
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
                        !(fold_chars(&self.text_slice(t.s, t.e)) == "rejet"
                            && self.span_before(t, 6).ends_with("ou de "))
                    }
                    Mk::OutNeutral => {
                        // l'objet d'une demande rejetée n'est pas le sort (« rejetons la demande de
                        // radiation », « exception d'incompétence, n'y fait pas droit »)
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

    /// Un rejet ou débouté substantiel dans la zone dispositif : sa clause ne vise
    /// ni l'accessoire (art. 700, dépens, frais) ni le reliquat (« surplus »,
    /// « plus amples », « toute autre »).
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
                // au passif (« le surplus des conclusions est rejeté »), l'objet précède le
                // verbe : mêmes exclusions sur le segment avant
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

    /// Marque de partialité dans la clause (même phrase) d'un confirme/infirme.
    /// Après un CONFIRME, « en ce qu'il a… / en ses dispositions soumises à la
    /// cour » énumère les chefs confirmés sans les restreindre : seuls « sauf en ce
    /// qu », « mais seulement », « en ses seules dispositions »… rendent la
    /// confirmation partielle. Après un INFIRME, « en ce qu'il » désigne les chefs
    /// infirmés et reste une partialité. « sauf à + infinitif » ajuste sans infirmer :
    /// exclu.
    fn restrictif_apres(&self, c: &PTok, apres_confirme: bool) -> bool {
        self.toks.iter().any(|s| {
            matches!(s.kind, Mk::OutSauf | Mk::OutPartial)
                && s.s > c.e
                && s.s < c.e + 200
                && !self.text_slice(c.e, s.s).contains('.')
                && !(s.kind == Mk::OutSauf && self.span_after(s, 3).trim_start().starts_with("a "))
                && !(apres_confirme && {
                    let surf = fold_chars(&self.text_slice(s.s, s.e));
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

    /// Un rejet de demande dans la zone dispositif : le rejet d'une défense
    /// (délais de paiement, suspension des effets de la clause résolutoire,
    /// exception de procédure, note écartée des débats) ne rend pas la
    /// satisfaction du demandeur partielle.
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

    /// Clé solution-17 lue dans la zone dispositif ; `true` = la détection prime le
    /// label du greffe (désistement constaté, pourvoi non admis). L'ordre de
    /// composition suit le gabarit.
    pub fn outcome(&self) -> Option<(&'static str, bool)> {
        // pourvoi non admis : décisif partout — exige « pourvoi » dans la surface ou
        // juste avant (« M. X n'est pas admis au bénéfice de l'aide
        // juridictionnelle » exclu)
        let non_admis = self.toks.iter().any(|t| {
            t.kind == Mk::OutNonAdmis && {
                let surf = fold_chars(&self.text_slice(t.s, t.e));
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
            return Some(("NON_ADMISSION", true));
        }
        let from = self.dispositif_start();
        if from >= self.len() {
            return None;
        }
        if let Some((_, key)) = self.abstract_closing().filter(|(s, _)| *s == from) {
            return Some((key, false));
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
                // mixte irrecevable + rejet au fond : le fond absorbe. Le rejet accessoire
                // (art. 700, dépens, surplus) n'absorbe pas.
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
                // condamnation substantielle (« à payer/verser », « somme de », « dommages »
                // dans la clause) vs procédurale (art. 700, dépens)
                let cond = self.toks.iter().any(|t| {
                    t.s >= from && t.s < end && t.kind == Mk::OutCondamne && {
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
                // faire droit à la demande sans condamner : ouverture de procédure collective,
                // sanction de gestion, mainlevée JLD, ordonnance commune ou prorogation en
                // référé
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
                // non-lieu décisif seulement quand le dispositif n'infirme ni ne confirme, ne
                // condamne, n'accueille ni ne rejette rien de substantiel par ailleurs
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
                if irrec && !self.substantive_rejet(from) {
                    return Some(("IRRECEVABILITE", false));
                }
                let rej = self.disp_has(from, Mk::OutRejette);
                // cond/grant priment le neutre ; le rejet nu reste derrière eux (« sursoit à
                // statuer… rejette le surplus » demeure AUTRE)
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
                // le dispositif se lit verbatim. Jugement ou acte annulé, en tout ou partie →
                // ANNULATION (« réformé » → REFORMATION) ; requête rejetée → REJET, sauf
                // irrecevabilité prononcée (cf. `irrec_pronounced`) ; SATISFACTION_* = plein
                // contentieux gagné sans annulation (condamnation à payer, décharge).
                // Only the Cour de cassation « casse », and its rulings without a
                // pourvoi preamble land here.
                if self.disp_has(from, Mk::OutCasse) {
                    return Some(if self.disp_has(from, Mk::OutPartial) {
                        ("CASSATION_PARTIELLE", false)
                    } else {
                        ("CASSATION", false)
                    });
                }
                if self.disp_has(from, Mk::OutAnnule) {
                    return Some(("ANNULATION", false));
                }
                // ordonnances judiciaires sans étiquettes de bloc de greffe (JLD rétention ou
                // hospitalisation, référés) routées ici faute de pivot : lecture appel
                // judiciaire, mêmes règles de partialité que le gabarit Blocs
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
                    let surf = fold_chars(&self.text_slice(t.s, t.e));
                    let key = if surf.starts_with("reform") {
                        "REFORMATION"
                    } else {
                        "ANNULATION"
                    };
                    return Some((key, false));
                }
                // non-lieu partiel + rejet substantiel = rejet ; le rejet du seul accessoire
                // (« le surplus des conclusions est rejeté », art. 700/761-1) laisse le
                // non-lieu principal
                if self.disp_has(from, Mk::OutNonLieu) {
                    if self.substantive_rejet(from) {
                        return Some(("REJET", false));
                    }
                    return Some(("NON_LIEU_A_STATUER", false));
                }
                let end = self.dispositif_end(from);
                let cond = self.toks.iter().any(|t| {
                    t.s >= from && t.s < end && t.kind == Mk::OutCondamne && {
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
                self.admin_procedural(from).map(|key| (key, false))
            }
        }
    }

    /// An administrative operative part with no ruling verb: a requête not admitted, a
    /// non-lieu missing its « pas », or a case sent to another court or stayed.
    fn admin_procedural(&self, from: usize) -> Option<&'static str> {
        let disp = self.folded_slice(from, self.dispositif_end(from).min(from + 600));
        if disp.contains("question prioritaire") || disp.contains("conseil constitutionnel") {
            return None;
        }
        if let Some(at) = disp.find("pas admis") {
            let clause = disp[..at].rsplit(':').next().unwrap_or_default();
            if (clause.contains("requete") || clause.contains("pourvoi"))
                && !clause.contains("intervention")
            {
                return Some("NON_ADMISSION");
            }
        }
        const NON_LIEU: &[&str] = &[
            "il n'y a lieu de statuer",
            "il n'y pas lieu de statuer",
            "il n'y a pas lieu, en l'etat, de statuer",
        ];
        if NON_LIEU.iter().any(|p| disp.contains(p)) {
            return Some("NON_LIEU_A_STATUER");
        }
        const ELSEWHERE: &[&str] = &[
            "est attribue",
            "est renvoye",
            "est transmis",
            "sursis a statuer",
            "avant de statuer",
        ];
        ELSEWHERE
            .iter()
            .any(|p| disp.contains(p))
            .then_some("AUTRE")
    }

    /// Irrecevabilité prononcée : un token d'irrecevabilité et un token de rejet
    /// dans la même phrase, entre la clause de clôture des motifs (≤ 300 chars
    /// avant le dispositif) et la fin du dispositif — « manifestement irrecevable
    /// et doit être rejetée », « rejette la requête comme irrecevable ». Les fins
    /// de non-recevoir discutées plus haut ne s'apparient pas. Frontière de phrase
    /// = « . » précédé d'autre chose qu'une majuscule.
    fn irrec_pronounced(&self, from: usize) -> bool {
        let end = self.dispositif_end(from);
        let zone = from.saturating_sub(300);
        // même chaîne de prononcé : pas de frontière de phrase entre les deux tokens.
        // « . » ne coupe pas après une majuscule ou un point (« R. 222-1 », « M. »,
        // ellipse « Mme A... »), ni quand la phrase suivante enchaîne la conséquence
        // (« irrecevable. Par suite, il y a lieu de rejeter… »).
        let same_reasoning = |a: usize, b: usize| {
            let slice = self.text_slice(a, b);
            let chars: Vec<char> = slice.chars().collect();
            for i in 0..chars.len() {
                let prev = if i == 0 { ' ' } else { chars[i - 1] };
                if chars[i] != '.' || prev.is_ascii_uppercase() || prev == '.' {
                    continue;
                }
                let rest: String = chars[i + 1..].iter().collect();
                let rest = fold_chars(rest.trim_start());
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
                    // « créance prescrite » (prescription quadriennale) est un moyen de fond : la
                    // surface prescrit* ne prononce pas d'irrecevabilité
                    !(b.ends_with("non ")
                        || b.ends_with("pas ")
                        || fold_chars(&self.text_slice(t.s, t.e)).starts_with("prescrit"))
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

    /// Irrecevabilité prononcée dans les motifs de clôture (≤ 800 chars avant le
    /// dispositif, chaque phrase étendue à sa vraie frontière) — complète
    /// `irrec_pronounced` pour les ordonnances dont le prononcé précède des
    /// paragraphes de conséquence ou de frais avant un dispositif « rejette » nu. La
    /// phrase doit viser l'objet contentieux (requête, conclusions, recours,
    /// pourvoi) ; sont exclus l'irrecevabilité d'un moyen, la citation de la règle
    /// (« aux termes »), l'attribution au premier juge et tout marqueur de fond
    /// (« dépourvue de fondement », « mal fondée ») entre le prononcé et le
    /// dispositif.
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
            let sent = fold_chars(&self.text_slice(sa, sb));
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
            let to_disp = fold_chars(&self.text_slice(sb, from));
            if to_disp.contains("sur les conclusions") {
                continue;
            }
            let span = fold_chars(&self.text_slice(sa, from));
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

    /// La cassation est-elle partielle ? Indice dans le dispositif, ou « casse …
    /// <partiel> » dans les 300 chars du verbe, n'importe où. « casse et annule,
    /// dans/en toutes ses dispositions » force la totalité.
    pub fn cassation_partial(&self) -> bool {
        let from = self.dispositif_start();
        let end = self.dispositif_end(from);
        let toutes = self.toks.iter().any(|t| {
            t.s >= from && t.s < end && t.kind == Mk::OutCasse && {
                let a = fold_chars(self.span_after(t, 40).trim_start());
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

    /// Le gabarit détecté est-il celui d'un pourvoi en cassation ? Le vocabulaire de
    /// sortie de la solution en dépend : gagner = casser.
    pub fn gabarit_cc(&self) -> bool {
        self.gabarit() == Gabarit::Cc
    }

    /// « pourvoi » dans l'EN-TÊTE : pourvoi en cassation administratif (CE) dont le
    /// gabarit reste Admin — le vocabulaire de sortie devient celui de la cassation
    /// (annuler = casser).
    pub fn header_has_pourvoi(&self) -> bool {
        let end = self.motifs_start();
        fold_chars(&self.text_slice(0, end)).contains("pourvoi")
    }

    /// Signaux procéduraux lus dans le texte — combinés aux métadonnées par
    /// `extract::extract_procedure`. Zone en-tête sauf mention contraire.
    pub fn procedure_signals(&self) -> ProcSignals {
        let header = self.motifs_start();
        let bandeau = self.bandeau_end();
        let has_h = |k: Mk| self.find_tok(&[k], 0, header).is_some();
        let has_b = |k: Mk| self.find_tok(&[k], 0, bandeau).is_some();
        let has_any = |k: Mk| self.toks.iter().any(|t| t.kind == k);
        // R222-1 qualifié : « dernier alinéa » ou « alinéa 4 » devant, ou
        // manifestement/irrecevable/tardive/hors délai/sans ministère d'avocat dans
        // les 180 chars suivants
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
                // « manifestement irrecevable » / « irrecevabilité manifeste » sans citation
                // R. 222-1. Lu sur le token OutIrrec : une surface composée le volerait au
                // leftmost-longest.
                || (t.kind == Mk::OutIrrec
                    && (self.span_before(t, 20).trim_end().ends_with("manifestement")
                        || self.span_after(t, 12).trim_start().starts_with("manifeste")))
        });
        // formule active de désignation en tête : « le président de la cour / du
        // tribunal a désigné M./Mme X » suivie de l'objet de la délégation (référés,
        // R. 222-1, « pour statuer »). Le passif « désigné par » raconte le jugement
        // attaqué et ne produit pas ce token.
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
            // bandeau seulement : cité plus bas, QPC et non-admission décrivent la
            // procédure antérieure
            qpc: has_b(Mk::ProcQpc),
            // formules PAPC décisives seulement (« l'admission est refusée », « non
            // spécialement motivé ») ; « NON-ADMISSION » au bandeau CC. Les citations
            // L822-1/R822-5 et « procédure préalable d'admission » sont du boilerplate de
            // visa. Gate juridiction (CC/CE) dans `extract_procedure`.
            papc: self.toks.iter().any(|t| {
                t.kind == Mk::ProcPapc && {
                    let s = fold_chars(&self.text_slice(t.s, t.e));
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
            // référé judiciaire dit par le texte : « Vu l'assignation en référé » en tête,
            // « ordonnance de référé » nue au bandeau (titre de la décision ou de la
            // décision déférée) ou précédée de appel d'une / confirme l' / réforme l'
            refere_civil: self.toks.iter().any(|t| {
                (t.kind == Mk::ProcRefCivil && {
                    let surf = fold_chars(&self.text_slice(t.s, t.e));
                    if surf == "assignation en refere" {
                        t.s < header && self.span_before(t, 6).ends_with("vu l'")
                    } else {
                        // « Par ordonnance de référé du …, X a été désigné » : récit d'un référé
                        // antérieur
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
                    let surf = fold_chars(&self.text_slice(t.s, t.e));
                    surf != "premier president de la cour"
                        || self.span_before(t, 60).contains("nous")
                }
            }),
            // « magistrat désigné » adjectival partout ; « président désigné » seulement
            // clos par ponctuation (signature)
            magdes: self.toks.iter().any(|t| {
                t.kind == Mk::ProcMagdes && {
                    let surf = fold_chars(&self.text_slice(t.s, t.e));
                    !surf.starts_with("president")
                        || self
                            .span_after(t, 3)
                            .trim_start()
                            .starts_with([',', '.', ';', ')'])
                }
            }),
            // « …désigné » en pied de décision = signature du magistrat qui a rendu
            // l'ordonnance ; les occurrences d'en-tête désignent le juge du jugement
            // attaqué
            magdes_tail: self
                .toks
                .iter()
                .any(|t| t.kind == Mk::ProcMagdes && t.s + 700 >= self.len()),
            mee_signed: self.toks.iter().any(|t| {
                t.kind == Mk::ProcMee && {
                    // « confirme l'ordonnance rendue … par le conseiller de la mise en état » names the order under appeal
                    let b = self.span_before(t, 24);
                    let named = b.ends_with("du ")
                        || b.ends_with("devant le ")
                        || ((b.ends_with("par le ") || b.ends_with("par la "))
                            && !b.contains("signe"))
                        || fold_chars(&self.text_slice(t.s, t.e)).starts_with("ordonnance");
                    !named
                        && (t.s + 700 >= self.len() || self.span_before(t, 120).contains("nous,"))
                }
            }),
            mee_banner: self.toks.iter().any(|t| {
                t.kind == Mk::ProcMee
                    && t.s < header
                    && fold_chars(&self.text_slice(t.s, t.e)).starts_with("ordonnance")
            }),
            magdes_form_cour: magdes_form("de la cour"),
            magdes_form_trib: magdes_form("du tribunal"),
            // « demande(nt) au juge des référés » en tête : la requête est adressée au
            // juge des référés de la juridiction saisie, il statue seul (L. 511-2 CJA).
            // Gabarit première instance seulement, côté `extract`.
            jref_demande: self.toks.iter().any(|t| {
                t.s < header && t.kind == Mk::ProcJref && {
                    let b = self.span_before(t, 14);
                    b.ends_with("demande au ") || b.ends_with("demandent au ")
                }
            }),
            // « au juge des référés du Conseil d'État » : premier ressort ou appel
            // L. 521-2 — le juge des référés du CE statue seul
            jref_conseil: self.toks.iter().any(|t| {
                t.s < header
                    && t.kind == Mk::ProcJref
                    && self
                        .span_after(t, 12)
                        .trim_start()
                        .starts_with("du conseil")
            }),
            // JEX opérationnel seulement : précédé de « jugement du », « rendu par le »,
            // « décision du »…, c'est la décision attaquée ou l'historique, pas l'office
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
                    let s = fold_chars(&self.text_slice(t.s, t.e));
                    s.starts_with("saisie")
                }
            }),
            proc_collective: has_h(Mk::ProcCollective),
            // vocabulaire domaine en-tête (objet de la requête) ; immig partout, OQTF et
            // CESEDA sont univoques où qu'ils apparaissent
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

    /// Gabarit admin : requérants de tous les blocs de requête de l'en-tête (« 1°
    /// Sous le n°…, par une requête…, X et Y demandent… 2° Sous le n°… »), chacun
    /// borné à son verbe « demande(nt) ». Sans ouverture de bloc détectée :
    /// en-tête simple borné au premier « demande ».
    /// Segments requérants du gabarit admin : un par bloc de requête, borné à
    /// son verbe « demande(nt) ».
    fn admin_segments(&self) -> Vec<(usize, usize)> {
        let end = self.motifs_start();
        let reqs: Vec<usize> = self
            .toks
            .iter()
            .filter(|t| t.kind == Mk::AdminReq && t.s < end)
            .map(|t| t.e)
            .collect();
        if reqs.is_empty() {
            // corps de moyens CC sans préambule : le texte ouvre sur la prose des motifs
            if self.toks.iter().any(|t| t.kind == Mk::DefEnd && t.s < 300) {
                return Vec::new();
            }
            return vec![(0, self.demande_boundary(0, end))];
        }
        reqs.iter()
            .enumerate()
            .map(|(i, &rs)| {
                let next = reqs.get(i + 1).copied().unwrap_or(end);
                (rs, self.demande_boundary(rs, next))
            })
            .collect()
    }

    pub fn admin_companies(&self) -> Vec<Mention> {
        let mut out: Vec<Mention> = Vec::new();
        for (from, to) in self.admin_segments() {
            for m in self.harvest(from, to, to) {
                if !out.iter().any(|o| o.value == m.value) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// Gabarit admin : la partie défenderesse, lue dans la clause de
    /// notification du dispositif (« Le présent arrêt sera notifié à M. A et au
    /// ministre de l'intérieur »), qui énumère les parties de l'instance. Ce qui
    /// y figure et n'est pas un requérant défend.
    ///
    /// « Copie en sera adressée à … » désigne un intéressé, pas une partie : la
    /// fenêtre s'arrête avant.
    pub fn admin_defendants(&self) -> Vec<Mention> {
        let apps_f: Vec<String> = self
            .admin_companies()
            .iter()
            .map(|a| fold_chars(&a.value))
            .collect();
        let mut out: Vec<Mention> = Vec::new();
        // « le ministre de l'action et des comptes publics conclut au rejet » :
        // le verbe qualifie la defense, son SUJET est la partie defenderesse.
        for t in self.toks.iter().filter(|t| t.kind == Mk::DefConclu) {
            let from = self.sentence_start(t.s, 0);
            for m in self.autorites_in(from, t.s) {
                let f = fold_chars(&m.value);
                if side_match(&f, &apps_f, &[]) {
                    continue;
                }
                Self::pousse_la_plus_longue(&mut out, m);
            }
        }
        for t in self.toks.iter().filter(|t| t.kind == Mk::NotifPartie) {
            let to = self.sentence_end(t.e, (t.e + 300).min(self.len()));
            for (from, upto) in self.enumeration_cells(t.e, to) {
                let mut cellule = self.harvest(from, upto, upto);
                cellule.extend(self.autorites_in(from, upto));
                for m in cellule {
                    let f = fold_chars(&m.value);
                    // une valeur contenue dans une autre est la meme partie vue
                    // deux fois (« commune de Conca » et « Conca ») : la plus
                    // longue porte le titre entier
                    if side_match(&f, &apps_f, &[]) {
                        continue;
                    }
                    Self::pousse_la_plus_longue(&mut out, m);
                }
            }
        }
        out
    }

    /// Une valeur contenue dans une autre est la meme partie vue deux fois
    /// (« commune de Conca » et « Conca ») : la plus longue porte le titre.
    fn pousse_la_plus_longue(out: &mut Vec<Mention>, m: Mention) {
        let f = fold_chars(&m.value);
        match out.iter_mut().find(|o| {
            let g = fold_chars(&o.value);
            g == f || g.contains(&f) || f.contains(&g)
        }) {
            Some(o) if o.value.len() < m.value.len() => *o = m,
            Some(_) => {}
            None => out.push(m),
        }
    }

    /// Debut de la phrase qui contient `pos`, sans descendre sous `plancher`.
    fn sentence_start(&self, pos: usize, plancher: usize) -> usize {
        let mut i = pos;
        while i > plancher {
            if self.norm.chars[i - 1] == ' '
                && i >= 2
                && matches!(self.norm.chars[i - 2], '.' | ';' | ':')
            {
                return i;
            }
            i -= 1;
        }
        plancher
    }

    /// Autorites administratives d'une fenetre : la tete porte son titre
    /// entier, virgules comprises (« ministre de l'economie, des finances et de
    /// la relance »), ce que l'extension de nom ordinaire coupe a la virgule.
    fn autorites_in(&self, from: usize, to: usize) -> Vec<Mention> {
        let mut out: Vec<Mention> = Vec::new();
        for t in self.toks.iter() {
            if t.kind != Mk::AutoriteHead || t.s < from || t.s >= to {
                continue;
            }
            let e = self.autorite_end(t.e, to);
            if e <= t.e {
                continue;
            }
            let v = self.text_slice(t.s, e);
            let v = v.trim().trim_end_matches([',', ';', '.', ' ']);
            if v.len() < 4 {
                continue;
            }
            out.push(Mention::new(t.s, e, v.to_string()));
        }
        out
    }

    /// Fin du titre d'une autorite. La virgule ne le ferme que si ce qui suit
    /// n'en est pas la suite : un titre enchaine ses complements (« , des
    /// finances », « , charge du logement », « , ministre de la justice »).
    fn autorite_end(&self, from: usize, to: usize) -> usize {
        const SUITE: [&str; 8] = [
            " de ",
            " du ",
            " des ",
            " d'",
            " charge",
            " et de",
            " et des",
            " ministre",
        ];
        let mut i = from;
        while i < to {
            match self.norm.chars[i] {
                '.' | ';' | ':' | ')' | '(' => return i,
                ',' => {
                    let apres = (i + 1).min(to);
                    let fin = (apres + 10).min(to);
                    let f = self.folded_slice(apres, fin);
                    if !SUITE.iter().any(|k| f.starts_with(*k)) {
                        return i;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        to
    }

    /// Decoupe une enumeration de parties en cellules : « a A et au B et a la
    /// C » donne trois fenetres. Sans la coupe, la moisson traverse le
    /// connecteur et colle deux parties en une seule valeur.
    fn enumeration_cells(&self, from: usize, to: usize) -> Vec<(usize, usize)> {
        const CONNECTEURS: [&str; 4] = [" et a ", " et au ", " et aux ", " et a l"];
        let mut cells = Vec::new();
        let mut debut = from;
        let mut i = from;
        while i < to {
            let fin = (i + 8).min(to);
            let f = self.folded_slice(i, fin);
            match CONNECTEURS.iter().find(|c| f.starts_with(**c)) {
                Some(c) => {
                    // le connecteur n'appartient a aucune des deux cellules
                    cells.push((debut, i));
                    debut = self.source_after(i, c.len());
                    i = debut;
                }
                None => i += 1,
            }
        }
        cells.push((debut, to));
        cells
    }

    /// Gabarit structurel du document, lu dans le flux de tokens de l'en-tête.
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

    /// Point d'entrée unique : (demandeurs, défendeurs). Le gabarit est détecté
    /// depuis le flux de tokens lui-même : pivot de pourvoi → CC ; étiquettes de
    /// bloc de greffe → judiciaire du fond ; ouvertures de requête → admin
    /// (défendeur = administration, jamais une société).
    pub fn companies(&self) -> (Vec<String>, Vec<String>) {
        use crate::registry::{Quality, Side};
        let r = self.party_registry();
        (
            r.view(Some(Side::Applicant), Quality::Party),
            r.view(Some(Side::Defendant), Quality::Party),
        )
    }

    /// Registre de parties mémoïsé, construit une fois depuis les moissons par
    /// gabarit ; `companies`, `counsel` et `intervenors` en sont des projections.
    pub fn party_registry(&self) -> &crate::registry::PartyRegistry {
        self.registry_memo.get_or_init(|| {
            use crate::registry::{PartyRegistry, Quality, Side};
            let mut r = PartyRegistry::default();
            let (apps, defs) = self.companies_uncached();
            r.push(apps, Some(Side::Applicant), Quality::Party);
            r.push(defs, Some(Side::Defendant), Quality::Party);
            let (ia, id) = self.individuals_uncached();
            r.push(ia, Some(Side::Applicant), Quality::Individual);
            r.push(id, Some(Side::Defendant), Quality::Individual);
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

    /// Parties personnes physiques par côté, sur les MÊMES segments que les
    /// personnes morales : la nature de la partie ne change pas où elle se
    /// nomme. Côté défendeur admin, la clause de notification du dispositif.
    fn individuals_uncached(&self) -> (Vec<Mention>, Vec<Mention>) {
        let mut apps: Vec<Mention> = Vec::new();
        let mut defs: Vec<Mention> = Vec::new();
        let pousse = |cible: &mut Vec<Mention>, from: usize, to: usize, s: &Self| {
            for m in s.persons_in(from, to) {
                if !cible.iter().any(|o| same_words(&o.value, &m.value)) {
                    cible.push(m);
                }
            }
        };
        match self.gabarit() {
            Gabarit::Cc => {
                if let Some(segs) = self.cc_segments() {
                    for &(from, to) in &segs.app {
                        pousse(&mut apps, from, to, self);
                    }
                    for &(from, to) in &segs.def {
                        pousse(&mut defs, from, to, self);
                    }
                }
            }
            Gabarit::Blocs => {
                for (kind, from, to) in self.block_segments() {
                    match kind {
                        Mk::BlockApp => pousse(&mut apps, from, to, self),
                        Mk::BlockDef => pousse(&mut defs, from, to, self),
                        _ => continue,
                    }
                }
            }
            Gabarit::Admin => {
                for (from, to) in self.admin_segments() {
                    pousse(&mut apps, from, to, self);
                }
                for t in self.toks.iter().filter(|t| t.kind == Mk::NotifPartie) {
                    let to = self.sentence_end(t.e, (t.e + 300).min(self.len()));
                    for (from, upto) in self.enumeration_cells(t.e, to) {
                        pousse(&mut defs, from, upto, self);
                    }
                }
            }
        }
        // partie nommée des deux côtés : le segment demandeur prime. L'arbitrage
        // porte sur les mentions telles que lues, avant que la résolution ne les
        // rallonge, sinon un défendeur tombe contre une graphie qu'il ne
        // partageait pas (11 records au banc).
        defs.retain(|d| !apps.iter().any(|a| same_words(&a.value, &d.value)));
        // côté demandeur seul : la clause de notification qui nomme les
        // défendeurs porte déjà la graphie longue, la résoudre lui coûte
        // 20 records
        Self::resout_variantes(&mut apps, &self.persons_in(0, self.motifs_start()));
        (apps, defs)
    }

    /// Deux graphies de la même personne : « M. A B » dans le rappel de
    /// procédure, « M. B » dans la requête. La plus longue porte l'identité, et
    /// deux courtes qui remontent à la même longue n'en font qu'une.
    fn resout_variantes(mentions: &mut Vec<Mention>, toutes: &[Mention]) {
        if mentions.is_empty() {
            return;
        }
        let candidats: Vec<(Vec<String>, &Mention)> = toutes
            .iter()
            .map(|c| (mots_personne(&c.value), c))
            .collect();
        for m in mentions.iter_mut() {
            let court = mots_personne(&m.value);
            let mut retenu: Option<(usize, &Mention)> = None;
            for (long, c) in &candidats {
                if long.len() <= court.len()
                    || !(long.starts_with(&court[..]) || long.ends_with(&court[..]))
                {
                    continue;
                }
                if retenu.is_none_or(|(n, _)| n < long.len()) {
                    retenu = Some((long.len(), c));
                }
            }
            if let Some((_, c)) = retenu {
                *m = c.clone();
            }
        }
        let mut vus: Vec<String> = Vec::new();
        let mut i = 0;
        while i < mentions.len() {
            let k = mots_personne(&mentions[i].value).join(" ");
            if vus.contains(&k) {
                mentions.remove(i);
            } else {
                vus.push(k);
                i += 1;
            }
        }
    }

    fn companies_uncached(&self) -> (Vec<Mention>, Vec<Mention>) {
        let (mut apps, mut defs) = match self.gabarit() {
            Gabarit::Cc => self.cc_companies(),
            Gabarit::Blocs => self.block_companies(),
            Gabarit::Admin => (self.admin_companies(), self.admin_defendants()),
        };
        // moyens annexés CC (« Moyen produit par <conseil> … pour la société X ») :
        // filet parties demanderesses quand le préambule n'en nomme pas
        if apps.is_empty() {
            apps = self.moyen_par_parties();
        }
        // arbitrage de qualité par force du signal par mention : une entité dont
        // toutes les mentions fortes sont côté cabinet n'est pas une partie
        apps.retain(|m| !self.drop_from_companies(&m.value));
        defs.retain(|m| !self.drop_from_companies(&m.value));
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

    fn counsel_uncached(&self) -> CounselMentions {
        let mut out = CounselMentions::default();
        match self.gabarit() {
            Gabarit::Cc => self.cc_counsel(&mut out),
            Gabarit::Blocs => self.block_counsel(&mut out),
            Gabarit::Admin => self.admin_counsel(&mut out),
        }
        // moyens annexés CC : filet conseil demandeur quand le préambule n'en nomme
        // pas
        let (mp_names, mp_firms) = self.moyen_par_counsel();
        if out.applicant_firms.is_empty() {
            out.applicant_firms = mp_firms;
        }
        if out.applicant_names.is_empty() {
            out.applicant_names = mp_names;
        }
        // arbitrage de qualité par force du signal par mention : une entité dont
        // toutes les mentions fortes sont côté partie n'est pas un cabinet
        out.applicant_firms
            .retain(|f| !self.drop_from_firms(&f.value));
        out.defendant_firms
            .retain(|f| !self.drop_from_firms(&f.value));
        un_seul_cote(&mut out.applicant_names, &mut out.defendant_names);
        un_seul_cote(&mut out.applicant_firms, &mut out.defendant_firms);
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
        let w = fold_chars(&self.text_slice(j.e, (j.e + 60).min(end)));
        (w.contains("forme par") || w.contains("formes par")).then_some(j)
    }

    /// Le pivot ouvre-t-il un pourvoi incident ou provoqué (« a formé un pourvoi
    /// incident contre le même arrêt ») ? Ces pivots n'ouvrent pas de zone : les
    /// rôles restent ceux des pourvois principaux.
    fn pivot_incident(&self, p: &PTok) -> bool {
        let tail: String = self.norm.chars[p.e..(p.e + 20).min(self.len())]
            .iter()
            .collect();
        let tail = tail.to_lowercase();
        tail.contains("incident") || tail.contains("provoqu")
    }

    /// Segments demandeurs/défendeurs du préambule CC ; `None` = pas de pivot. Un
    /// arrêt de jonction porte plusieurs pourvois principaux (« I - Statuant sur
    /// le pourvoi n° X formé par… II - … ») : une zone demandeurs et une fenêtre
    /// défendeurs par pivot.
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
            // jonction ancienne sans pivot (« Joint les pourvois n° U formé par X, n° A
            // formé par Y… ; ») : l'ancre de jonction ouvre la zone demandeurs, bornée à
            // la première frontière structurelle
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
                // le préambule des pourvois joints s'arrête à la première frontière
                // d'observations (« Sur le rapport », « invoque », « Vu la communication »…)
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
    pub fn cc_companies(&self) -> (Vec<Mention>, Vec<Mention>) {
        let Some(segs) = self.cc_segments() else {
            return (Vec::new(), Vec::new());
        };
        let end = self.motifs_start();
        let mut applicants: Vec<Mention> = Vec::new();
        for &(from, to) in &segs.app {
            for m in self.harvest(from, to, to) {
                if !applicants.iter().any(|o| same_words(&o.value, &m.value)) {
                    applicants.push(m);
                }
            }
        }
        let mut defendants: Vec<Mention> = Vec::new();
        for &(from, to) in &segs.def {
            for m in self.harvest(from, to, to) {
                if !defendants.iter().any(|o| same_words(&o.value, &m.value)) {
                    defendants.push(m);
                }
            }
        }
        // demandeur nommé des deux côtés : le pivot prime la fenêtre défendeurs
        defendants.retain(|d| !applicants.iter().any(|a| same_words(&a.value, &d.value)));
        if defendants.is_empty() {
            // filet : « avocat de <partie> » des observations, moins les demandeurs déjà
            // connus (comparaison pliée et variantes de graphie)
            let apps_f: Vec<String> = applicants.iter().map(|a| fold_chars(&a.value)).collect();
            for t in self.toks.iter().filter(|t| t.kind == Mk::AvocatDe) {
                if t.s >= end {
                    break;
                }
                let w_end = (t.e + 140).min(end);
                for c in self.harvest(t.e, w_end, w_end) {
                    let cf = fold_chars(&c.value);
                    if apps_f.iter().any(|a| a.contains(cf.as_str()))
                        || side_match(&cf, &apps_f, &[])
                    {
                        continue;
                    }
                    if !defendants.iter().any(|o| o.value == c.value) {
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
        // layout suffixe (« Monsieur X … représenté par Me A ⏎ APPELANT ») : l'intro
        // de conseil colle à l'étiquette qu'elle précède, fenêtre en fin de token.
        // L'intro qui présente le personnel judiciaire (« Assistée de Marion COBOS,
        // Greffier. ⏎ DEMANDERESSE ») ne vote pas pour le suffixe.
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
    pub fn block_companies(&self) -> (Vec<Mention>, Vec<Mention>) {
        let mut applicants: Vec<Mention> = Vec::new();
        let mut defendants: Vec<Mention> = Vec::new();
        for (kind, from, to) in self.block_segments() {
            let side = match kind {
                Mk::BlockApp => &mut applicants,
                Mk::BlockDef => &mut defendants,
                _ => continue,
            };
            for m in self.harvest(from, to, to) {
                if !side.iter().any(|o| o.value == m.value) {
                    side.push(m);
                }
            }
        }
        (applicants, defendants)
    }

    /// Intervenants : segments étiquetés du greffe (« PARTIE INTERVENANTE : »,
    /// « INTERVENANT(E)(S) »), apposition parenthésée (« (Intervenant forcé) »),
    /// mémoires en intervention (admin).
    pub fn intervenors(&self) -> Vec<String> {
        self.party_registry()
            .view(None, crate::registry::Quality::Intervenor)
    }

    fn intervenors_uncached(&self) -> Vec<Mention> {
        let mut out: Vec<Mention> = Vec::new();
        // queue de récolte : connecteurs traînants (« CPAM DE CHARENTE DE ») et
        // placeholders d'adresse empilés — un placeholder après un autre groupe fermé
        // est du bloc adresse, un placeholder seul peut être le nom même
        // (« RÉSIDENCE [Adresse 11] »)
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
        // dédup par contenance pliée (« Organisme [3] » ↔ « organisme [3] »).
        // La queue rognée sort de la valeur, pas de la tranche : celle-ci reste
        // l'occurrence décisive, elle n'est pas la copie de la valeur.
        fn push(mut m: Mention, out: &mut Vec<Mention>) {
            m.value = trim_tail(&m.value).to_string();
            if m.value.is_empty() {
                return;
            }
            let vu = m.value.to_uppercase();
            if !out.iter().any(|o| {
                let ou = o.value.to_uppercase();
                ou.starts_with(&vu) || ou.ends_with(&vu)
            }) {
                out.push(m);
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
            // étiquette-mot : casse de greffe stricte (tout en capitales)
            let wordy = lf.starts_with("intervenant") || lf.starts_with("partie");
            if (wordy && !is_upper_span(&self.norm.chars, label.s, label.e))
                || lf == "autres parties"
                || label.s >= hdr
            {
                continue;
            }
            // bloc de greffe compact : bornage aux motifs et à l'audience, plus un
            // garde-fou de taille
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
            // entrée tout-caps sans tête de forme collée à l'étiquette (« INTERVENANT
            // VOLONTAIRE ⏎ ALLIANZ IARD, [Adresse 1] ») : on prend le nom de tête du bloc
            // quand son premier mot est en capitales, après les qualificatifs
            // d'étiquette, puces et articles
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
                            push(Mention::new(ns, ne, name), &mut out);
                        }
                    }
                }
                // entrées empilées du greffe (« FIVA, demeurant [Adresse 6] ⏎ DRJSCS,
                // demeurant [Adresse 1] ») : un nom tout-caps sans tête de forme par entrée,
                // signé par « , demeurant »
                let (bs, be) = (self.norm.char2byte[from], self.norm.char2byte[to]);
                for (off, _) in self.norm.folded[bs..be].match_indices(", demeurant") {
                    let comma = self.norm.byte2char[bs + off];
                    let mut e = comma;
                    while e > from && self.norm.chars[e - 1] == ' ' {
                        e -= 1;
                    }
                    // marche arrière mot-à-mot : seuls des mots tout-caps s'agrègent (les sauts
                    // de ligne sont des espaces dans norm)
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
                            push(Mention::new(s, e, name), &mut out);
                        }
                    }
                }
            }
            for m in self.harvest_in(from, to, to, true) {
                push(m, &mut out);
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
            if let Some(m) = self.harvest_in(from, t.s, t.s, true).pop() {
                push(m, &mut out);
            }
        }
        // mémoire en intervention : les entités de la fenêtre aval (« présenté
        // pour la fédération X et pour le club Y »)
        for t in self.toks.iter().filter(|t| t.kind == Mk::IntervIntro) {
            // ancre entre guillemets : un intitulé cité, pas une intervention reçue
            let before = (0..t.s)
                .rev()
                .map(|i| self.norm.chars[i])
                .find(|c| *c != ' ');
            if matches!(before, Some('"' | '«' | '“')) {
                continue;
            }
            let to = (t.e + 220).min(self.len());
            for m in self.harvest_in(t.e, to, to, true) {
                push(m, &mut out);
            }
        }
        // re-casse d'élision : « L'organisme [3] » livre « organisme [3] », la
        // majuscule mangée par l'article. Si toutes les occurrences minuscules du
        // document sont élidées et que la chaîne existe avec initiale majuscule,
        // c'est elle le nom — verbatim ancré, jamais de casse fabriquée.
        let mut text: Option<String> = None;
        for m in &mut out {
            let Some(first) = m.value.chars().next().filter(|c| c.is_lowercase()) else {
                continue;
            };
            let cap: String = first
                .to_uppercase()
                .chain(m.value.chars().skip(1))
                .collect();
            let text = text.get_or_insert_with(|| self.norm.chars.iter().collect());
            let all_elided = text.match_indices(m.value.as_str()).all(|(i, _)| {
                text[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c == '\'' || c == '\u{2019}')
            });
            if all_elided && text.contains(&cap) {
                m.value = cap;
            }
        }
        out
    }

    /// Gabarit blocs : les conseils de chaque segment vont au côté de son
    /// étiquette (le conseil d'une partie est nommé dans son bloc).
    fn block_counsel(&self, out: &mut CounselMentions) {
        let segs = self.block_segments();
        for &(kind, from, to) in &segs {
            let (names, firms) = match kind {
                Mk::BlockApp => (&mut out.applicant_names, &mut out.applicant_firms),
                Mk::BlockDef => (&mut out.defendant_names, &mut out.defendant_firms),
                _ => continue,
            };
            self.counsel_in(from, to, names, firms);
        }
        // arbitrage de côté par force du signal par mention : une valeur émise des
        // deux côtés est tranchée par l'intro forte (« Rep/assistant : »,
        // « représenté par ») quand un seul côté en porte ; sans signal net, les deux
        // côtés restent.
        let arbitrate = |app: &mut Vec<Mention>, def: &mut Vec<Mention>| {
            let dual: Vec<String> = app
                .iter()
                .filter(|m| def.iter().any(|d| d.value == m.value))
                .map(|m| m.value.clone())
                .collect();
            for v in dual {
                let (mut app_strong, mut def_strong) = (false, false);
                for &(kind, from, to) in &segs {
                    match kind {
                        Mk::BlockApp => app_strong |= self.strong_counsel_mention(&v, from, to),
                        Mk::BlockDef => def_strong |= self.strong_counsel_mention(&v, from, to),
                        _ => {}
                    }
                }
                if app_strong && !def_strong {
                    def.retain(|x| x.value != v);
                } else if def_strong && !app_strong {
                    app.retain(|x| x.value != v);
                }
            }
        };
        arbitrate(&mut out.applicant_names, &mut out.defendant_names);
        arbitrate(&mut out.applicant_firms, &mut out.defendant_firms);
    }

    /// Partie designee par le connecteur « representant » colle a la fin d'une
    /// entree conseil (« Me X, representant la SAS Alpha »). Renvoie la fenetre
    /// pliee qui la nomme, a resoudre contre les partis connus.
    fn partie_representee(&self, apres: usize, to: usize) -> Option<String> {
        let c = self.toks.iter().find(|c| {
            c.kind == Mk::CounselIntro && c.s >= apres && c.s < to && c.s <= apres + 3 && {
                fold_chars(&self.text_slice(c.s, c.e)).starts_with("representant")
            }
        })?;
        Some(fold_chars(&self.text_slice(c.e, to.min(c.e + 120))))
    }

    /// Une mention de `v` dans `[from..to)` porte-t-elle une intro de
    /// conseil forte (« Rep/assistant : », « représenté par »… à ≤ 12 chars
    /// de son ancre « Me »/structure) ?
    fn strong_counsel_mention(&self, v: &str, from: usize, to: usize) -> bool {
        let needle = fold_chars(v);
        if needle.is_empty() {
            return false;
        }
        let hay = self.folded_slice(from, to.min(self.len()));
        let mut off = 0;
        while let Some(p) = hay[off..].find(&needle) {
            let abs = self.source_after(from, off + p);
            let anchor = self
                .toks
                .iter()
                .find(|t| {
                    matches!(t.kind, Mk::Me | Mk::LawStruct | Mk::Form)
                        && t.e <= abs
                        && self.skip_spaces(t.e, abs + 1) == abs
                })
                .map(|t| t.s)
                .unwrap_or(abs);
            let strong = self.toks.iter().any(|c| {
                c.kind == Mk::CounselIntro && c.e <= anchor && c.e + 12 > anchor && {
                    let f = fold_chars(&self.text_slice(c.s, c.e));
                    Self::intro_precedes_counsel(&f)
                }
            });
            if strong {
                return true;
            }
            off += p + needle.len();
        }
        false
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
    fn admin_counsel(&self, out: &mut CounselMentions) {
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
            .map(|a| fold_chars(&a.value))
            .collect();
        let defs_f: Vec<String> = self
            .admin_defendants()
            .iter()
            .map(|d| fold_chars(&d.value))
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
                Mk::Me => self.person_after_me(t.e, end).map(|m| (m, false)),
                _ => self.firm_entry(t, end).map(|m| (m, true)),
            };
            let Some((v, is_firm)) = entry else {
                continue;
            };
            let ne = v.e;
            let win_end = ctoks.get(i + 1).map(|n| n.s).unwrap_or(end);
            let avde = self
                .toks
                .iter()
                .find(|a| a.kind == Mk::AvocatDe && a.s >= ne && a.s < win_end);
            let is_def = match avde {
                Some(a) => {
                    // petit span positionné par le token
                    let party = fold_chars(&self.text_slice(a.e, win_end.min(a.e + 120)));
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
                // « Me X, représentant la SAS Alpha » : l'arête nomme la
                // PARTIE, et le conseil tient son côté d'elle. Elle prime le
                // segment, qui ne sait que sa position dans l'en-tête et avale
                // le rapport d'audience.
                None => match self.partie_representee(v.e, win_end) {
                    Some(party) if side_match(&party, &apps_f, &defs_f) => false,
                    Some(party) if side_match(&party, &defs_f, &apps_f) => true,
                    _ => seg_def(t.s),
                },
            };
            let (names, firms) = if is_def {
                (&mut out.defendant_names, &mut out.defendant_firms)
            } else {
                (&mut out.applicant_names, &mut out.applicant_firms)
            };
            let list = if is_firm { firms } else { names };
            if !list.iter().any(|o| o.value == v.value) {
                list.push(v);
            }
        }
    }

    /// Gabarit CC : conseils des segments demandeurs/défendeurs, puis chaîne des
    /// observations (« les observations de la SCP A, avocat de <partie>, de la SCP
    /// B… ») — le côté suit la partie citée après « avocat de » ; sans
    /// rattachement, la première entrée défend le demandeur (ordre de greffe).
    fn cc_counsel(&self, out: &mut CounselMentions) {
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
        let apps_f: Vec<String> = apps.iter().map(|a| fold_chars(&a.value)).collect();
        let defs_f: Vec<String> = defs.iter().map(|d| fold_chars(&d.value)).collect();
        let mut starts: Vec<usize> = Vec::new();
        let mut entries: Vec<(bool, Mention)> = Vec::new();
        for t in &self.toks {
            if t.s < obs_from || t.s >= end {
                continue;
            }
            let entry = match t.kind {
                Mk::Me => self.person_after_me(t.e, end).map(|m| (false, m)),
                Mk::LawStruct | Mk::Form => self.firm_entry(t, end).map(|m| (true, m)),
                _ => None,
            };
            if let Some(en) = entry {
                starts.push(t.s);
                entries.push(en);
            }
        }
        for (i, (is_firm, m)) in entries.iter().enumerate() {
            let ne = &m.e;
            let win_end = starts.get(i + 1).copied().unwrap_or(end);
            // partie rattachée : « avocat de <partie> » juste après l'entrée
            let side_def = self
                .toks
                .iter()
                .find(|a| a.kind == Mk::AvocatDe && a.s >= *ne && a.s < win_end)
                .and_then(|a| {
                    // petit span positionné par le token : la partie se nomme juste après
                    // « avocat de »
                    let party = fold_chars(&self.text_slice(a.e, win_end.min(a.e + 120)));
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
            if !list.iter().any(|o| o.value == m.value) {
                list.push(m.clone());
            }
        }
    }

    /// Entrées conseil des « Moyen(s) produit(s) par <conseil> » annexés à l'arrêt
    /// CC : (valeur, cabinet ?, fin de la tranche, fin de clause) par marqueur —
    /// la clause court jusqu'au marqueur suivant. Les moyens incidents ou
    /// provoqués sont écartés, soit par l'incise qui casse le marqueur, soit par
    /// la queue de clause (« …, demanderesse au pourvoi incident »).
    fn moyen_par_entries(&self) -> Vec<(Mention, bool, usize)> {
        let mps: Vec<&PTok> = self
            .toks
            .iter()
            .filter(|t| t.kind == Mk::MoyenPar)
            .collect();
        let mut entries: Vec<(Mention, bool, usize)> = Vec::new();
        for (i, t) in mps.iter().enumerate() {
            let clause_end = mps.get(i + 1).map(|n| n.s).unwrap_or(self.len());
            let Some(ct) = self
                .toks
                .iter()
                .find(|c| matches!(c.kind, Mk::LawStruct | Mk::Me) && c.s >= t.e && c.s < t.e + 12)
            else {
                continue;
            };
            let entry = match ct.kind {
                Mk::Me => self.person_after_me(ct.e, self.len()).map(|m| (m, false)),
                _ => self.firm_after_struct(ct, self.len()).map(|m| (m, true)),
            };
            let Some((v, is_firm)) = entry else {
                continue;
            };
            let ne = v.e;
            let tail = fold_chars(&self.text_slice(ne, clause_end.min(ne + 120)));
            if tail.contains("incident") || tail.contains("provoqu") {
                continue;
            }
            entries.push((v, is_firm, clause_end));
        }
        entries
    }

    /// Début du corps du moyen dans la fenêtre pliée `[from..to)` : le corps se
    /// reconnaît à ses ouvertures formulaires (« PREMIER MOYEN DE CASSATION »,
    /// « Il est fait grief… »).
    fn moyen_body_start(&self, from: usize, to: usize) -> usize {
        const OPENERS: [&str; 20] = [
            " premier moyen",
            " second moyen",
            " deuxieme moyen",
            " troisieme moyen",
            " quatrieme moyen",
            " cinquieme moyen",
            " sixieme moyen",
            " moyen unique",
            " moyen de cassation",
            " moyens de cassation",
            " il est fait grief",
            " il est reproche",
            " le moyen ",
            " l'arret ",
            " aux motifs",
            " en ce qu",
            " d'avoir ",
            " contre l'arret",
            " sur le ",
            " sur l'",
        ];
        let win = self.folded_slice(from, to);
        OPENERS
            .iter()
            .filter_map(|n| win.find(n))
            .min()
            .map(|p| self.source_after(from, p))
            .unwrap_or(to)
    }

    /// Fin de phrase après `from` : premier point suivi d'un espace qui
    /// n'est ni une ellipse d'anonymisation (« X... ») ni une abréviation
    /// courte en capitale (« M. », « MM. ») — `to` si aucun.
    fn sentence_end(&self, from: usize, to: usize) -> usize {
        let to = to.min(self.len());
        let mut i = from;
        while i + 1 < to {
            if self.norm.chars[i] == '.' && self.norm.chars[i + 1] == ' ' {
                let ellipsis = i > 0 && self.norm.chars[i - 1] == '.';
                let mut ws = i;
                while ws > from && self.norm.chars[ws - 1].is_alphabetic() {
                    ws -= 1;
                }
                let abbrev = i - ws <= 3
                    && i > ws
                    && self.norm.chars.get(ws).is_some_and(|c| c.is_uppercase());
                if !ellipsis && !abbrev {
                    return i;
                }
            }
            i += 1;
        }
        to
    }

    /// Conseil (avocats + cabinets) des moyens annexés CC : filet demandeur —
    /// un seul conseil distinct exigé (plusieurs = pourvois croisés,
    /// attribution ambiguë).
    fn moyen_par_counsel(&self) -> (Vec<Mention>, Vec<Mention>) {
        let mut names: Vec<Mention> = Vec::new();
        let mut firms: Vec<Mention> = Vec::new();
        for (v, is_firm, _) in self.moyen_par_entries() {
            let list = if is_firm { &mut firms } else { &mut names };
            if !list.iter().any(|o| o.value == v.value) {
                list.push(v);
            }
        }
        if names.len() + firms.len() == 1 {
            (names, firms)
        } else {
            (Vec::new(), Vec::new())
        }
    }

    /// Parties des « Moyen produit par <conseil> … pour <partie> » annexés : filet
    /// demandeur quand le préambule n'en nomme pas, avec la même garde d'unicité
    /// de conseil que [`Self::moyen_par_counsel`].
    fn moyen_par_parties(&self) -> Vec<Mention> {
        let entries = self.moyen_par_entries();
        let distinct = entries
            .iter()
            .map(|(v, _, _)| v.value.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if distinct.len() != 1 {
            return Vec::new();
        }
        let mut out: Vec<Mention> = Vec::new();
        for (v, _, clause_end) in entries {
            let ne = v.e;
            // « , avocat aux Conseils, pour <partie> » : le « pour » vit à
            // portée courte du conseil, dans la même clause
            let lim = clause_end.min(ne + 60).min(self.len());
            let mut from = None;
            for i in ne..lim.saturating_sub(5) {
                if self.folded_slice(i, i + 6) == " pour " {
                    from = Some(i + 6);
                    break;
                }
            }
            let Some(from) = from else {
                continue;
            };
            let to = self.sentence_end(from, clause_end.min(from + 140));
            let to = self.moyen_body_start(from, to);
            for m in self.harvest(from, to, to) {
                if !out.iter().any(|o| same_words(&o.value, &m.value)) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// Récolte les conseils d'un segment `[from..to)` : personnes après
    /// « Me », cabinets via [`Self::firm_entry`].
    fn counsel_in(
        &self,
        from: usize,
        to: usize,
        names: &mut Vec<Mention>,
        firms: &mut Vec<Mention>,
    ) {
        let to = to.min(self.len());
        for t in &self.toks {
            if t.s < from || t.s >= to {
                continue;
            }
            match t.kind {
                Mk::Me => {
                    if let Some(p) = self.person_after_me(t.e, to) {
                        if !names.iter().any(|o| o.value == p.value) {
                            names.push(p);
                        }
                    }
                }
                Mk::LawStruct | Mk::Form => {
                    if let Some(f) = self.firm_entry(t, to) {
                        if !firms.iter().any(|o| o.value == f.value) {
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
    fn firm_entry(&self, t: &PTok, to: usize) -> Option<Mention> {
        let f = self.firm_after_struct(t, to)?;
        let ne = f.e;
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
        ok.then_some(f)
    }

    /// Nom de personne après « Me »/« Maître » : tranche verbatim jusqu'à la
    /// ponctuation ou au prochain token fermant. `None` = « Me » de prose.
    ///
    /// Un nom de personne = mots Capitalisés (+ particules de/du/van…) : la
    /// prose en bas-de-casse qui suit sans ponctuation (« Me B... à assister
    /// et représenter… ») et le groupe parenthésé (cabinet) coupent le nom.
    fn person_after_me(&self, e: usize, to: usize) -> Option<Mention> {
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
            let fw = fold_chars(&w);
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
        self.clean_inner(ns, cut).map(|v| Mention::new(ns, cut, v))
    }

    /// Cabinet après une structure d'avocats (SCP, SELARL…) : les associés
    /// s'enchaînent par virgule (« SCP Célice, Texidor, Périer ») — on
    /// prolonge sur un mot Capitalisé qui n'ouvre pas de token.
    fn firm_after_struct(&self, t: &PTok, to: usize) -> Option<Mention> {
        let ns = self.skip_spaces(t.e, to);
        let first = self.norm.chars.get(ns).copied().unwrap_or(' ');
        if !(first.is_uppercase() || first.is_ascii_digit() || first == '[')
            || self.opens_on_terminator(ns)
        {
            return None;
        }
        let ne = self.chain_end(self.extend_name(ns, to), to);
        let name = self.clean_inner(ns, ne)?;
        let v = format!("{} {}", self.text_slice(t.s, t.e), name);
        Some(Mention::new(t.s, ne, v))
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
        let needle = fold_chars(value);
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
                let f = fold_chars(&self.text_slice(t.s, t.e));
                // « Représentant : la SCP X » (label de greffe, colonisé) introduit le
                // conseil ; « représentant X » de prose introduit la partie
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
                    let f = fold_chars(&self.text_slice(t.s, t.e));
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
                '[' => {
                    g += self.norm.chars[g..to].iter().position(|&c| c == ']')? + 1;
                }
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
                    let f = fold_chars(&self.text_slice(t.s, t.e));
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
            let f = fold_chars(&self.text_slice(t.s, t.e));
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
        // prior de forme : une forme commerciale (SAS, SARL…) émise cabinet sans
        // aucune mention cabinet-forte est une partie
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

    /// With its `s` lost, the banner never reads a cited court's pôle as its own formation.
    #[test]
    fn le_bandeau_sans_marqueur_ne_lit_pas_la_formation_d_une_decision_citee() {
        let doc = "*1DE/06/51/25/41*\n\nCopie : -DGFIP -SAS ALEPH elle-même repré entée par  on \
             pré ident M. [P] [F] -SCP BTSG en la per onne de Me Deni  Ga nier -Parquet\n\n\
             R.G. : 2025108154 P.C. : P202500899\n\nREPUBLIQUE FRANCAISE AU NOM DU PEUPLE \
             FRANCAIS TRIBUNAL DES ACTIVITES ECONOMIQUES DE PARIS\n\nJugement prononcé le \
             mardi 6 janvier 2026 Audience de vacation\n\nREPORT DE LA PERIODE D'OB ERVATION \
             NOMINATION DE JUGE COMMI  AIRE\n\nFAIT  ET PROCEDURE\n\nPar jugement en date du \
             13 mar  2025, le tribunal a ouvert,  ur a  ignation, une procédure de \
             liquidation judiciaire à l'égard de la  A  NIP UX EXPERT . Par arrêt en date du \
             7 octobre 2025, la cour d'appel de Paris Pôle 5 - Chambre 8 a infirmé ledit \
             jugement et,  tatuant à nouveau : a ouvert une procédure de redre  ement.";
        let bandeau = scan(doc).bandeau_text();
        assert!(bandeau.chars().count() <= BANDEAU_MAX);
        assert!(!bandeau.contains("Pôle 5"));
    }

    /// Un avocat nommé deux fois, une seule fois avec son prénom, ne peut pas
    /// se retrouver des deux côtés : la graphie complète porte l'arête.
    #[test]
    fn un_avocat_ne_tient_qu_un_cote() {
        let mut apps = vec![Mention::new(0, 16, "Emmanuel Feuilly".into())];
        let mut defs = vec![Mention::new(90, 97, "Feuilly".into())];
        un_seul_cote(&mut apps, &mut defs);
        assert_eq!(apps.len(), 1);
        assert!(defs.is_empty());

        // la graphie complète est en défense : c'est elle qui reste
        let mut apps = vec![Mention::new(0, 7, "Brumaire".into())];
        let mut defs = vec![Mention::new(90, 106, "SELARL Brumaire".into())];
        un_seul_cote(&mut apps, &mut defs);
        assert!(apps.is_empty());
        assert_eq!(defs.len(), 1);

        // deux avocats distincts restent chacun de son côté
        let mut apps = vec![Mention::new(0, 6, "Babet".into())];
        let mut defs = vec![Mention::new(90, 97, "Bougon".into())];
        un_seul_cote(&mut apps, &mut defs);
        assert_eq!((apps.len(), defs.len()), (1, 1));
    }

    /// Grammaire du pseudonyme : les trois ères de pseudonymisation, la
    /// civilité hors valeur, la mention matrimoniale terminatrice, et la
    /// résolution de deux graphies sur la plus longue.
    #[test]
    fn lecteur_de_partie_personne_physique() {
        let cas: [(&str, &[&str], &[&str]); 6] = [
            (
                "Vu la procedure suivante : Par une requete, M. A B, representé par Me Lemaire, demande a la cour d'annuler l'arreté.",
                &["A B"],
                &[],
            ),
            (
                "Statuant sur le pourvoi formé par : X... Jacques, partie civile, contre l'arret rendu par la cour d'appel de Paris.",
                &["X... Jacques"],
                &[],
            ),
            (
                "APPELANT : Monsieur Vincent X... INTIMÉE : Madame Régine Y..., épouse Z...",
                &["Vincent X..."],
                &["Régine Y..."],
            ),
            (
                "APPELANTS : [CP] [F] né le 3 mars 1970 INTIMÉ : [YS] [WC] [R] [I] demeurant a Paris",
                &["[CP] [F]"],
                &["[YS] [WC] [R] [I]"],
            ),
            (
                "APPELANTS : Monsieur Pierre X... Madame Danièle X... INTIMÉE : SA Verrerie du Nord",
                &["Pierre X...", "Danièle X..."],
                &[],
            ),
            (
                "Vu la procedure suivante : Procedure contentieuse anterieure : M. A B a demandé au tribunal administratif de Strasbourg d'annuler l'arreté. Procedure devant la cour : Par une requete, M. B, representé par Me Lemaire, demande a la cour.",
                &["A B"],
                &[],
            ),
        ];
        for (texte, apps, defs) in cas {
            let s = scan(texte);
            let (a, d) = s.individuals_uncached();
            let lu = |v: &[Mention]| v.iter().map(|m| m.value.clone()).collect::<Vec<_>>();
            assert_eq!(lu(&a), apps, "demandeurs de « {texte} »");
            assert_eq!(lu(&d), defs, "défendeurs de « {texte} »");
        }
    }

    /// Contrat de [`Mention`] : la tranche `[s..e)` porte tous les mots de la
    /// valeur, dans l'ordre. Elle n'en est pas la copie (forme sociale
    /// recollée, intérieur nettoyé, queue rognée), elle l'ancre.
    #[test]
    fn mention_span_carries_its_value() {
        let textes = [
            "Vu la procédure suivante : Par une requête enregistrée le 3 mars 2023, la SAS Alpha Distribution, représentée par Me Grantaire, demande à la cour : 1°) d'annuler l'arrêté du maire de Saint-Priest ;",
            "APPELANTE : SAS Gondrand, représentée par Me Laura Javert, avocat au barreau de MÂCON INTIMÉE : SA Verrerie du Nord, représentée par la SELARL Brumaire",
            "Sur le pourvoi formé par la société anonyme Crédit Lyonnais, contre l'arrêt rendu par la cour d'appel de Paris, dans le litige l'opposant à la CPAM du Gard, défenderesse à la cassation ;",
        ];
        for texte in textes {
            let s = scan(texte);
            let r = s.party_registry();
            assert!(!r.entities.is_empty(), "aucune entité : {texte}");
            for e in &r.entities {
                let tranche = fold_chars(&s.text_slice(e.s, e.e));
                let mut reste = tranche.as_str();
                for mot in fold_chars(&e.value).split_whitespace() {
                    let Some(i) = reste.find(mot) else {
                        panic!(
                            "« {mot} » de « {} » hors de sa tranche « {tranche} »",
                            e.value
                        );
                    };
                    reste = &reste[i + mot.len()..];
                }
            }
        }
    }

    #[test]
    fn composite_stop_recasts_to_audience_in_prose() {
        let w = scan("L'affaire a été débattue le 4 septembre 2012, en chambre du conseil")
            .audience_windows();
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("4 septembre 2012"));
    }

    #[test]
    fn prefix_closure_marker_survives_garbled_alias() {
        let sig = scan("Vu : - le code de l'entrée et du séjour des étrangers et du droit d'asile ; - le code de justice administrative.")
            .procedure_signals();
        assert!(sig.retention);
    }

    #[test]
    fn admin_single_request_header() {
        let text = "Vu la procédure suivante : Par une requête et un mémoire, enregistrés les 2 février 2021 et 15 juillet 2021, la SAS Gaillot Distribution, représentée par Me Grantaire, demande à la cour : 1°) d'annuler l'arrêté du 22 décembre 2020 du maire de Saint-Priest refusant de lui délivrer un permis de construire ;";
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
    fn zones_cc_jonction_multi_pourvois() {
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
        let text = "Vu la procédure suivante : Par une requête enregistrée le 3 mars 2023, la SAS Alpha, représentée par Me Grantaire, demande à la cour d'annuler l'arrêté du maire. Par un mémoire en défense, enregistré le 4 avril 2023, la commune de Machilly, représentée par Me Prouvaire, conclut au rejet de la requête. Considérant ce qui suit : 1. La requête est rejetée.";
        let out = scan(text).counsel();
        assert_eq!(out.applicant_names, vec!["Grantaire".to_string()]);
        assert_eq!(out.defendant_names, vec!["Prouvaire".to_string()]);
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
    fn counsel_cc_moyen_annexe_fallback_me() {
        let text = "LA COUR DE CASSATION, PREMIÈRE CHAMBRE CIVILE, a rendu l'arrêt suivant : Sur le moyen unique : Attendu que la clinique a recherché la responsabilité des médecins ; PAR CES MOTIFS : CASSE ET ANNULE l'arrêt rendu le 10 octobre 2007 ; MOYEN ANNEXE au présent arrêt.\nMoyen produit par Me Le Prado, avocat aux Conseils pour la société Centre médico-chirurgical de l'Atlantique.\nLE MOYEN reproche à l'arrêt attaqué.";
        let s = scan(text);
        let out = s.counsel();
        assert_eq!(out.applicant_names, vec!["Le Prado".to_string()]);
        assert!(out.applicant_firms.is_empty());
        let (apps, _) = s.companies();
        assert_eq!(
            apps,
            vec!["Centre médico-chirurgical de l'Atlantique".to_string()]
        );
    }

    #[test]
    fn counsel_cc_moyen_annexe_incident_excluded() {
        let text = "LA COUR DE CASSATION, CHAMBRE SOCIALE, a rendu l'arrêt suivant : Attendu que M. X... a saisi la juridiction prud'homale ; PAR CES MOTIFS : REJETTE les pourvois ; MOYENS ANNEXES au présent arrêt.\nMoyens produits par la SCP Piwnica et Molinié, avocat aux Conseils pour M. X..., demandeur au pourvoi principal\nMoyen produit par Me Spinosi, avocat aux Conseils pour la société Planète services, demanderesse au pourvoi incident\nIl est fait grief à l'arrêt attaqué.";
        let s = scan(text);
        let out = s.counsel();
        assert_eq!(
            out.applicant_firms,
            vec!["SCP Piwnica et Molinié".to_string()]
        );
        assert!(out.applicant_names.is_empty());
        let (apps, _) = s.companies();
        assert!(apps.is_empty());
    }

    #[test]
    fn counsel_side_arbitrage_cartouche_vs_intro_forte() {
        let text = "TRIBUNAL JUDICIAIRE DE [Localité 8] ORDONNANCE DE RÉFÉRÉ affaire : [H] [J] c/ [T] [V], S.C.I. MARIE Expédition délivrée à Me Eric MARY Me Michel MONTAGARD le l'an deux mil vingt cinq, avons rendu l'ordonnance suivante : A la requête de : Monsieur [H] [J] [Adresse 4] Rep/assistant : Me Michel MONTAGARD, avocat au barreau de NICE DEMANDEUR Contre : Monsieur [T] [V] [Adresse 6] Rep/assistant : Me Eric MARY, avocat au barreau de NICE S.C.I. MARIE [Adresse 5] Rep/assistant : Me Eric MARY, avocat au barreau de NICE DÉFENDEURS EXPOSE DU LITIGE La juridiction statue.";
        let s = scan(text);
        let out = s.counsel();
        assert_eq!(out.applicant_names, vec!["Michel MONTAGARD".to_string()]);
        assert_eq!(out.defendant_names, vec!["Eric MARY".to_string()]);
        let (apps, defs) = s.companies();
        assert!(apps.is_empty());
        assert_eq!(defs, vec!["S.C.I. MARIE".to_string()]);
    }

    #[test]
    fn quality_arbitrage_form_after_counsel_line_is_party_not_firm() {
        let text = "COUR D'APPEL DE PARIS ARRET DU 3 MAI 2022 APPELANT : M. X Représentant : Me A, avocat au barreau de LYON INTIMEES : SA Enedis [Adresse 4] Représentant : la SCP Vrai Conseil, avocat au barreau de PARIS SA Grdf [Adresse 3] défaillante COMPOSITION DE LA COUR : M. B, président EXPOSE DU LITIGE La cour statue.";
        let s = scan(text);
        let out = s.counsel();
        assert_eq!(out.defendant_firms, vec!["SCP Vrai Conseil".to_string()]);
        let (_, defs) = s.companies();
        assert!(defs.iter().any(|d| d.starts_with("SA Enedis")));
    }

    #[test]
    fn quality_arbitrage_representant_precedes_party() {
        let text = "Vu la procédure suivante : Par une requête enregistrée le 3 mars 2023, la caisse locale des assurances mutuelles agricoles de Paris demande à la cour d'annuler l'arrêté. Ont été entendues les observations de Me Joly, représentant la caisse locale des assurances mutuelles agricoles de Paris. Considérant ce qui suit : 1. La requête est rejetée.";
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
        let sc = scan("Par une intervention enregistrée le 25 août 2025, la fédération des chasseurs et le club international des chasseurs de bécassines, représentés par Me Lesgle, concluent au rejet.");
        assert_eq!(
            sc.intervenors(),
            vec![
                "fédération des chasseurs".to_string(),
                "club international des chasseurs de bécassines".to_string(),
            ]
        );
        let sc = scan("Par une requête intitulée \" mémoire en intervention volontaire \" enregistrée le 17 juin, la société Veolia Eau a fait valoir son intérêt.");
        assert!(sc.intervenors().is_empty());
    }
}

#[cfg(test)]
mod ecole_solution_formation {
    use super::scan;

    #[test]
    fn solution_irrecevabilite_prononcee() {
        let sc = scan("Dès lors, la requête de M. A est manifestement irrecevable et doit être rejetée, en application de l'article R. 222-1 du code de justice administrative.\nO R D O N N E :\nArticle 1er : La requête de M. A est rejetée.");
        assert_eq!(sc.outcome(), Some(("IRRECEVABILITE", false)));
        let sc = scan("Sa requête est manifestement irrecevable. Par suite, il y a lieu de rejeter ses conclusions.\nO R D O N N E :\nArticle 1er : La requête est rejetée.");
        assert_eq!(sc.outcome(), Some(("IRRECEVABILITE", false)));
        let sc = scan("La créance dont se prévaut le requérant étant prescrite, ses conclusions indemnitaires ne peuvent donc qu'être rejetées.\nO R D O N N E :\nArticle 1er : La requête est rejetée.");
        assert_eq!(sc.outcome(), Some(("REJET", false)));
    }

    #[test]
    fn old_admin_operative_part_opens_on_article_one() {
        let sc = scan("Vu la requête enregistrée le 18 février 1985 au secrétariat du Contentieux du Conseil d'Etat, présentée par M. X..., demeurant ... ; Considérant que M. X... n'apporte aucun élément de nature à remettre en cause la solution retenue par les premiers juges ; que, dès lors, il y a lieu de rejeter sa requête ; Article 1er : La requête de M. X... est rejetée. Article 2 : La présente décision sera notifiée à M. X... et au ministre de l'intérieur.");
        assert_eq!(sc.outcome(), Some(("REJET", false)));
        let sc = scan("Vu la requête enregistrée le 6 janvier 1986 au secrétariat du Contentieux du Conseil d'Etat, présentée par le ministre de l'intérieur ; Considérant qu'il ressort des pièces du dossier que M. X... est entré en France en 1982 sous couvert d'un visa de court séjour et s'y est maintenu après l'expiration de ce visa ; qu'il n'établit ni l'ancienneté de sa résidence, ni l'intensité de ses liens personnels et familiaux en France ; que, dès lors, en prenant l'arrêté attaqué, le préfet n'a pas commis d'erreur manifeste dans l'appréciation de ses conséquences sur la situation de l'intéressé ; Considérant qu'il résulte de ce qui précède que c'est à tort que le tribunal administratif a annulé l'arrêté du préfet ; Article 1er : Le jugement du tribunal administratif de Cayenne en date du 6 novembre 1985 est annulé. Article 2 : La demande présentée par M. X... devant le tribunal administratif de Cayenne est rejetée. Article 3 : La présente décision sera notifiée à M. X... et au ministre de l'intérieur.");
        assert_eq!(sc.outcome(), Some(("ANNULATION", false)));
    }

    #[test]
    fn old_admin_operative_part_reads_its_hyphen_and_its_numbering() {
        let sc = scan("Vu la requête enregistrée le 12 mars 1987 au greffe de la cour, présentée pour M. X..., demeurant à Rouen ; Considérant qu'il ressort des pièces du dossier que M. X... a été assujetti à des cotisations supplémentaires d'impôt sur le revenu au titre des années 1980 et 1981 ; que l'administration a régulièrement suivi la procédure de redressement contradictoire ; Considérant qu'il résulte de ce qui précède que M. X... n'est pas fondé à soutenir que c'est à tort que le tribunal administratif a rejeté sa demande ;\nArticle 1er - La requête de M. X... est rejetée.\nArticle 2 - Le présent arrêt sera notifié à M. X... et au ministre délégué au budget.");
        assert_eq!(sc.outcome(), Some(("REJET", false)));
        let sc = scan("Vu la requête enregistrée le 12 mars 1987 au greffe de la cour, présentée pour M. X..., demeurant à Rouen ; Considérant qu'il résulte de l'instruction que l'accident dont M. X... a été victime le 3 juin 1982 sur la voie publique est imputable au défaut d'entretien normal de l'ouvrage ; que l'Etat n'établit pas que la victime aurait commis une imprudence de nature à atténuer sa responsabilité ; Considérant que M. X... a subi une incapacité temporaire totale de six mois et conserve une incapacité permanente partielle de 15 % ; qu'il a supporté des souffrances physiques évaluées à 3 sur une échelle de 7 ; qu'il sera fait une juste appréciation de l'ensemble de ses préjudices en les fixant à 50 000 F ; Considérant qu'il résulte de ce qui précède que M. X... est fondé à demander que la somme que le tribunal administratif lui a allouée soit portée à ce montant ;\nArticle 1er : La somme que l'Etat a été condamné à verser à M. X... par le jugement attaqué est portée à 50 000 F.\nArticle 2 : Le jugement du tribunal administratif de Rouen est réformé en ce qu'il a de contraire à la présente décision.\nArticle 3 : La présente décision sera notifiée à M. X... et au ministre de l'intérieur.");
        assert_eq!(sc.outcome(), Some(("REFORMATION", false)));
    }

    #[test]
    fn a_quoted_decree_numbered_up_to_its_notification_does_not_open_the_operative_part() {
        let sc = scan("Vu la requête enregistrée le 12 mars 1987, présentée pour M. X... ; Considérant que M. X... soutient que l'arrêté du préfet est illégal ; que cet arrêté dispose :\nArticle 1er : Les tarifs de stationnement sont fixés à 5 F.\nArticle 2 : Le présent arrêté sera notifié aux intéressés ;\nConsidérant qu'il ressort des pièces du dossier que les tarifs ont été régulièrement fixés ;");
        assert_eq!(sc.dispositif_start(), sc.text_len());
    }

    #[test]
    fn old_admin_operative_part_sends_the_case_elsewhere_or_does_not_admit_it() {
        let sc = scan("Vu la requête enregistrée le 12 mars 1987 au greffe de la cour, présentée pour M. X..., demeurant à Rouen ; Considérant qu'il ressort des pièces du dossier que M. X... a été assujetti à des cotisations supplémentaires d'impôt sur le revenu au titre des années 1980 et 1981 ; que l'administration a régulièrement suivi la procédure de redressement contradictoire ; Considérant qu'il résulte de ce qui précède que M. X... n'est pas fondé à soutenir que c'est à tort que le tribunal administratif a rejeté sa demande ;\nArticle 1er : Le jugement de la requête de M. X... est attribué à la cour administrative d'appel de Nancy.\nArticle 2 : La présente décision sera notifiée à M. X... et au ministre de l'intérieur.");
        assert_eq!(sc.outcome(), Some(("AUTRE", false)));
        let sc = scan("Vu la requête enregistrée le 12 mars 1987 au greffe de la cour, présentée pour M. X..., demeurant à Rouen ; Considérant qu'il ressort des pièces du dossier que M. X... a été assujetti à des cotisations supplémentaires d'impôt sur le revenu au titre des années 1980 et 1981 ; que l'administration a régulièrement suivi la procédure de redressement contradictoire ; Considérant qu'il résulte de ce qui précède que M. X... n'est pas fondé à soutenir que c'est à tort que le tribunal administratif a rejeté sa demande ;\nArticle 1er : Il est sursis à statuer sur la requête de M. X... jusqu'à ce que la Cour de justice se soit prononcée.\nArticle 2 : La présente décision sera notifiée à M. X... et au ministre de l'intérieur.");
        assert_eq!(sc.outcome(), Some(("AUTRE", false)));
        let sc = scan("Vu la requête enregistrée le 12 mars 1987 au greffe de la cour, présentée pour M. X..., demeurant à Rouen ; Considérant qu'il ressort des pièces du dossier que M. X... a été assujetti à des cotisations supplémentaires d'impôt sur le revenu au titre des années 1980 et 1981 ; que l'administration a régulièrement suivi la procédure de redressement contradictoire ; Considérant qu'il résulte de ce qui précède que M. X... n'est pas fondé à soutenir que c'est à tort que le tribunal administratif a rejeté sa demande ;\nArticle 1er : La requête de M. Y... n'est pas admise.\nArticle 2 : La présente décision sera notifiée à M. Y... et au ministre de l'économie.");
        assert_eq!(sc.outcome(), Some(("NON_ADMISSION", false)));
        let sc = scan("Vu la requête enregistrée le 12 mars 1987 au greffe de la cour, présentée pour M. X..., demeurant à Rouen ; Considérant qu'il ressort des pièces du dossier que M. X... a été assujetti à des cotisations supplémentaires d'impôt sur le revenu au titre des années 1980 et 1981 ; que l'administration a régulièrement suivi la procédure de redressement contradictoire ; Considérant qu'il résulte de ce qui précède que M. X... n'est pas fondé à soutenir que c'est à tort que le tribunal administratif a rejeté sa demande ;\nArticle 1er : Il n'y pas lieu de statuer sur la requête de M. X....\nArticle 2 : La présente décision sera notifiée à M. X... et au ministre de l'intérieur.");
        assert_eq!(sc.outcome(), Some(("NON_LIEU_A_STATUER", false)));
    }

    #[test]
    fn a_quoted_article_one_does_not_open_the_operative_part() {
        let sc = scan("Vu la requête enregistrée le 18 février 1985 au secrétariat du Contentieux du Conseil d'Etat, présentée par M. X... ; Considérant que le décret du 5 mai 1980 dispose : « Article 1er : La décision de retrait est rejetée par l'autorité de tutelle » ; qu'aux termes de l'article 1er : La requête du maire est transmise au préfet ; que M. X... soutient que ce décret a été méconnu");
        assert_eq!(sc.dispositif_start(), sc.text_len());
        assert_eq!(sc.outcome(), None);
    }

    #[test]
    fn jref_demande_et_conseil_header() {
        let sc = scan("Par une requête, enregistrée le 4 mai 2022, Mme A demande au juge des référés, statuant en application de l'article L. 521-3 du code de justice administrative, la suspension de la décision.\nConsidérant ce qui suit : 1. La requête est rejetée.");
        assert!(sc.procedure_signals().jref_demande);
        let sc = scan("M. B demande au juge des référés du Conseil d'Etat, statuant sur le fondement de l'article L. 521-2 du code de justice administrative, l'annulation de l'ordonnance.\nConsidérant ce qui suit : 1. La requête est rejetée.");
        assert!(sc.procedure_signals().jref_conseil);
    }
}
