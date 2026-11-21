use crate::i18n::Run::{Link, Local, Text};
use crate::i18n::{SourceCopy, SourcesUi};

const LICENCE_OUVERTE: &str = "https://www.etalab.gouv.fr/licence-ouverte-open-licence/";

pub const SOURCES: SourcesUi = SourcesUi {
    title: "Gegevens & bronnen - LibreJustice",
    heading: "Gegevens & bronnen",
    intro: "LibreJustice brengt openbare juridische gegevens samen: rechterlijke beslissingen, teksten (wetboeken, wetten, verdragen, Unierecht), aankondigingen van overheidsopdrachten en registers. Hieronder wordt elke bron beschreven met haar herkomst en haar voorwaarden voor hergebruik. LibreJustice wijzigt de inhoud niet en garandeert niet dat het corpus volledig of actueel is.",
    reuse: "Voorwaarden voor hergebruik: ",
    case_law: "Rechtspraak",
    norms: "Regelgeving",
    commentaries: "Commentaren bij beslissingen",
    procurement: "Overheidsopdrachten",
    registers: "Ondernemingen, verenigingen en advocaten",
    judilibre: SourceCopy {
        title: "Judilibre, Cour de cassation",
        body: &[
            &[
                Text("Beslissingen van de Franse rechterlijke orde (Cour de cassation, hoven van beroep, rechtbanken, handelsrechtbanken en conseils de prud'hommes), die de Cour de cassation als open data verspreidt via de API Judilibre en vóór publicatie pseudonimiseert. LibreJustice neemt de volledige stroom over en synchroniseert die elke dag opnieuw. De Franse open data van de rechtspraak worden geleidelijk uitgebreid: de strafzaken in eerste aanleg worden in 2027 verwacht ("),
                Link("https://www.justice.gouv.fr/documentation/open-data-decisions-justice", "tijdpad van de openstelling"),
                Text(")."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text(", aangevuld met de "),
            Link("https://www.courdecassation.fr/conditions-generales-dutilisation-pour-la-reutilisation-des-donnees-issues-des-decisions-de-justice", "algemene gebruiksvoorwaarden"),
            Text(" van de Cour de cassation."),
        ],
    },
    administrative_open_data: SourceCopy {
        title: "Open data van de Franse administratieve rechtspraak, Conseil d'État",
        body: &[
            &[
                Text("Beslissingen van de Franse administratieve rechtscolleges (Conseil d'État, administratieve hoven van beroep, administratieve rechtbanken), die de Conseil d'État verspreidt in ZIP-archieven met XML, elke dag bijgewerkt en vóór publicatie gepseudonimiseerd. LibreJustice synchroniseert ze elke dag opnieuw. De stroom levert de volledige tekst van de beslissingen, maar niet de analyses of de conclusies van de rapporteur public, die niet als open data worden verspreid. De analyses komen uit ArianeWeb, dat verderop wordt beschreven."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text(", aangevuld met de "),
            Link("https://opendata.justice-administrative.fr/cgu.html", "algemene gebruiksvoorwaarden"),
            Text(" van de Conseil d'État."),
        ],
    },
    arianeweb: SourceCopy {
        title: "ArianeWeb, Conseil d'État",
        body: &[
            &[
                Text("ArianeWeb is de rechtspraakdatabank van de Conseil d'État. LibreJustice neemt er de analyses over die voor zijn beslissingen zijn opgesteld (verzameling AJCE), het bestaan van conclusies van de rapporteur public en enkele beslissingen die via geen ander kanaal beschikbaar zijn. Ze worden aan de beslissingen gekoppeld en elke dag opnieuw gesynchroniseerd. De lijst van belangrijke beslissingen die de Conseil d'État publiceert, geeft de beslissingen die erop staan ook hun gangbare naam."),
            ],
        ],
        reuse: &[
            Text("vrij herbruikbare overheidsinformatie, op voorwaarde dat ze niet wordt gewijzigd en dat de bron en de datum worden vermeld, volgens de "),
            Link("https://www.conseil-etat.fr/mentions-legales", "juridische informatie van de Conseil d'État"),
            Text("."),
        ],
    },
    consiliaweb: SourceCopy {
        title: "ConsiliaWeb, Conseil d'État",
        body: &[
            &[
                Text("ConsiliaWeb is de databank van de adviezen van de Conseil d'État: de adviezen die hij aan de regering en het parlement geeft over wetsontwerpen, decreten en de vragen die hem worden voorgelegd. LibreJustice neemt elke dag de adviezen over die de databank publiceert, met de tekst van hun oorspronkelijke document, en rangschikt ze onder de Conseil d'État in zijn adviserende samenstelling."),
            ],
        ],
        reuse: &[
            Text("vrij herbruikbare overheidsinformatie, op voorwaarde dat ze niet wordt gewijzigd, dat de datum wordt vermeld en dat de website van de Conseil d'État als bron wordt genoemd, volgens de "),
            Link("https://www.conseil-etat.fr/bas-de-page/mentions-legales", "juridische informatie van de Conseil d'État"),
            Text("."),
        ],
    },
    administrative_court_sites: SourceCopy {
        title: "Websites van de Franse administratieve rechtbanken en administratieve hoven van beroep",
        body: &[
            &[
                Text("Elke administratieve rechtbank en elk administratief hof van beroep publiceert op zijn website, onder \"Dernières décisions\", beslissingen die pas later of helemaal niet in de open data terechtkomen. LibreJustice neemt elke beslissing over die als PDF is bijgevoegd, zoals het rechtscollege ze heeft gepubliceerd. Wanneer dezelfde beslissing in de open data verschijnt, neemt de tekst uit de open data het over. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("rechterlijke beslissingen die het rechtscollege ter beschikking van het publiek stelt, volgens artikel L. 10 van de code de justice administrative."),
        ],
    },
    constitutional_council_site: SourceCopy {
        title: "Website van de Conseil constitutionnel",
        body: &[
            &[
                Text("De Conseil constitutionnel publiceert zijn beslissingen op zijn website enkele weken voordat ze in de verzameling CONSTIT van de DILA verschijnen. LibreJustice neemt elke dag de beslissingen over die de website aankondigt en die nog niet in die verzameling staan. Wanneer de DILA dezelfde beslissing verspreidt, neemt haar tekst het over."),
            ],
        ],
        reuse: &[
            Text("teksten die vrij mogen worden gereproduceerd, op voorwaarde dat de website als bron wordt vermeld en de teksten ongewijzigd blijven, volgens de "),
            Link("https://www.conseil-constitutionnel.fr/mentions-legales", "juridische informatie van de Conseil constitutionnel"),
            Text("."),
        ],
    },
    cnda: SourceCopy {
        title: "CNDA, Cour nationale du droit d'asile",
        body: &[
            &[
                Text("De beslissingen van de CNDA staan niet in Judilibre en ook niet in de administratieve open data. LibreJustice neemt de beslissingen over die het Hof op zijn website publiceert, met de analyse die het erbij opstelt. De volledige tekst wordt uit het bijgevoegde document gehaald. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("die van de website van de CNDA, zoals vastgelegd in de "),
            Link("https://www.cnda.fr/mentions-legales", "juridische informatie"),
            Text("."),
        ],
    },
    ddd: SourceCopy {
        title: "Défenseur des droits",
        body: &[
            &[
                Text("De Défenseur des droits publiceert in zijn juridische databank zijn beslissingen en die van de instanties die hij in 2011 heeft opgevolgd: de HALDE, de CNDS, de Défenseur des enfants en de Médiateur de la République. LibreJustice neemt de beslissingen over met hun fiche. De volledige tekst wordt uit het bijgevoegde document gehaald. De gegevens worden dagelijks bijgewerkt."),
            ],
            &[
                Text("Hij publiceert er ook de vonnissen en arresten in de zaken waarin hij is tussengekomen, door hemzelf gepseudonimiseerd. LibreJustice neemt die van de Franse rechtscolleges in eerste aanleg en in hoger beroep over, behalve zaken over een minderjarige, en koppelt ze aan de beslissing uit een andere bron wanneer die al bekend is."),
            ],
        ],
        reuse: &[
            Text("vrij herbruikbare overheidsinformatie in de zin van de code des relations entre le public et l'administration, volgens de "),
            Link("https://www.defenseurdesdroits.fr/mentions-legales-367", "juridische informatie van de Défenseur des droits"),
            Text("."),
        ],
    },
    adlc: SourceCopy {
        title: "Autorité de la concurrence",
        body: &[
            &[
                Text("De Autorité de la concurrence publiceert op data.gouv.fr de volledige tekst van haar beslissingen en van die van de Conseil de la concurrence sinds 1988. LibreJustice neemt er de beslissingen over mededingingsbeperkende praktijken, de voorlopige maatregelen en de concentratiebeslissingen over. De adviezen worden niet overgenomen. De gegevens worden wekelijks bijgewerkt, op het ritme van de dataset."),
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
                Text("De beslissingen van de Commission des sanctions van de AMF zijn via geen enkel ander kanaal beschikbaar. LibreJustice neemt de beslissingen over die de AMF op haar website publiceert. De volledige tekst wordt uit het bijgevoegde document gehaald. De gegevens worden dagelijks bijgewerkt."),
            ],
            &[
                Text("Sinds 2017 kan een beslissing bepalen hoelang ze online blijft zonder te worden geanonimiseerd. LibreJustice verwijdert ze voordat die termijn afloopt en neemt daarna de geanonimiseerde versie over die de AMF vervolgens publiceert."),
            ],
        ],
        reuse: &[
            Text("gebruik voor persoonlijke, educatieve, beroeps- of verenigingsdoeleinden, met uitsluiting van elk commercieel of reclamegebruik, zonder de inhoud of de draagwijdte van de beslissingen te wijzigen, volgens de "),
            Link("https://www.amf-france.org/fr/mentions-legales-et-editoriales", "wettelijke en redactionele vermeldingen van de AMF"),
            Text(". Bron: Autorité des marchés financiers, amf-france.org, dagelijks bijgewerkt. ©Autorité des marchés financiers, alle rechten voorbehouden."),
        ],
    },
    dila_case_law: SourceCopy {
        title: "DILA, rechtspraakverzamelingen",
        body: &[
            &[
                Text("De Direction de l'information légale et administrative verspreidt ook verzamelingen van beslissingen. LibreJustice haalt daaruit JADE (administratieve rechtspraak, waaronder die van de Conseil d'État met zijn analyses), CONSTIT (beslissingen van de Conseil constitutionnel), CNIL (beraadslagingen van de CNIL, waaronder haar sancties) en de gerechtelijke verzamelingen CASS, INCA en CAPP. JADE, CONSTIT en CNIL worden elke dag opnieuw gesynchroniseerd. Een beslissing die via meerdere kanalen binnenkomt, heeft maar één fiche."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    hudoc: SourceCopy {
        title: "HUDOC, Europees Hof voor de Rechten van de Mens",
        body: &[
            &[
                Text("Arresten en beslissingen van het Europees Hof voor de Rechten van de Mens, die het Hof verspreidt in zijn databank HUDOC. LibreJustice neemt de Franse en de Engelse versie over en synchroniseert ze elke dag opnieuw."),
            ],
        ],
        reuse: &[
            Text("het Hof staat toe dat zijn teksten met bronvermelding kosteloos worden gereproduceerd voor privégebruik, informatie en onderwijs. Voor ander gebruik is zijn schriftelijke toestemming nodig ("),
            Link("https://www.echr.coe.int/fr/copyright-and-disclaimer", "voorwaarden van het Hof"),
            Text(")."),
        ],
    },
    contribution: SourceCopy {
        title: "Beslissingen ingestuurd door gebruikers",
        body: &[
            &[
                Text("Beslissingen die geen enkele open bron verspreidt, ingestuurd via de pagina "),
                Local("/decision/contribute", "Een beslissing insturen"),
                Text(". Elke beslissing wordt geanonimiseerd en nagelezen voordat ze wordt gepubliceerd."),
            ],
        ],
        reuse: &[],
    },
    dila: SourceCopy {
        title: "DILA, juridische databanken van de Franse Staat",
        body: &[
            &[
                Text("De Direction de l'information légale et administrative verspreidt de databanken van het Franse recht als open data. LibreJustice haalt daaruit LEGI (geconsolideerde wetboeken en wetten, met versies per artikel: elk artikel is te lezen in de geldende versie of op een datum in het verleden), het JORF (Journal officiel, waaronder de verdragen die bij decreet zijn bekendgemaakt), KALI (nationale collectieve arbeidsovereenkomsten) en CIRCULAIRES (omzendbrieven en instructies van de ministeries). Die verzamelingen worden elke dag opnieuw gesynchroniseerd op basis van de incrementele updates van de DILA."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    eur_lex: SourceCopy {
        title: "EUR-Lex, Unierecht en Hof van Justitie",
        body: &[
            &[
                Text("Het Publicatiebureau van de Europese Unie verspreidt het Unierecht en de rechtspraak van de rechtscolleges van de Unie. LibreJustice haalt daaruit de verordeningen en richtlijnen en, voor de geconsolideerde handelingen, hun opeenvolgende versies: elk artikel is te lezen op de gevraagde datum. Ook de arresten, beschikkingen en conclusies van de advocaten-generaal van het Hof van Justitie van de Europese Unie worden overgenomen, in de procestaal, in het Frans en in het Engels. Beide verzamelingen worden elke dag opnieuw gesynchroniseerd."),
            ],
        ],
        reuse: &[
            Text("vrij hergebruik, ook commercieel, volgens de "),
            Link("https://eur-lex.europa.eu/content/legal-notice/legal-notice.html", "juridische mededeling van EUR-Lex"),
            Text(". De geconsolideerde teksten vallen onder de licentie CC BY 4.0."),
        ],
    },
    curia: SourceCopy {
        title: "Curia, website van het Hof van Justitie van de Europese Unie",
        body: &[
            &[
                Text("Het Hof van Justitie van de Europese Unie publiceert de volledige tekst van zijn beslissingen op zijn website. LibreJustice neemt er de beslissingen over die EUR-Lex niet verspreidt en die waarvan EUR-Lex alleen de samenvatting geeft, in dezelfde talen."),
            ],
        ],
        reuse: &[
            Text("reproductie toegestaan met bronvermelding, volgens de "),
            Link("https://curia.europa.eu/site/jcms/d2_5167/fr/copyright", "auteursrechtpagina van het Hof"),
            Text("."),
        ],
    },
    comeu: SourceCopy {
        title: "Europese Commissie, mededingingszaken",
        body: &[
            &[
                Text("De Europese Commissie publiceert haar beslissingen over kartels en misbruik van machtspositie, concentraties, staatssteun, digitale markten en buitenlandse subsidies. LibreJustice neemt elke gepubliceerde beslissing over met haar document, in de oorspronkelijke taal, in het Frans en in het Engels. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/licenses/by/4.0/deed.fr", "CC BY 4.0"),
            Text(", volgens de "),
            Link("https://commission.europa.eu/legal-notice_fr", "juridische mededeling van de Commissie"),
            Text("."),
        ],
    },
    juricaf: SourceCopy {
        title: "JuriCAF, Association des hautes juridictions de cassation francophones",
        body: &[
            &[
                Text("JuriCAF is de rechtspraakdatabank van de AHJUCAF, gevoed door de aangesloten hooggerechtshoven. LibreJustice neemt er de beslissingen over die geen enkele officiële bron online verspreidt: hooggerechtshoven van een dertigtal landen en regionale Afrikaanse rechtscolleges (OHADA, UEMOA, CEMAC, CEDEAO, Afrikaans Hof voor de Rechten van de Mens en de Volkeren). De gegevens worden dagelijks bijgewerkt."),
            ],
            &[
                Text("Wanneer JuriCAF alleen de referentie van een beslissing geeft, haalt LibreJustice de tekst uit een kopie van de beslissing zoals het rechtscollege ze heeft gewezen. De arresten van het Hooggerechtshof van Madagaskar en de Marokkaanse beslissingen in het Arabisch, gepubliceerd door jurisprudence.ma, komen uit de kopieën die het Internet Archive bewaart. De arresten en vonnissen van de rechtscolleges van Burkina Faso en Congo worden overgenomen zoals het rechtscollege ze heeft gewezen."),
            ],
        ],
        reuse: &[
            Link("https://opendatacommons.org/licenses/odbl/1-0/", "ODbL 1.0"),
            Text(", volgens de "),
            Link("https://juricaf.org/static/donneespersonnelles", "juridische informatie van JuriCAF"),
            Text("."),
        ],
    },
    entscheidsuche: SourceCopy {
        title: "entscheidsuche.ch, Zwitserse rechtspraak",
        body: &[
            &[
                Text("entscheidsuche.ch is een Zwitserse vereniging van openbaar nut die elke dag de beslissingen verzamelt die de federale en kantonnale rechtbanken publiceren. LibreJustice haalt daaruit de beslissingen van de rechtbanken en de administratieve overheden, in het Frans, het Duits en het Italiaans: Tribunal fédéral, Tribunal administratif fédéral, Tribunal pénal fédéral, de kantonnale rechtbanken en de federale en kantonnale overheden."),
            ],
        ],
        reuse: &[
            Text("Rechterlijke beslissingen worden niet beschermd door het Zwitserse auteursrecht (art. 5 LDA); entscheidsuche.ch verspreidt ze zonder licentie."),
        ],
    },
    opencaselaw: SourceCopy {
        title: "OpenCaseLaw, rechtspraak van Obwalden (Zwitserland)",
        body: &[
            &[
                Text("OpenCaseLaw publiceert de beslissingen van Zwitserse rechtbanken en overheden als open data op Hugging Face. LibreJustice haalt daaruit de beslissingen van het kanton Obwalden, in het Duits, afkomstig uit zijn officiële verzamelingen: Obergericht, Verwaltungsgericht, Regierungsrat, departementen en commissies van het kanton. De gegevens worden wekelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/publicdomain/zero/1.0/deed.fr", "CC0 1.0"),
            Text(", volgens de "),
            Link("https://huggingface.co/datasets/voilaj/swiss-caselaw", "beschrijving van de dataset"),
            Text("."),
        ],
    },
    legimonaco: SourceCopy {
        title: "Légimonaco, Monegaskische rechtspraak",
        body: &[
            &[
                Text("Légimonaco is de officiële website van de Monegaskische wetgeving en rechtspraak. LibreJustice neemt er de rechterlijke beslissingen over, die geanonimiseerd worden gepubliceerd: Tribunal suprême, Cour de révision, Cour d'appel, Tribunal de première instance, Tribunal du travail en andere rechtscolleges van het Vorstendom."),
            ],
        ],
        reuse: &[],
    },
    juportal: SourceCopy {
        title: "Juportal, Belgische rechtspraak",
        body: &[
            &[
                Text("Juportal is de openbare databank van de Belgische rechtspraak, beheerd door de FOD Justitie. LibreJustice neemt er de beslissingen over die in het Frans zijn gewezen: Hof van Cassatie, Grondwettelijk Hof, Raad van State, hoven van beroep, arbeidshoven en rechtbanken. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("Officiële akten van de overheid geven geen aanleiding tot auteursrecht (art. XI.172, § 2, van het Wetboek van economisch recht)."),
        ],
    },
    rechtspraak: SourceCopy {
        title: "Rechtspraak.nl, Nederlandse rechtspraak",
        body: &[
            &[
                Text("Rechtspraak.nl is de website van de Nederlandse rechterlijke macht, die de gepseudonimiseerde uitspraken van de gerechten als open data publiceert. LibreJustice neemt er de uitspraken over van de Hoge Raad, de Raad van State, de Centrale Raad van Beroep, het College van Beroep voor het bedrijfsleven, de gerechtshoven en de rechtbanken. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("Op rechterlijke uitspraken rust geen auteursrecht (art. 11 Auteurswet). De dataset is gepubliceerd onder de Public Domain Mark 1.0."),
        ],
    },
    cce: SourceCopy {
        title: "RvV, Raad voor Vreemdelingenbetwistingen (België)",
        body: &[
            &[
                Text("De Raad voor Vreemdelingenbetwistingen is het Belgische administratieve rechtscollege voor het verblijf van vreemdelingen en voor asiel. Hij publiceert zijn arresten geanonimiseerd op zijn website. LibreJustice neemt er de arresten in het Frans, het Nederlands en het Duits over en haalt de tekst uit de gepubliceerde PDF. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("Officiële akten van de overheid geven geen aanleiding tot auteursrecht (art. XI.172, § 2, van het Wetboek van economisch recht)."),
        ],
    },
    justice_lu: SourceCopy {
        title: "data.public.lu, Luxemburgse rechtspraak",
        body: &[
            &[
                Text("De Administration judiciaire van het Groothertogdom Luxemburg publiceert elke week de gepseudonimiseerde beslissingen van de rechtscolleges van de rechterlijke orde op data.public.lu, het Luxemburgse portaal voor open data. LibreJustice neemt er de beslissingen over van de Cour de cassation, de Cour d'appel, de arrondissementsrechtbanken van Luxemburg en Diekirch, de vredegerechten en de Conseil supérieur de la sécurité sociale. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/licenses/by-nd/4.0/deed.fr", "CC BY-ND 4.0"),
            Text(", volgens de beschrijving van elke dataset, bijvoorbeeld die van de "),
            Link("https://data.public.lu/fr/datasets/cour-de-cassation-1/", "Cour de cassation"),
            Text("."),
        ],
    },
    supreme_court_canada: SourceCopy {
        title: "Hooggerechtshof van Canada",
        body: &[
            &[
                Text("De website van het Hooggerechtshof van Canada verspreidt de arresten die het sinds 1970 heeft gewezen, in het Frans en in het Engels. LibreJustice neemt de Franse versie over. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("Het Décret sur la reproduction de la législation fédérale et des décisions des tribunaux de constitution fédérale (TR/97-5) staat toe ze kosteloos en zonder toestemming te reproduceren, op voorwaarde dat de reproductie juist is en niet als officiële versie wordt voorgesteld."),
        ],
    },
    a2aj: SourceCopy {
        title: "A2AJ, Canadese rechtspraak",
        body: &[
            &[
                Text("Het project A2AJ (Access to Algorithmic Justice) van York University en Toronto Metropolitan University publiceert de beslissingen van Canadese rechtscolleges, verzameld op hun websites, als open data op Hugging Face. LibreJustice neemt daaruit, in het Frans en in het Engels, de beslissingen over van het Hooggerechtshof van Canada, het Federaal Hof van Beroep, het Federaal Hof, het Belastinghof van Canada, het Hof van Beroep voor de krijgsraad van Canada en federale administratieve rechtscolleges, waaronder het Tribunal de la sécurité sociale, de Commission de l'immigration et du statut de réfugié en het Tribunal canadien du commerce extérieur. Daarnaast neemt het, in het Engels, de beslissingen over van het Court of Appeal for Ontario en achttien administratieve rechtscolleges van Ontario, waaronder het Landlord and Tenant Board en het Human Rights Tribunal, het Court of Appeal en het Supreme Court van British Columbia, het Court of Appeal van Yukon, de vijf rechtscolleges van Nova Scotia en het Court of Appeal van Newfoundland en Labrador, en de door het Refugee Law Lab gepubliceerde beslissingen van de Refugee Protection Division. De gegevens worden wekelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("Het Décret sur la reproduction de la législation fédérale et des décisions des tribunaux de constitution fédérale (TR/97-5) staat toe ze kosteloos en zonder toestemming te reproduceren, op voorwaarde dat de reproductie juist is en niet als officiële versie wordt voorgesteld. De rechtscolleges van Nova Scotia geven aan dat het auteursrecht niet van toepassing is op hun gepubliceerde beslissingen, en die van Newfoundland en Labrador staan de vrije reproductie ervan toe. Het werk van A2AJ valt onder de MIT-licentie, volgens de "),
            Link("https://huggingface.co/datasets/a2aj/canadian-case-law", "beschrijving van de dataset"),
            Text("."),
        ],
    },
    african_court_sites: SourceCopy {
        title: "Websites van Afrikaanse rechtscolleges",
        body: &[
            &[
                Text("Verschillende rechtscolleges publiceren hun beslissingen op hun eigen website: de "),
                Link("https://courconstitutionnelle.bj/fr/decisions", "Cour constitutionnelle du Bénin"),
                Text(", de "),
                Link("https://www.conseil-constitutionnel.ci/decisions", "Conseil constitutionnel de Côte d'Ivoire"),
                Text(", de "),
                Link("https://www.conseil-constitutionnel.dj/decisions/", "Conseil constitutionnel de Djibouti"),
                Text(", de "),
                Link("https://www.hcc.gov.mg/", "Haute Cour constitutionnelle de Madagascar"),
                Text(", de "),
                Link("https://jurisprudence.justice.gov.mg/", "Cour de cassation de Madagascar"),
                Text(" en het "),
                Link("https://www.african-court.org/ajudata/latest-decisions/judgments", "Afrikaans Hof voor de Rechten van de Mens en de Volkeren"),
                Text(". LibreJustice neemt er de beslissingen in het Frans over, zoals het rechtscollege ze heeft gepubliceerd."),
            ],
        ],
        reuse: &[],
    },
    djibouti_journal: SourceCopy {
        title: "Staatsblad van de Republiek Djibouti",
        body: &[
            &[
                Text("Het "),
                Link("https://www.journalofficiel.dj/", "Staatsblad van de Republiek Djibouti"),
                Text(" publiceert zijn nummers sinds 1900 online. LibreJustice neemt elke tekst over met het nummer waarin hij verscheen, en rekent de beslissingen van de Conseil constitutionnel tot de rechtspraak. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[],
    },
    legalis: SourceCopy {
        title: "Legalis, digitaal recht",
        body: &[
            &[
                Text("Legalis publiceert sinds 1995 rechterlijke beslissingen over digitaal recht, die het zelf pseudonimiseert. LibreJustice neemt de tekst van de beslissingen over en koppelt die aan de beslissing uit een andere bron wanneer die al bekend is. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[],
    },
    bofip: SourceCopy {
        title: "BOFiP-Impôts, standpunten van de belastingadministratie",
        body: &[
            &[
                Text("Het Bulletin officiel des finances publiques bevat de standpunten van de DGFiP, de Franse belastingadministratie, die tegenstelbaar zijn aan de administratie. LibreJustice neemt de geldende publicaties over, die het ministerie van Economie als open data verspreidt."),
            ],
        ],
        reuse: &[
            Link("https://data.economie.gouv.fr/explore/dataset/bofip-vigueur/information/", "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    boss: SourceCopy {
        title: "BOSS, standpunten van de administratie over sociale bijdragen",
        body: &[
            &[
                Text("Het Bulletin officiel de la sécurité sociale bevat de standpunten van de administratie over de sociale bijdragen en heffingen, die tegenstelbaar zijn aan de administratie. LibreJustice neemt er elke rubriek paragraaf per paragraaf over, met de vroegere versies die de website bewaart: elke paragraaf is te lezen in de geldende versie of op een datum in het verleden. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Link("https://opendatacommons.org/licenses/odbl/1-0/", "ODbL 1.0"),
            Text(", volgens de "),
            Link("https://boss.gouv.fr/portail/accueil/mentions-legales.html", "juridische informatie van het BOSS"),
            Text("."),
        ],
    },
    bulletins: SourceCopy {
        title: "Officiële bulletins van de Franse ministeries",
        body: &[
            &[
                Text("De ministeries publiceren hun omzendbrieven, instructies en nota's op de websites die worden aangewezen door "),
                Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/d312-11", "artikel D. 312-11 van de code des relations entre le public et l'administration"),
                Text(". LibreJustice neemt de officiële bulletins over van de ministeries van Ecologie, Justitie, Defensie, Landbouw, Volksgezondheid en Arbeid, Cultuur, Buitenlandse Zaken, Onderwijs, Hoger Onderwijs, Sport, Binnenlandse Zaken en Economie, alsook de omzendbrieven van het openbaar ambt. Elke tekst wordt overgenomen met zijn datums, zijn NOR-nummer als hij er een heeft en de link naar zijn publicatie. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("vrij herbruikbare overheidsinformatie ("),
            Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/l321-1", "artikel L. 321-1 van de code des relations entre le public et l'administration"),
            Text(")."),
        ],
    },
    acpr: SourceCopy {
        title: "ACPR, officieel register",
        body: &[
            &[
                Text("De Autorité de contrôle prudentiel et de résolution publiceert haar handelingen in haar officiële register. LibreJustice neemt al die handelingen over. De instructies, aanbevelingen, standpunten, toelichtingen en richtsnoeren worden ook bij de regelgeving opgenomen, met hun tekst gelezen uit de gepubliceerde PDF. De gegevens worden dagelijks bijgewerkt."),
            ],
        ],
        reuse: &[
            Text("vrij herbruikbare overheidsinformatie ("),
            Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/l321-1", "artikel L. 321-1 van de code des relations entre le public et l'administration"),
            Text(")."),
        ],
    },
    jafbase: SourceCopy {
        title: "JaFBase, internationaal privaatrecht in familiezaken",
        body: &[
            &[
                Text("Een databank die Cyril Roth, magistraat, sinds 2008 belangeloos bijhoudt, een van de weinige die het buitenlandse familierecht toegankelijk maken. Met toestemming van de auteur neemt LibreJustice er de buitenlandse wetboeken over (familie, staat van de personen, burgerlijk wetboek), per artikel opgesplitst en gekoppeld aan de beslissingen die ze aanhalen. JaFBase bundelt niet alleen teksten: het is ook een methode, een "),
                Link("http://jafbase.fr/instruments", "mindmap"),
                Text(" om de toepasselijke verwijzingsregel te bepalen. Die teksten zijn niet exhaustief en niet gegarandeerd actueel."),
            ],
        ],
        reuse: &[
            Text("overgenomen met toestemming van de auteur."),
        ],
    },
    hal: SourceCopy {
        title: "HAL, open archief",
        body: &[
            &[
                Text("LibreJustice koppelt de annotaties, kronieken en proefschriften in HAL aan de beslissingen die ze bespreken. De documenten blijven op HAL of bij hun uitgever en LibreJustice neemt alleen de link over."),
            ],
        ],
        reuse: &[
            Text("die van de auteur of de uitgever van elk document."),
        ],
    },
    cairn: SourceCopy {
        title: "Cairn",
        body: &[
            &[
                Text("De annotaties in Le Droit Ouvrier en de Revue critique de droit international privé worden gekoppeld aan de beslissingen die ze bespreken, op basis van de metadata die Cairn publiceert. De artikelen blijven op Cairn, sommige alleen voor abonnees, en LibreJustice neemt alleen de link over."),
            ],
        ],
        reuse: &[
            Text("metadata die vrij mogen worden geoogst; voor de artikelen gelden die van Cairn en van elk tijdschrift."),
        ],
    },
    hypotheses: SourceCopy {
        title: "Hypothèses, onderzoeksblogs",
        body: &[
            &[
                Text("De bijdragen van de blogs "),
                Link("https://dsf.hypotheses.org/", "Droit du travail et de la protection sociale"),
                Text(" en "),
                Link("https://m2dsplille.hypotheses.org/", "Droit de la protection sociale"),
                Text(", van de Université de Lille, worden op dezelfde manier gekoppeld, op basis van de metadata die OpenEdition publiceert."),
            ],
        ],
        reuse: &[
            Text("metadata onder licentie CC0; voor de bijdragen gelden die van elke blog."),
        ],
    },
    ajdc: SourceCopy {
        title: "Actualité juridique du dommage corporel",
        body: &[
            &[
                Text("De commentaren in dit tijdschrift van de Université Lumière Lyon 2 worden gekoppeld aan de beslissingen die ze bespreken, op basis van de pagina van elk artikel."),
            ],
        ],
        reuse: &[
            Text("licentie CC BY."),
        ],
    },
    boamp: SourceCopy {
        title: "BOAMP",
        body: &[
            &[
                Text("Aankondigingen van lopende overheidsopdrachten en aankondigingen van gegunde opdrachten uit het Bulletin officiel des annonces des marchés publics, die elke dag worden opgehaald."),
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
                Text("De Franse aankondigingen van TED, het supplement bij het Publicatieblad van de Europese Unie, die elke dag worden opgehaald."),
            ],
        ],
        reuse: &[
            Text("vrij herbruikbaar, ook voor commerciële doeleinden ("),
            Link("https://ted.europa.eu/fr/legal-notice", "voorwaarden van TED"),
            Text(")."),
        ],
    },
    decp: SourceCopy {
        title: "Données essentielles de la commande publique (DECP)",
        body: &[
            &[
                Text("Deze kerngegevens van de Franse overheidsopdrachten vermelden de opdrachtnemer van de gesloten opdrachten."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    buyer_platforms: SourceCopy {
        title: "Platformen van de aanbestedende overheden",
        body: &[
            &[
                Text("LibreJustice volgt ook de elektronische platformen van de aanbestedende overheden, waarop veel opdrachten volgens de procédure adaptée (een Franse vereenvoudigde procedure) verschijnen die niet in het BOAMP staan. Een opdracht die via meerdere kanalen wordt gezien, krijgt één fiche, die verwijst naar het dossier op het platform van de aanbestedende overheid."),
            ],
        ],
        reuse: &[],
    },
    sirene: SourceCopy {
        title: "SIRENE, Insee",
        body: &[
            &[
                Text("Het register van de ondernemingen en hun vestigingen, bijgehouden door het Insee, het Franse statistiekinstituut. LibreJustice neemt er de juridische eenheden over die mogen worden verspreid, zowel vennootschappen als eenmanszaken, en maakt er zijn entiteitsfiches van."),
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
                Text("Dit register, bijgehouden door het Franse ministerie van Binnenlandse Zaken, vermeldt de geregistreerde verenigingen. LibreJustice maakt er een fiche per vereniging van."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    lawyers: SourceCopy {
        title: "Advocatengids, Conseil national des barreaux",
        body: &[
            &[
                Text("De nationale gids die de Conseil national des barreaux publiceert, vermeldt de advocaten die bij de Franse balies zijn ingeschreven. LibreJustice leest hem elke week opnieuw en maakt er een fiche per advocaat van."),
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
                Text("De openbare dienst die de Annuaire des entreprises voedt. Wanneer de fiche van een onderneming wordt getoond, haalt ze daar de actuele identificatiegegevens, de bestuurders en de financiële gegevens op."),
            ],
        ],
        reuse: &[],
    },
    bodacc: SourceCopy {
        title: "BODACC",
        body: &[
            &[
                Text("Het Bulletin officiel des annonces civiles et commerciales, uitgegeven door de DILA, maakt de inschrijvingen, wijzigingen, verkopen en insolventieprocedures van ondernemingen bekend. De fiche van een onderneming toont die bekendmakingen in chronologische volgorde, opgehaald op het moment dat de fiche wordt getoond."),
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
                Text("Het wordt uitgegeven door de DILA en maakt de oprichting, de wijzigingen en de ontbinding van verenigingen bekend. De fiche van een vereniging toont die bekendmakingen."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
};
