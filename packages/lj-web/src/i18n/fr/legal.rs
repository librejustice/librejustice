use crate::i18n::Block::{Address, List, Para};
use crate::i18n::Run::{Link, Local, Strong, Text};
use crate::i18n::{LegalPage, LegalSection, LegalUi};

pub const LEGAL: LegalUi = LegalUi {
    terms: TERMS,
    privacy: PRIVACY,
    notice: NOTICE,
};

const TERMS: LegalPage = LegalPage {
    title: "Conditions d'utilisation - LibreJustice",
    heading: "Conditions d'utilisation",
    in_force: "En vigueur au 6 novembre 2026",
    sections: &[
        LegalSection {
            title: "Objet",
            blocks: &[
                Para(&[
                    Text("Ces conditions régissent l'utilisation de "),
                    Strong("librejustice.fr"),
                    Text(", de son connecteur MCP et de son API (ensemble, « le service »), édités par Iris Sainte Fare Garnot (entrepreneur individuel, SIRET 922 108 600 00029). Elles sont acceptées lors de la création d'un compte et s'appliquent à toute utilisation du service. Elles sont régies par le droit français. Contact : "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Accès",
            blocks: &[
                Para(&[
                    Text("Le service est gratuit et ouvert à tout usage, y compris commercial. La recherche sur le site ne demande pas de compte. Le connecteur MCP et l'API en demandent un. L'API s'utilise avec une clé personnelle, créée depuis la "),
                    Local("/profile", "page profil"),
                    Text(". Le titulaire d'un compte répond de l'usage qui en est fait. Il peut le supprimer à tout moment."),
                ]),
            ],
        },
        LegalSection {
            title: "Usage",
            blocks: &[
                Para(&[
                    Text("Le service permet de rechercher et de consulter des décisions de justice, des textes, des fiches d'entités et des annonces de marchés publics. Il n'a pas vocation à permettre la reproduction de ces contenus dans leur ensemble. Chaque compte dispose d'un volume de requêtes, commun au site, au connecteur MCP et à l'API, afin que la capacité du service reste partagée entre tous les utilisateurs. Ce volume ne peut être contourné. Pour un besoin plus important, écrivez-nous. Il est interdit de perturber le service, d'accéder à des données ou à des comptes qui ne vous sont pas destinés ou d'en éprouver la sécurité sans autorisation."),
                ]),
                Para(&[
                    Text("Il est aussi interdit d'utiliser le service pour :"),
                ]),
                List(&[
                    &[
                        Text("identifier une personne que la source des données a occultée ou soustraite à la diffusion ;"),
                    ],
                    &[
                        Text("dresser des listes de personnes ou d'associations selon l'origine, la nationalité, la religion, l'orientation sexuelle, l'état de santé ou l'appartenance syndicale, réels ou supposés, de ces personnes, des membres de ces associations ou de ceux qu'elles défendent ;"),
                    ],
                    &[
                        Text("écarter quelqu'un d'un logement ou d'un emploi en raison des procédures qui le concernent ;"),
                    ],
                    &[
                        Text("rassembler de quoi retrouver des personnes, des associations, des syndicats ou des lieux de culte, afin de les exposer à des violences ou à des dégradations ;"),
                    ],
                    &[
                        Text("évaluer, analyser, comparer ou prédire les pratiques professionnelles des magistrats et des membres du greffe à partir de leur identité, ce que punissent les articles "),
                        Local("/norm/fr/code-de-l-organisation-judiciaire/l111-13", "L. 111-13 du code de l'organisation judiciaire"),
                        Text(" et "),
                        Local("/norm/fr/code-de-justice-administrative/l10", "L. 10 du code de justice administrative"),
                        Text("."),
                    ],
                ]),
            ],
        },
        LegalSection {
            title: "Décisions versées",
            blocks: &[
                Para(&[
                    Text("En versant une décision, vous attestez l'avoir anonymisée et avoir le droit de la partager, et vous acceptez qu'elle soit publiée et réutilisée librement. L'éditeur peut la refuser."),
                ]),
            ],
        },
        LegalSection {
            title: "Réutilisation",
            blocks: &[
                Para(&[
                    Text("Un service qui s'appuie sur LibreJustice pour fournir des résultats à ses propres utilisateurs, par l'API, le connecteur MCP ou tout autre moyen, indique cette provenance de manière visible là où ces résultats apparaissent. S'il fait payer l'accès à ces résultats, il indique aussi qu'ils sont consultables gratuitement sur librejustice.fr. L'utilisateur est seul responsable de l'usage des données obtenues et de ce qu'il en présente à des tiers. Les contenus restent soumis à la licence de leur source, indiquée sur la page "),
                    Local("/sources", "Données & sources"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Responsabilité",
            blocks: &[
                Para(&[
                    Text("Le service est fourni en l'état. L'éditeur ne garantit ni l'exactitude, ni l'exhaustivité, ni la disponibilité des données et du service. Les informations servies ne constituent pas un conseil juridique. Les données personnelles sont traitées dans les conditions de la "),
                    Local("/privacy", "politique de confidentialité"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Évolution et suspension",
            blocks: &[
                Para(&[
                    Text("L'éditeur peut modifier, limiter, suspendre ou arrêter tout ou partie du service à tout moment et sans préavis. Il peut de même suspendre ou supprimer, sans préavis ni indemnité, un compte ou une clé dont l'usage méconnaît ces conditions ou perturbe le service. Son titulaire en reçoit le motif et peut y répondre. Ces conditions peuvent être modifiées. Celles qui s'appliquent sont celles en ligne au moment de l'utilisation."),
                ]),
            ],
        },
    ],
};

const PRIVACY: LegalPage = LegalPage {
    title: "Confidentialité - LibreJustice",
    heading: "Politique de confidentialité",
    in_force: "En vigueur au 8 novembre 2026",
    sections: &[
        LegalSection {
            title: "Responsable de traitement",
            blocks: &[
                Para(&[
                    Text("Iris Sainte Fare Garnot (entrepreneur individuel, SIRET 922 108 600 00029), éditeur de "),
                    Strong("librejustice.fr"),
                    Text(". Contact : "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Données collectées",
            blocks: &[
                Para(&[
                    Strong("Compte utilisateur."),
                    Text(" Votre adresse e-mail, votre nom si vous l'avez indiqué, et votre identifiant Google ou Microsoft si vous vous connectez par l'un d'eux. Nous notons aussi la langue de votre navigateur, votre pays, la date de votre inscription et celle de votre dernière visite. Si vous connectez un assistant IA à votre compte, nous lui transmettons votre adresse e-mail. Nous gardons aussi la trace des e-mails que nous vous envoyons et de votre éventuel désabonnement. Tout est effacé quand vous supprimez votre compte."),
                ]),
                Para(&[
                    Strong("Journaux techniques."),
                    Text(" Le fonctionnement du service repose sur des journaux applicatifs et des traces de performance, qui servent à mesurer les temps de réponse, à diagnostiquer les erreurs, à suivre l'utilisation du service et à repérer les usages interdits par les conditions d'utilisation. Ils peuvent contenir les paramètres des requêtes, dont les termes de recherche, ainsi que le type de client utilisé (navigateur, API ou assistant IA). Ces données sont pseudonymisées : aucun nom ni adresse e-mail n'y figure, l'utilisateur n'y apparaît que sous un identifiant technique dérivé. Elles sont supprimées automatiquement au bout de 30 jours."),
                ]),
                Para(&[
                    Strong("Mesure d'audience."),
                    Text(" Pour suivre l'utilisation du service et orienter ses évolutions, nous produisons des indicateurs de fréquentation consolidés à la journée : le volume des recherches et des consultations, les pages les plus consultées, les fonctionnalités et les filtres utilisés, le site d'où arrivent les visiteurs et la répartition par canal d'accès. Ces indicateurs se limitent à des décomptes : ils ne conservent ni les termes recherchés, ni les pages lues par une personne. Ils ne font appel à aucun cookie, et aucune adresse IP n'est enregistrée."),
                ]),
                Para(&[
                    Strong("Décisions versées."),
                    Text(" Le fichier que vous versez et votre compte. Le fichier est effacé une fois relu. Si la décision est publiée, votre compte reste attaché à sa provenance et n'est jamais affiché."),
                ]),
                Para(&[
                    Strong("Signalement d'un problème."),
                    Text(" Le texte de votre message, l'adresse de la page concernée (y compris les termes de recherche lorsqu'elle en porte), le fait que vous étiez connecté ou non, et votre adresse e-mail si vous la donnez ou si vous êtes connecté. Ces éléments arrivent dans notre boîte de support, où ils sont conservés pour le suivi de votre demande."),
                ]),
            ],
        },
        LegalSection {
            title: "Cookies et stockage local",
            blocks: &[
                Para(&[
                    Text("Nous n'utilisons aucun cookie publicitaire ni aucun traceur soumis à consentement. Notre mesure d'audience est configurée pour respecter votre vie privée, conformément aux recommandations de la CNIL : c'est pourquoi ce site n'affiche pas de bandeau. Si vous êtes connecté, votre navigateur garde un jeton de session qui s'efface à la déconnexion. Un cookie retient la juridiction et la langue que vous choisissez. L'onglet garde aussi le contexte de votre recherche jusqu'à sa fermeture, pour revenir aux résultats et surligner les termes cherchés dans une décision."),
                ]),
            ],
        },
        LegalSection {
            title: "Sous-traitants",
            blocks: &[
                List(&[
                    &[
                        Strong("OVH"),
                        Text(" (Roubaix) : hébergement, base de données, télémétrie, messagerie."),
                    ],
                    &[
                        Strong("Cloudflare Lda."),
                        Text(" (Lisbonne) : CDN, protection, mesure d'audience et analyse sémantique des recherches. Ses "),
                        Link("https://developers.cloudflare.com/workers-ai/platform/data-usage/", "conditions d'utilisation des données"),
                        Text(" excluent l'entraînement de ses modèles sur ces recherches."),
                    ],
                    &[
                        Strong("Grafana Cloud"),
                        Text(" (New York, données à Francfort) : logs, traces et mesure d'audience."),
                    ],
                    &[
                        Strong("Mistral AI"),
                        Text(" (Paris) : résumés des décisions et reclassement des résultats de recherche. L'entraînement de ses modèles sur ce que nous lui envoyons est désactivé, et sa "),
                        Link("https://legal.mistral.ai/terms/privacy-policy/", "politique de confidentialité"),
                        Text(" fixe la durée de conservation."),
                    ],
                    &[
                        Strong("Resend"),
                        Text(" (San Francisco, envoi via AWS Dublin) : e-mails de service (confirmation, réinitialisation) et e-mails d'information."),
                    ],
                ]),
            ],
        },
        LegalSection {
            title: "Personnes citées dans les données publiées",
            blocks: &[
                Para(&[
                    Text("Les données servies viennent de publications officielles et de registres publics. Les décisions sont reprises telles que leur source les publie, avec l'occultation qu'elle y a appliquée."),
                ]),
                Para(&[
                    Text("Une décision versée par un utilisateur a été anonymisée par lui avant sa publication."),
                ]),
                Para(&[
                    Text("Pour faire occulter ou corriger une donnée qui vous concerne, adressez-vous à la juridiction ou au registre qui la publie. Si elle figure dans une décision versée par un utilisateur, écrivez-nous à "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Vos droits",
            blocks: &[
                Para(&[
                    Text("Vous disposez d'un droit d'accès, de rectification, d'effacement, de limitation, d'opposition et de portabilité (RGPD, loi Informatique et Libertés)."),
                ]),
                Para(&[
                    Text("Le bouton « Supprimer mon compte » sur votre "),
                    Local("/profile", "page profil"),
                    Text(" efface immédiatement votre compte."),
                ]),
                Para(&[
                    Text("Pour toute autre demande, écrivez à "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text(" ou saisissez la "),
                    Link("https://www.cnil.fr/fr/plaintes", "CNIL"),
                    Text("."),
                ]),
            ],
        },
    ],
};

const NOTICE: LegalPage = LegalPage {
    title: "Mentions légales - LibreJustice",
    heading: "Mentions légales",
    in_force: "En vigueur au 15 mai 2026",
    sections: &[
        LegalSection {
            title: "Éditeur du site",
            blocks: &[
                Address(&[
                    &[
                        Strong("Iris Sainte Fare Garnot"),
                        Text(", entrepreneur individuel"),
                    ],
                    &[
                        Text("également directeur de la publication"),
                    ],
                    &[
                        Text("SIRET : 922 108 600 00029"),
                    ],
                    &[
                        Text("Courriel : "),
                        Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    ],
                ]),
            ],
        },
        LegalSection {
            title: "Hébergement",
            blocks: &[
                Para(&[
                    Text("Site, moteur de recherche, API et base de données : "),
                    Strong("OVH SAS"),
                    Text(" (Roubaix)."),
                ]),
                Para(&[
                    Text("Réseau de diffusion (CDN) et protection : "),
                    Strong("Cloudflare Lda."),
                    Text(" (Lisbonne)."),
                ]),
            ],
        },
        LegalSection {
            title: "Données publiées",
            blocks: &[
                Para(&[
                    Text("Les données diffusées par LibreJustice proviennent de sources publiques et de sources dont la reprise est autorisée. Chacune est présentée, avec ses conditions de réutilisation, sur la page "),
                    Local("/sources", "Données & sources"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Propriété intellectuelle",
            blocks: &[
                Para(&[
                    Text("Le "),
                    Link("https://github.com/librejustice/librejustice", "code du site"),
                    Text(" est publié sous licence Apache 2.0. Les composants de tiers sur lesquels il s'appuie, bibliothèques, polices et outils de lecture des documents, restent sous "),
                    Local("/open-source", "leurs propres licences"),
                    Text("."),
                ]),
            ],
        },
    ],
};
