use crate::i18n::Block::{Address, List, Para};
use crate::i18n::Run::{Link, Local, Strong, Text};
use crate::i18n::{LegalPage, LegalSection, LegalUi};

pub const LEGAL: LegalUi = LegalUi {
    terms: TERMS,
    privacy: PRIVACY,
    notice: NOTICE,
};

const TERMS: LegalPage = LegalPage {
    title: "Gebruiksvoorwaarden - LibreJustice",
    heading: "Gebruiksvoorwaarden",
    in_force: "Van kracht op 6 november 2026. Vertaling ter informatie; de Franse tekst is bindend.",
    sections: &[
        LegalSection {
            title: "Voorwerp",
            blocks: &[
                Para(&[
                    Text("Deze voorwaarden regelen het gebruik van "),
                    Strong("librejustice.fr"),
                    Text(", van de MCP-connector en van de API ervan (samen \"de dienst\"), uitgegeven door Iris Sainte Fare Garnot (individuele ondernemer, SIRET 922 108 600 00029). Ze worden aanvaard bij het aanmaken van een account en zijn van toepassing op elk gebruik van de dienst. Ze worden beheerst door het Franse recht. Contact: "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Toegang",
            blocks: &[
                Para(&[
                    Text("De dienst is gratis en staat open voor elk gebruik, ook commercieel. Zoeken op de site vereist geen account. De MCP-connector en de API vereisen er wel een. De API wordt gebruikt met een persoonlijke sleutel, aan te maken op de "),
                    Local("/profile", "profielpagina"),
                    Text(". De houder van een account staat in voor het gebruik dat ervan wordt gemaakt. Hij kan het account op elk moment verwijderen."),
                ]),
            ],
        },
        LegalSection {
            title: "Gebruik",
            blocks: &[
                Para(&[
                    Text("Met de dienst kunnen rechterlijke beslissingen, teksten, entiteitsfiches en aankondigingen van overheidsopdrachten worden opgezocht en geraadpleegd. De dienst is niet bedoeld om de reproductie van die inhoud in zijn geheel mogelijk te maken. Elk account beschikt over een volume aan verzoeken dat gemeenschappelijk is voor de site, de MCP-connector en de API, zodat de capaciteit van de dienst gedeeld blijft tussen alle gebruikers. Dit volume mag niet worden omzeild. Hebt u meer nodig, schrijf ons dan. Het is verboden de dienst te verstoren, zich toegang te verschaffen tot gegevens of accounts die niet voor u bestemd zijn of de beveiliging ervan zonder toestemming te testen."),
                ]),
                Para(&[
                    Text("Het is ook verboden de dienst te gebruiken om:"),
                ]),
                List(&[
                    &[
                        Text("een persoon te identificeren die door de bron van de gegevens onherkenbaar is gemaakt of aan de verspreiding is onttrokken;"),
                    ],
                    &[
                        Text("lijsten van personen of verenigingen op te stellen volgens de werkelijke of vermeende afkomst, nationaliteit, godsdienst, seksuele geaardheid, gezondheidstoestand of vakbondslidmaatschap van die personen, van de leden van die verenigingen of van degenen die zij verdedigen;"),
                    ],
                    &[
                        Text("iemand uit te sluiten van een woning of een betrekking wegens de procedures die hem betreffen;"),
                    ],
                    &[
                        Text("te verzamelen wat nodig is om personen, verenigingen, vakbonden of plaatsen van eredienst op te sporen, teneinde ze bloot te stellen aan geweld of vernielingen;"),
                    ],
                    &[
                        Text("de beroepspraktijken van magistraten en leden van de griffie op basis van hun identiteit te evalueren, te analyseren, te vergelijken of te voorspellen, wat strafbaar is gesteld bij de artikelen "),
                        Local("/norm/fr/code-de-l-organisation-judiciaire/l111-13", "L. 111-13 van de code de l'organisation judiciaire"),
                        Text(" en "),
                        Local("/norm/fr/code-de-justice-administrative/l10", "L. 10 van de code de justice administrative"),
                        Text("."),
                    ],
                ]),
            ],
        },
        LegalSection {
            title: "Ingestuurde beslissingen",
            blocks: &[
                Para(&[
                    Text("Door een beslissing in te sturen, verklaart u dat u ze hebt geanonimiseerd en dat u het recht hebt ze te delen. U aanvaardt daarmee ook dat ze vrij wordt gepubliceerd en hergebruikt. De uitgever kan ze weigeren."),
                ]),
            ],
        },
        LegalSection {
            title: "Hergebruik",
            blocks: &[
                Para(&[
                    Text("Een dienst die op LibreJustice steunt om resultaten te leveren aan zijn eigen gebruikers, via de API, de MCP-connector of enig ander middel, vermeldt die herkomst zichtbaar op de plaats waar die resultaten verschijnen. Laat hij voor de toegang tot die resultaten betalen, dan vermeldt hij ook dat ze gratis te raadplegen zijn op librejustice.fr. De gebruiker is als enige verantwoordelijk voor het gebruik van de verkregen gegevens en voor wat hij daarvan aan derden presenteert. De inhoud blijft onderworpen aan de licentie van de bron ervan, vermeld op de pagina "),
                    Local("/sources", "Gegevens & bronnen"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Aansprakelijkheid",
            blocks: &[
                Para(&[
                    Text("De dienst wordt geleverd in de staat waarin hij zich bevindt. De uitgever garandeert noch de juistheid, noch de volledigheid, noch de beschikbaarheid van de gegevens en van de dienst. De verstrekte informatie vormt geen juridisch advies. De persoonsgegevens worden verwerkt onder de voorwaarden van het "),
                    Local("/privacy", "privacybeleid"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Wijziging en opschorting",
            blocks: &[
                Para(&[
                    Text("De uitgever kan de dienst op elk moment en zonder voorafgaande kennisgeving geheel of gedeeltelijk wijzigen, beperken, opschorten of stopzetten. Hij kan eveneens, zonder voorafgaande kennisgeving of vergoeding, een account of een sleutel opschorten of verwijderen waarvan het gebruik in strijd is met deze voorwaarden of de dienst verstoort. De houder ervan krijgt de reden meegedeeld en kan daarop antwoorden. Deze voorwaarden kunnen worden gewijzigd. Van toepassing zijn de voorwaarden die online staan op het moment van het gebruik."),
                ]),
            ],
        },
    ],
};

const PRIVACY: LegalPage = LegalPage {
    title: "Privacy - LibreJustice",
    heading: "Privacybeleid",
    in_force: "Van kracht op 8 november 2026. Vertaling ter informatie; de Franse tekst is bindend.",
    sections: &[
        LegalSection {
            title: "Verwerkingsverantwoordelijke",
            blocks: &[
                Para(&[
                    Text("Iris Sainte Fare Garnot (individuele ondernemer, SIRET 922 108 600 00029), uitgever van "),
                    Strong("librejustice.fr"),
                    Text(". Contact: "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Verzamelde gegevens",
            blocks: &[
                Para(&[
                    Strong("Gebruikersaccount."),
                    Text(" Uw e-mailadres, uw naam als u die hebt opgegeven en uw Google- of Microsoft-ID als u via een van beide inlogt. Wij noteren ook de taal van uw browser, uw land, de datum van uw inschrijving en die van uw laatste bezoek. Als u een AI-assistent aan uw account koppelt, geven wij hem uw e-mailadres door. Wij houden ook bij welke e-mails wij u sturen en of u zich eventueel hebt uitgeschreven. Dat alles wordt gewist wanneer u uw account verwijdert."),
                ]),
                Para(&[
                    Strong("Technische logs."),
                    Text(" De werking van de dienst steunt op applicatielogs en prestatietraces, die dienen om de responstijden te meten, fouten te diagnosticeren, het gebruik van de dienst te volgen en door de gebruiksvoorwaarden verboden gebruik op te sporen. Ze kunnen de parameters van de verzoeken bevatten, waaronder de zoektermen, alsook het gebruikte type client (browser, API of AI-assistent). Deze gegevens zijn gepseudonimiseerd: er staat geen naam of e-mailadres in; de gebruiker komt er alleen in voor onder een afgeleide technische identificatiecode. Ze worden na 30 dagen automatisch verwijderd."),
                ]),
                Para(&[
                    Strong("Bezoekersmeting."),
                    Text(" Om het gebruik van de dienst te volgen en de verdere ontwikkeling ervan te sturen, stellen wij per dag samengevoegde bezoekersindicatoren op: het volume van de zoekopdrachten en raadplegingen, de meest geraadpleegde pagina's, de gebruikte functies en filters, de site waarvandaan de bezoekers komen en de verdeling per toegangskanaal. Deze indicatoren zijn beperkt tot tellingen: ze bewaren noch de gezochte termen, noch de pagina's die een persoon heeft gelezen. Ze maken geen gebruik van cookies en er wordt geen enkel IP-adres geregistreerd."),
                ]),
                Para(&[
                    Strong("Ingestuurde beslissingen."),
                    Text(" Het bestand dat u instuurt en uw account. Het bestand wordt gewist zodra het is nagelezen. Als de beslissing wordt gepubliceerd, blijft uw account aan de herkomst ervan verbonden en wordt het nooit getoond."),
                ]),
                Para(&[
                    Strong("Melding van een probleem."),
                    Text(" De tekst van uw bericht, het adres van de betrokken pagina (met inbegrip van de zoektermen als het die bevat), het feit dat u al dan niet ingelogd was en uw e-mailadres als u het opgeeft of als u ingelogd bent. Die gegevens komen terecht in onze supportmailbox, waar ze worden bewaard voor de opvolging van uw verzoek."),
                ]),
            ],
        },
        LegalSection {
            title: "Cookies en lokale opslag",
            blocks: &[
                Para(&[
                    Text("Wij gebruiken geen advertentiecookies en geen trackers waarvoor toestemming vereist is. Onze bezoekersmeting is zo ingesteld dat ze uw persoonlijke levenssfeer respecteert, overeenkomstig de aanbevelingen van de Franse toezichthouder, de CNIL: daarom toont deze site geen cookiebanner. Als u ingelogd bent, bewaart uw browser een sessietoken dat bij het uitloggen wordt gewist. Een cookie onthoudt het rechtsstelsel en de taal die u kiest. Het tabblad bewaart ook de context van uw zoekopdracht tot het wordt gesloten, om naar de resultaten terug te keren en de gezochte termen in een beslissing te markeren."),
                ]),
            ],
        },
        LegalSection {
            title: "Verwerkers",
            blocks: &[
                List(&[
                    &[
                        Strong("OVH"),
                        Text(" (Roubaix): hosting, databank, telemetrie, e-maildienst."),
                    ],
                    &[
                        Strong("Cloudflare Lda."),
                        Text(" (Lissabon): CDN, bescherming, bezoekersmeting en semantische analyse van de zoekopdrachten. Zijn "),
                        Link("https://developers.cloudflare.com/workers-ai/platform/data-usage/", "voorwaarden voor gegevensgebruik"),
                        Text(" sluiten uit dat zijn modellen op die zoekopdrachten worden getraind."),
                    ],
                    &[
                        Strong("Grafana Cloud"),
                        Text(" (New York, gegevens in Frankfurt): logs, traces en bezoekersmeting."),
                    ],
                    &[
                        Strong("Mistral AI"),
                        Text(" (Parijs): samenvattingen van de beslissingen en herrangschikking van de zoekresultaten. Het trainen van zijn modellen op wat wij naar Mistral AI sturen is uitgeschakeld. Zijn "),
                        Link("https://legal.mistral.ai/terms/privacy-policy/", "privacybeleid"),
                        Text(" bepaalt de bewaartermijn."),
                    ],
                    &[
                        Strong("Resend"),
                        Text(" (San Francisco, verzending via AWS Dublin): service-e-mails (bevestiging, wachtwoordherstel) en informatieve e-mails."),
                    ],
                ]),
            ],
        },
        LegalSection {
            title: "Personen vermeld in de gepubliceerde gegevens",
            blocks: &[
                Para(&[
                    Text("De verstrekte gegevens komen uit officiële publicaties en openbare registers. De beslissingen worden overgenomen zoals hun bron ze publiceert, met de onherkenbaarmaking die die bron erop heeft toegepast."),
                ]),
                Para(&[
                    Text("Een door een gebruiker ingestuurde beslissing is vóór de publicatie door die gebruiker geanonimiseerd."),
                ]),
                Para(&[
                    Text("Om een gegeven dat u betreft onherkenbaar te laten maken of te laten corrigeren, wendt u zich tot het rechtscollege of het register dat het publiceert. Staat het in een door een gebruiker ingestuurde beslissing, schrijf ons dan via "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Uw rechten",
            blocks: &[
                Para(&[
                    Text("Op grond van de Algemene Verordening Gegevensbescherming (AVG) en de Franse loi Informatique et Libertés hebt u recht op inzage, op rectificatie, op gegevenswissing, op beperking van de verwerking, van bezwaar en op overdraagbaarheid van uw gegevens."),
                ]),
                Para(&[
                    Text("De knop \"Mijn account verwijderen\" op uw "),
                    Local("/profile", "profielpagina"),
                    Text(" verwijdert uw account onmiddellijk."),
                ]),
                Para(&[
                    Text("Schrijf voor elk ander verzoek naar "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text(" of wend u tot de "),
                    Link("https://www.cnil.fr/fr/plaintes", "CNIL"),
                    Text("."),
                ]),
            ],
        },
    ],
};

const NOTICE: LegalPage = LegalPage {
    title: "Juridische informatie - LibreJustice",
    heading: "Juridische informatie",
    in_force: "Van kracht op 15 mei 2026. Vertaling ter informatie; de Franse tekst is bindend.",
    sections: &[
        LegalSection {
            title: "Uitgever van de site",
            blocks: &[
                Address(&[
                    &[
                        Strong("Iris Sainte Fare Garnot"),
                        Text(", individuele ondernemer"),
                    ],
                    &[
                        Text("tevens verantwoordelijke uitgever"),
                    ],
                    &[
                        Text("SIRET: 922 108 600 00029"),
                    ],
                    &[
                        Text("E-mail: "),
                        Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    ],
                ]),
            ],
        },
        LegalSection {
            title: "Hosting",
            blocks: &[
                Para(&[
                    Text("Site, zoekmachine, API en databank: "),
                    Strong("OVH SAS"),
                    Text(" (Roubaix)."),
                ]),
                Para(&[
                    Text("Distributienetwerk (CDN) en bescherming: "),
                    Strong("Cloudflare Lda."),
                    Text(" (Lissabon)."),
                ]),
            ],
        },
        LegalSection {
            title: "Gepubliceerde gegevens",
            blocks: &[
                Para(&[
                    Text("De gegevens die LibreJustice verspreidt, komen uit openbare bronnen en uit bronnen waarvan de overname is toegestaan. Elke bron wordt, met haar voorwaarden voor hergebruik, beschreven op de pagina "),
                    Local("/sources", "Gegevens & bronnen"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Intellectuele eigendom",
            blocks: &[
                Para(&[
                    Text("De "),
                    Link("https://github.com/librejustice/librejustice", "code van de site"),
                    Text(" wordt gepubliceerd onder de Apache 2.0-licentie. De componenten van derden waarop hij steunt, bibliotheken, lettertypen en hulpmiddelen voor het lezen van documenten, blijven onder "),
                    Local("/open-source", "hun eigen licenties"),
                    Text(" vallen."),
                ]),
            ],
        },
    ],
};
