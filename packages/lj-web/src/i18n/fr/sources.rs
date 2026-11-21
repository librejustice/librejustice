use crate::i18n::Run::{Link, Local, Text};
use crate::i18n::{SourceCopy, SourcesUi};

const LICENCE_OUVERTE: &str = "https://www.etalab.gouv.fr/licence-ouverte-open-licence/";

pub const SOURCES: SourcesUi = SourcesUi {
    title: "Données & sources - LibreJustice",
    heading: "Données & sources",
    intro: "LibreJustice agrège des données juridiques publiques : décisions de justice, textes (codes, lois, traités, droit de l'Union), avis de marchés publics et registres. Chaque source est présentée ci-dessous avec son origine et ses conditions de réutilisation. LibreJustice n'en modifie pas le contenu et ne garantit ni l'exhaustivité ni l'actualité du corpus.",
    reuse: "Conditions de réutilisation : ",
    case_law: "Jurisprudence",
    norms: "Normes",
    commentaries: "Commentaires de décisions",
    procurement: "Marchés publics",
    registers: "Entreprises, associations et avocats",
    judilibre: SourceCopy {
        title: "Judilibre, Cour de cassation",
        body: &[
            &[
                Text("Décisions de l'ordre judiciaire (Cour de cassation, cours d'appel, tribunaux judiciaires et de commerce, conseils de prud'hommes), diffusées en open data par la Cour de cassation via l'API Judilibre, pseudonymisées avant publication. LibreJustice en ingère l'intégralité du flux et le resynchronise quotidiennement. L'open data judiciaire s'élargit progressivement : le pénal de première instance est attendu en 2027 ("),
                Link("https://www.justice.gouv.fr/documentation/open-data-decisions-justice", "calendrier d'ouverture"),
                Text(")."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text(", complétée par les "),
            Link("https://www.courdecassation.fr/conditions-generales-dutilisation-pour-la-reutilisation-des-donnees-issues-des-decisions-de-justice", "conditions générales d'utilisation"),
            Text(" de la Cour de cassation."),
        ],
    },
    administrative_open_data: SourceCopy {
        title: "Open data de la justice administrative, Conseil d'État",
        body: &[
            &[
                Text("Décisions de l'ordre administratif (Conseil d'État, cours administratives d'appel, tribunaux administratifs), diffusées par le Conseil d'État en archives ZIP XML mises à jour quotidiennement et pseudonymisées avant publication. LibreJustice les resynchronise chaque jour. Le flux fournit le texte intégral des décisions, mais ni les analyses ni les conclusions des rapporteurs publics, qui ne sont pas diffusées en open data. Les analyses viennent d'ArianeWeb, présenté plus bas."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text(", complétée par les "),
            Link("https://opendata.justice-administrative.fr/cgu.html", "conditions générales d'utilisation"),
            Text(" du Conseil d'État."),
        ],
    },
    arianeweb: SourceCopy {
        title: "ArianeWeb, Conseil d'État",
        body: &[
            &[
                Text("ArianeWeb est la base de jurisprudence du Conseil d'État. LibreJustice y reprend les analyses rédigées pour ses décisions (fonds AJCE), l'existence des conclusions du rapporteur public et quelques décisions absentes des autres canaux. Il les rattache aux décisions et les resynchronise chaque jour. La liste des grandes décisions publiée par le Conseil d'État donne aussi leur nom aux décisions qu'elle recense."),
            ],
        ],
        reuse: &[
            Text("informations publiques librement réutilisables, à condition de ne pas les altérer et d'en indiquer la source et la date, selon les "),
            Link("https://www.conseil-etat.fr/mentions-legales", "mentions légales du Conseil d'État"),
            Text("."),
        ],
    },
    consiliaweb: SourceCopy {
        title: "ConsiliaWeb, Conseil d'État",
        body: &[
            &[
                Text("ConsiliaWeb est la base des avis consultatifs du Conseil d'État : ceux qu'il rend au Gouvernement et au Parlement sur les projets de loi, les décrets et les questions qui lui sont soumises. LibreJustice reprend chaque jour les avis que la base publie, avec le texte de leur document original, sous le Conseil d'État, dans sa formation consultative."),
            ],
        ],
        reuse: &[
            Text("informations publiques librement réutilisables, à condition de ne pas les altérer, d'en indiquer la date et de citer le site internet du Conseil d'État comme source, selon les "),
            Link("https://www.conseil-etat.fr/bas-de-page/mentions-legales", "mentions légales du Conseil d'État"),
            Text("."),
        ],
    },
    administrative_court_sites: SourceCopy {
        title: "Sites des tribunaux administratifs et des cours administratives d'appel",
        body: &[
            &[
                Text("Chaque tribunal administratif et chaque cour administrative d'appel publie sur son site, sous « Dernières décisions », des décisions qui n'arrivent dans l'open data que plus tard, ou pas du tout. LibreJustice reprend chaque décision jointe en PDF, telle que la juridiction l'a publiée. Quand l'open data diffuse la même décision, son texte prend le relais. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Text("décisions de justice mises à la disposition du public par la juridiction, selon l'article L. 10 du code de justice administrative."),
        ],
    },
    constitutional_council_site: SourceCopy {
        title: "Site du Conseil constitutionnel",
        body: &[
            &[
                Text("Le Conseil constitutionnel publie ses décisions sur son site plusieurs semaines avant qu'elles n'arrivent dans le fonds CONSTIT de la DILA. LibreJustice reprend chaque jour les décisions que le site annonce et que ce fonds ne porte pas encore. Quand la DILA diffuse la même décision, son texte prend le relais."),
            ],
        ],
        reuse: &[
            Text("textes librement reproductibles, à condition de citer le site comme source et d'en respecter l'intégrité, selon les "),
            Link("https://www.conseil-constitutionnel.fr/mentions-legales", "mentions légales du Conseil constitutionnel"),
            Text("."),
        ],
    },
    cnda: SourceCopy {
        title: "CNDA, Cour nationale du droit d'asile",
        body: &[
            &[
                Text("Les décisions de la CNDA ne figurent ni dans Judilibre ni dans l'open data administratif. LibreJustice reprend celles que la Cour publie sur son site avec l'analyse qu'elle en rédige. Le texte intégral est extrait du document joint. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Text("celles du site de la CNDA, fixées par ses "),
            Link("https://www.cnda.fr/mentions-legales", "mentions légales"),
            Text("."),
        ],
    },
    ddd: SourceCopy {
        title: "Défenseur des droits",
        body: &[
            &[
                Text("Le Défenseur des droits publie dans sa base juridique ses décisions et celles des autorités auxquelles il a succédé en 2011 : la HALDE, la CNDS, le Défenseur des enfants et le Médiateur de la République. LibreJustice en reprend les décisions avec leur notice. Le texte intégral est extrait du document joint. La reprise est quotidienne."),
            ],
            &[
                Text("Il y publie aussi les jugements et arrêts rendus dans les affaires où il est intervenu, pseudonymisés par ses soins. LibreJustice reprend ceux des juridictions françaises de première instance et d'appel, hors affaires concernant un mineur, rattachés à la décision déjà connue d'une autre source quand elle l'est."),
            ],
        ],
        reuse: &[
            Text("informations publiques librement réutilisables au sens du code des relations entre le public et les administrations, selon les "),
            Link("https://www.defenseurdesdroits.fr/mentions-legales-367", "mentions légales du Défenseur des droits"),
            Text("."),
        ],
    },
    adlc: SourceCopy {
        title: "Autorité de la concurrence",
        body: &[
            &[
                Text("L'Autorité de la concurrence publie sur data.gouv.fr le texte intégral de ses décisions et de celles du Conseil de la concurrence depuis 1988. LibreJustice en reprend les décisions contentieuses, les mesures conservatoires et les décisions de contrôle des concentrations. Les avis ne sont pas repris. La reprise est hebdomadaire, au rythme du jeu de données."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    amf: SourceCopy {
        title: "Autorité des marchés financiers",
        body: &[
            &[
                Text("Les décisions de la Commission des sanctions de l'AMF ne figurent dans aucun autre canal. LibreJustice reprend celles que l'Autorité publie sur son site. Le texte intégral est extrait du document joint. La reprise est quotidienne."),
            ],
            &[
                Text("Depuis 2017, une décision peut fixer la durée pendant laquelle elle reste en ligne sans être anonymisée. LibreJustice la retire avant cette date, puis reprend la version anonymisée que l'Autorité publie ensuite."),
            ],
        ],
        reuse: &[
            Text("usage personnel, éducatif, associatif ou professionnel, à l'exclusion de tout usage commercial ou publicitaire, sans altérer la teneur ni la portée des décisions, selon les "),
            Link("https://www.amf-france.org/fr/mentions-legales-et-editoriales", "mentions légales et éditoriales de l'AMF"),
            Text(". Source : Autorité des marchés financiers, amf-france.org, mise à jour quotidienne. ©Autorité des marchés financiers \u{2013} droits réservés."),
        ],
    },
    dila_case_law: SourceCopy {
        title: "DILA, fonds de jurisprudence",
        body: &[
            &[
                Text("La Direction de l'information légale et administrative diffuse aussi des fonds de décisions. LibreJustice en reprend JADE (jurisprudence administrative, dont le Conseil d'État avec ses analyses), CONSTIT (décisions du Conseil constitutionnel), CNIL (délibérations de la CNIL, dont ses sanctions) et les fonds judiciaires CASS, INCA et CAPP. JADE, CONSTIT et CNIL sont resynchronisés chaque jour. Une décision reçue par plusieurs canaux n'a qu'une fiche."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    hudoc: SourceCopy {
        title: "HUDOC, Cour européenne des droits de l'homme",
        body: &[
            &[
                Text("Arrêts et décisions de la Cour européenne des droits de l'homme, diffusés par la Cour dans sa base HUDOC. LibreJustice en reprend les versions française et anglaise et les resynchronise chaque jour."),
            ],
        ],
        reuse: &[
            Text("la Cour autorise la reproduction de ses textes, source mentionnée, à titre gratuit et à des fins d'usage privé, d'information et d'éducation. Les autres usages demandent son autorisation écrite ("),
            Link("https://www.echr.coe.int/fr/copyright-and-disclaimer", "conditions de la Cour"),
            Text(")."),
        ],
    },
    contribution: SourceCopy {
        title: "Décisions versées par les utilisateurs",
        body: &[
            &[
                Text("Des décisions qu'aucune source ouverte ne diffuse, versées depuis la page "),
                Local("/decision/contribute", "Verser une décision"),
                Text(". Chacune est anonymisée et relue avant d'être publiée."),
            ],
        ],
        reuse: &[],
    },
    dila: SourceCopy {
        title: "DILA, bases juridiques de l'État",
        body: &[
            &[
                Text("La Direction de l'information légale et administrative diffuse en open data les bases du droit français. LibreJustice en reprend LEGI (codes et lois consolidés, versionnés par article : chaque article se lit en vigueur ou à une date passée), le JORF (Journal officiel, dont les traités publiés par décret), KALI (conventions collectives nationales) et CIRCULAIRES (circulaires et instructions des ministères). Ces fonds sont resynchronisés chaque jour à partir des incréments de la DILA."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    eur_lex: SourceCopy {
        title: "EUR-Lex, droit de l'Union et CJUE",
        body: &[
            &[
                Text("L'Office des publications de l'Union européenne diffuse le droit de l'Union et la jurisprudence de ses juridictions. LibreJustice en reprend les règlements et directives et, pour les actes consolidés, leurs versions successives : chaque article se lit à la date demandée. Il reprend aussi les arrêts, ordonnances et conclusions des avocats généraux de la Cour de justice de l'Union européenne, dans leur langue de procédure, en français et en anglais. Les deux fonds sont resynchronisés chaque jour."),
            ],
        ],
        reuse: &[
            Text("réutilisation libre, y compris commerciale, selon l'"),
            Link("https://eur-lex.europa.eu/content/legal-notice/legal-notice.html", "avis juridique d'EUR-Lex"),
            Text(". Les textes consolidés sont sous licence CC BY 4.0."),
        ],
    },
    curia: SourceCopy {
        title: "Curia, site de la Cour de justice de l'Union européenne",
        body: &[
            &[
                Text("La Cour de justice de l'Union européenne publie sur son site le texte intégral de ses décisions. LibreJustice y reprend celles qu'EUR-Lex ne diffuse pas et celles dont il ne donne que le résumé, dans les mêmes langues."),
            ],
        ],
        reuse: &[
            Text("reproduction autorisée avec mention de la source, selon la "),
            Link("https://curia.europa.eu/site/jcms/d2_5167/fr/copyright", "page droits d'auteur de la Cour"),
            Text("."),
        ],
    },
    comeu: SourceCopy {
        title: "Commission européenne, affaires de concurrence",
        body: &[
            &[
                Text("La Commission européenne publie ses décisions en matière d'ententes et d'abus de position dominante, de concentrations, d'aides d'État, de marchés numériques et de subventions étrangères. LibreJustice reprend chaque décision publiée avec son document, dans sa langue d'origine, en français et en anglais. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/licenses/by/4.0/deed.fr", "CC BY 4.0"),
            Text(", selon l'"),
            Link("https://commission.europa.eu/legal-notice_fr", "avis juridique de la Commission"),
            Text("."),
        ],
    },
    juricaf: SourceCopy {
        title: "JuriCAF, Association des hautes juridictions de cassation francophones",
        body: &[
            &[
                Text("JuriCAF est la base de jurisprudence de l'AHJUCAF, alimentée par les cours suprêmes membres. LibreJustice en reprend les décisions qu'aucune source officielle ne diffuse en ligne : cours suprêmes d'une trentaine de pays, et juridictions régionales africaines (OHADA, UEMOA, CEMAC, CEDEAO, Cour africaine des droits de l'homme et des peuples). La reprise est quotidienne."),
            ],
            &[
                Text("Quand JuriCAF ne donne que la référence d'une décision, LibreJustice en reprend le texte depuis une copie de la décision rendue par la juridiction. Les arrêts de la Cour suprême de Madagascar et les décisions marocaines, en arabe, publiées par jurisprudence.ma, viennent des copies conservées par l'Internet Archive. Les arrêts et jugements des juridictions du Burkina Faso et du Congo sont repris tels que la juridiction les a rendus."),
            ],
        ],
        reuse: &[
            Link("https://opendatacommons.org/licenses/odbl/1-0/", "ODbL 1.0"),
            Text(", selon les "),
            Link("https://juricaf.org/static/donneespersonnelles", "mentions légales de JuriCAF"),
            Text("."),
        ],
    },
    entscheidsuche: SourceCopy {
        title: "entscheidsuche.ch, jurisprudence suisse",
        body: &[
            &[
                Text("entscheidsuche.ch est une association suisse d'utilité publique qui réunit chaque jour les décisions publiées par les tribunaux fédéraux et cantonaux. LibreJustice en reprend les décisions des tribunaux et des autorités administratives, en français, en allemand et en italien : Tribunal fédéral, Tribunal administratif fédéral, Tribunal pénal fédéral, tribunaux des cantons, autorités fédérales et cantonales."),
            ],
        ],
        reuse: &[
            Text("Les décisions de justice ne sont pas protégées par le droit d'auteur suisse (art. 5 LDA) ; entscheidsuche.ch les diffuse sans licence."),
        ],
    },
    opencaselaw: SourceCopy {
        title: "OpenCaseLaw, jurisprudence d'Obwald (Suisse)",
        body: &[
            &[
                Text("OpenCaseLaw publie en données ouvertes, sur Hugging Face, les décisions de tribunaux et d'autorités suisses. LibreJustice en reprend les décisions du canton d'Obwald, en allemand, tirées de ses recueils officiels : Tribunal supérieur, Tribunal administratif, Conseil d'État, départements et commissions du canton. La reprise est hebdomadaire."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/publicdomain/zero/1.0/deed.fr", "CC0 1.0"),
            Text(", selon la "),
            Link("https://huggingface.co/datasets/voilaj/swiss-caselaw", "fiche du jeu de données"),
            Text("."),
        ],
    },
    legimonaco: SourceCopy {
        title: "Légimonaco, jurisprudence monégasque",
        body: &[
            &[
                Text("Légimonaco est le site officiel de la législation et de la jurisprudence monégasques. LibreJustice en reprend les décisions de justice, publiées anonymisées : Tribunal suprême, Cour de révision, Cour d'appel, Tribunal de première instance, Tribunal du travail et autres juridictions de la Principauté."),
            ],
        ],
        reuse: &[],
    },
    juportal: SourceCopy {
        title: "Juportal, jurisprudence belge",
        body: &[
            &[
                Text("Juportal est la base de données publique de la jurisprudence belge, tenue par le SPF Justice. LibreJustice en reprend les décisions rendues en français : Cour de cassation, Cour constitutionnelle, Conseil d'État, cours d'appel, cours du travail et tribunaux. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Text("Les actes officiels de l'autorité ne donnent pas lieu au droit d'auteur (art. XI.172, § 2, du Code de droit économique belge)."),
        ],
    },
    rechtspraak: SourceCopy {
        title: "Rechtspraak.nl, jurisprudence néerlandaise",
        body: &[
            &[
                Text("Rechtspraak.nl est le site du pouvoir judiciaire néerlandais, qui publie en données ouvertes les décisions pseudonymisées des juridictions. LibreJustice en reprend les décisions du Hoge Raad, du Raad van State, de la Centrale Raad van Beroep, du College van Beroep voor het bedrijfsleven, des cours d'appel et des tribunaux. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Text("Les décisions de justice ne donnent pas lieu au droit d'auteur (art. 11 de la loi néerlandaise sur le droit d'auteur, Auteurswet). Le jeu de données est publié sous Public Domain Mark 1.0."),
        ],
    },
    cce: SourceCopy {
        title: "CCE, Conseil du contentieux des étrangers (Belgique)",
        body: &[
            &[
                Text("Le Conseil du contentieux des étrangers est la juridiction administrative belge du séjour des étrangers et de l'asile. Il publie ses arrêts dépersonnalisés sur son site. LibreJustice en reprend les arrêts rendus en français, en néerlandais et en allemand, dont le texte est extrait du PDF publié. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Text("Les actes officiels de l'autorité ne donnent pas lieu au droit d'auteur (art. XI.172, § 2, du Code de droit économique belge)."),
        ],
    },
    justice_lu: SourceCopy {
        title: "data.public.lu, jurisprudence luxembourgeoise",
        body: &[
            &[
                Text("L'Administration judiciaire du Grand-Duché de Luxembourg publie chaque semaine sur data.public.lu, le portail luxembourgeois des données ouvertes, les décisions pseudonymisées des juridictions de l'ordre judiciaire. LibreJustice en reprend les décisions de la Cour de cassation, de la Cour d'appel, des tribunaux d'arrondissement de Luxembourg et de Diekirch, des justices de paix et du Conseil supérieur de la sécurité sociale. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/licenses/by-nd/4.0/deed.fr", "CC BY-ND 4.0"),
            Text(", selon la description de chaque jeu de données, par exemple celui de la "),
            Link("https://data.public.lu/fr/datasets/cour-de-cassation-1/", "Cour de cassation"),
            Text("."),
        ],
    },
    supreme_court_canada: SourceCopy {
        title: "Cour suprême du Canada",
        body: &[
            &[
                Text("Le site de la Cour suprême du Canada diffuse ses jugements rendus depuis 1970, en français et en anglais. LibreJustice en reprend la version française. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Text("Le Décret sur la reproduction de la législation fédérale et des décisions des tribunaux de constitution fédérale (TR/97-5) permet de les reproduire sans frais ni autorisation, à condition d'en assurer l'exactitude et de ne pas présenter la reproduction comme une version officielle."),
        ],
    },
    a2aj: SourceCopy {
        title: "A2AJ, jurisprudence canadienne",
        body: &[
            &[
                Text("Le projet A2AJ (Access to Algorithmic Justice) de l'Université York et de l'Université métropolitaine de Toronto publie en données ouvertes, sur Hugging Face, les décisions de juridictions canadiennes recueillies sur leurs sites. LibreJustice en reprend, en français et en anglais, les décisions de la Cour suprême du Canada, de la Cour d'appel fédérale, de la Cour fédérale, de la Cour canadienne de l'impôt, de la Cour d'appel de la cour martiale du Canada et de tribunaux administratifs fédéraux, dont le Tribunal de la sécurité sociale, la Commission de l'immigration et du statut de réfugié et le Tribunal canadien du commerce extérieur. S'y ajoutent, en anglais, les décisions de la Cour d'appel de l'Ontario et de dix-huit tribunaux administratifs de l'Ontario, dont la Commission de la location immobilière et le Tribunal des droits de la personne, de la Cour d'appel et de la Cour suprême de la Colombie-Britannique, de la Cour d'appel du Yukon, des cinq juridictions de la Nouvelle-Écosse et de la Cour d'appel de Terre-Neuve-et-Labrador, ainsi que les décisions de la Section de la protection des réfugiés publiées par le Refugee Law Lab. La reprise est hebdomadaire."),
            ],
        ],
        reuse: &[
            Text("Le Décret sur la reproduction de la législation fédérale et des décisions des tribunaux de constitution fédérale (TR/97-5) permet de les reproduire sans frais ni autorisation, à condition d'en assurer l'exactitude et de ne pas présenter la reproduction comme une version officielle. Les juridictions de la Nouvelle-Écosse indiquent que le droit d'auteur ne s'applique pas à leurs décisions publiées, et celles de Terre-Neuve-et-Labrador en permettent la libre reproduction. Le travail d'A2AJ est sous licence MIT, selon la "),
            Link("https://huggingface.co/datasets/a2aj/canadian-case-law", "fiche du jeu de données"),
            Text("."),
        ],
    },
    african_court_sites: SourceCopy {
        title: "Sites des juridictions africaines",
        body: &[
            &[
                Text("Plusieurs juridictions publient leurs décisions sur leur propre site : la "),
                Link("https://courconstitutionnelle.bj/fr/decisions", "Cour constitutionnelle du Bénin"),
                Text(", le "),
                Link("https://www.conseil-constitutionnel.ci/decisions", "Conseil constitutionnel de Côte d'Ivoire"),
                Text(", le "),
                Link("https://www.conseil-constitutionnel.dj/decisions/", "Conseil constitutionnel de Djibouti"),
                Text(", la "),
                Link("https://eservices.courdecassation.ci/search/stoppages?matter=civ_com", "Cour de cassation de Côte d'Ivoire"),
                Text(", la "),
                Link("https://www.hcc.gov.mg/", "Haute Cour constitutionnelle de Madagascar"),
                Text(", la "),
                Link("https://jurisprudence.justice.gov.mg/", "Cour de cassation de Madagascar"),
                Text(" et la "),
                Link("https://www.african-court.org/ajudata/latest-decisions/judgments", "Cour africaine des droits de l'homme et des peuples"),
                Text(". L'OHADA publie celles de sa Cour commune de justice et d'arbitrage dans sa "),
                Link("https://biblio.ohada.org/", "bibliothèque numérique"),
                Text(". LibreJustice en reprend les décisions rendues en français, telles que la juridiction les a publiées."),
            ],
        ],
        reuse: &[],
    },
    djibouti_journal: SourceCopy {
        title: "Journal officiel de la République de Djibouti",
        body: &[
            &[
                Text("Le "),
                Link("https://www.journalofficiel.dj/", "Journal officiel de la République de Djibouti"),
                Text(" publie en ligne ses numéros parus depuis 1900. LibreJustice en reprend chaque texte avec le numéro qui l'a publié, et range les décisions du Conseil constitutionnel avec la jurisprudence. La reprise est quotidienne."),
            ],
        ],
        reuse: &[],
    },
    legalis: SourceCopy {
        title: "Legalis, droit du numérique",
        body: &[
            &[
                Text("Legalis publie depuis 1995 des décisions de justice en droit du numérique, pseudonymisées par ses soins. LibreJustice en reprend le texte des décisions, rattaché à la décision déjà connue d'une autre source quand elle l'est. La reprise est quotidienne."),
            ],
        ],
        reuse: &[],
    },
    bofip: SourceCopy {
        title: "BOFiP-Impôts, doctrine fiscale",
        body: &[
            &[
                Text("Le Bulletin officiel des finances publiques porte la doctrine fiscale de la DGFiP, opposable à l'administration. LibreJustice reprend les publications en vigueur, diffusées en open data par le ministère de l'Économie."),
            ],
        ],
        reuse: &[
            Link("https://data.economie.gouv.fr/explore/dataset/bofip-vigueur/information/", "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    boss: SourceCopy {
        title: "BOSS, doctrine sociale",
        body: &[
            &[
                Text("Le Bulletin officiel de la sécurité sociale porte la doctrine de l'administration sur les cotisations et contributions sociales, opposable à l'administration. LibreJustice en reprend chaque rubrique, paragraphe par paragraphe, avec les versions antérieures que le site conserve : chaque paragraphe se lit dans sa version en vigueur ou à une date passée. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Link("https://opendatacommons.org/licenses/odbl/1-0/", "ODbL 1.0"),
            Text(", selon les "),
            Link("https://boss.gouv.fr/portail/accueil/mentions-legales.html", "mentions légales du BOSS"),
            Text("."),
        ],
    },
    bulletins: SourceCopy {
        title: "Bulletins officiels des ministères",
        body: &[
            &[
                Text("Les ministères publient leurs circulaires, instructions et notes sur les sites que désigne l'"),
                Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/d312-11", "article D. 312-11 du code des relations entre le public et l'administration"),
                Text(". LibreJustice reprend les bulletins officiels des ministères de l'écologie, de la justice, des armées, de l'agriculture, de la santé et du travail, de la culture, des affaires étrangères, de l'éducation nationale, de l'enseignement supérieur, des sports, de l'intérieur et de l'économie, ainsi que le bulletin officiel des douanes et les circulaires de la fonction publique. Chaque texte est repris avec ses dates, son NOR quand il en porte un et le lien vers sa publication. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Text("informations publiques librement réutilisables ("),
            Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/l321-1", "article L. 321-1 du code des relations entre le public et l'administration"),
            Text(")."),
        ],
    },
    acpr: SourceCopy {
        title: "ACPR, registre officiel",
        body: &[
            &[
                Text("L'Autorité de contrôle prudentiel et de résolution publie ses actes dans son registre officiel. LibreJustice reprend chacun de ces actes ; les instructions, recommandations, positions, notices et lignes directrices rejoignent aussi les normes, leur texte lu dans le PDF publié. La reprise est quotidienne."),
            ],
        ],
        reuse: &[
            Text("informations publiques librement réutilisables ("),
            Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/l321-1", "article L. 321-1 du code des relations entre le public et l'administration"),
            Text(")."),
        ],
    },
    jafbase: SourceCopy {
        title: "JaFBase, droit international privé de la famille",
        body: &[
            &[
                Text("Base bénévole tenue depuis 2008 par Cyril Roth, magistrat, l'une des seules à rendre accessible le droit étranger de la famille. Avec l'autorisation de l'auteur, LibreJustice y reprend les codes étrangers (famille, état des personnes, code civil) découpés article par article et reliés aux décisions qui les citent. JaFBase n'est pas qu'un recueil de textes : c'est aussi une méthode, une "),
                Link("http://jafbase.fr/instruments", "carte mentale"),
                Text(" pour déterminer la règle de conflit applicable. Ces textes ne sont ni exhaustifs ni garantis à jour."),
            ],
        ],
        reuse: &[
            Text("reprise avec l'autorisation de l'auteur."),
        ],
    },
    hal: SourceCopy {
        title: "HAL, archive ouverte",
        body: &[
            &[
                Text("LibreJustice relie aux décisions les notes d'arrêt, chroniques et thèses déposées dans HAL qui les commentent. Les documents restent sur HAL ou chez leur éditeur et LibreJustice n'en reprend que le lien."),
            ],
        ],
        reuse: &[
            Text("celles que fixe l'auteur ou l'éditeur de chaque document."),
        ],
    },
    cairn: SourceCopy {
        title: "Cairn",
        body: &[
            &[
                Text("Les notes d'arrêt du Droit Ouvrier et de la Revue critique de droit international privé sont reliées aux décisions qu'elles commentent, à partir des métadonnées que Cairn publie. Les articles restent sur Cairn, certains réservés aux abonnés, et LibreJustice n'en reprend que le lien."),
            ],
        ],
        reuse: &[
            Text("métadonnées en libre moissonnage ; pour les articles, celles de Cairn et de chaque revue."),
        ],
    },
    hypotheses: SourceCopy {
        title: "Hypothèses, carnets de recherche",
        body: &[
            &[
                Text("Les notes des carnets "),
                Link("https://dsf.hypotheses.org/", "Droit du travail et de la protection sociale"),
                Text(" et "),
                Link("https://m2dsplille.hypotheses.org/", "Droit de la protection sociale"),
                Text(", de l'université de Lille, sont reliées de la même façon, à partir des métadonnées qu'OpenEdition publie."),
            ],
        ],
        reuse: &[
            Text("métadonnées sous licence CC0 ; pour les billets, celles de chaque carnet."),
        ],
    },
    ajdc: SourceCopy {
        title: "Actualité juridique du dommage corporel",
        body: &[
            &[
                Text("Les commentaires de la revue de l'université Lumière Lyon 2 sont reliés aux décisions qu'ils commentent, lus sur la page de chaque article."),
            ],
        ],
        reuse: &[
            Text("licence CC BY."),
        ],
    },
    boamp: SourceCopy {
        title: "BOAMP",
        body: &[
            &[
                Text("Avis de marchés publics en cours et avis d'attribution du Bulletin officiel des annonces des marchés publics, relevés chaque jour."),
            ],
        ],
        reuse: &[
            Link("https://www.data.gouv.fr/fr/datasets/boamp/", "Licence Ouverte"),
            Text("."),
        ],
    },
    ted: SourceCopy {
        title: "TED",
        body: &[
            &[
                Text("Les avis français du TED, supplément au Journal officiel de l'Union européenne, relevés chaque jour."),
            ],
        ],
        reuse: &[
            Text("librement réutilisables, y compris à des fins commerciales ("),
            Link("https://ted.europa.eu/fr/legal-notice", "conditions de TED"),
            Text(")."),
        ],
    },
    decp: SourceCopy {
        title: "Données essentielles de la commande publique (DECP)",
        body: &[
            &[
                Text("Elles donnent le titulaire des marchés conclus."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    buyer_platforms: SourceCopy {
        title: "Plateformes des acheteurs",
        body: &[
            &[
                Text("LibreJustice relève aussi les plateformes de dématérialisation des acheteurs, où paraissent beaucoup de marchés à procédure adaptée absents du BOAMP. Une consultation vue sur plusieurs canaux fait une seule fiche, qui renvoie au dossier sur la plateforme de l'acheteur."),
            ],
        ],
        reuse: &[],
    },
    sirene: SourceCopy {
        title: "SIRENE, Insee",
        body: &[
            &[
                Text("Le répertoire des entreprises et de leurs établissements, tenu par l'Insee. LibreJustice en reprend les unités légales diffusibles, sociétés comme entrepreneurs individuels, et en fait ses fiches entité."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    rna: SourceCopy {
        title: "Répertoire national des associations (RNA)",
        body: &[
            &[
                Text("Tenu par le ministère de l'Intérieur, il recense les associations déclarées. LibreJustice en tire une fiche par association."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    lawyers: SourceCopy {
        title: "Annuaire des avocats, Conseil national des barreaux",
        body: &[
            &[
                Text("L'annuaire national publié par le Conseil national des barreaux recense les avocats inscrits aux barreaux français. LibreJustice le relit chaque semaine et en tire une fiche par avocat."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    company_search: SourceCopy {
        title: "API Recherche d'entreprises",
        body: &[
            &[
                Text("Le service public qui alimente l'Annuaire des entreprises. À l'affichage, la fiche d'une entreprise y lit son identité à jour, ses dirigeants et ses données financières."),
            ],
        ],
        reuse: &[],
    },
    bodacc: SourceCopy {
        title: "BODACC",
        body: &[
            &[
                Text("Le Bulletin officiel des annonces civiles et commerciales, publié par la DILA, annonce les immatriculations, modifications, ventes et procédures collectives des entreprises. La fiche d'une entreprise en affiche la chronologie, lue au moment de l'affichage."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    joafe: SourceCopy {
        title: "Journal officiel des associations et fondations d'entreprise (JOAFE)",
        body: &[
            &[
                Text("Publié par la DILA, il annonce la création, les modifications et la dissolution des associations. La fiche d'une association en affiche les annonces."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
};
