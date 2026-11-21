use crate::i18n::Run::{Link, Local, Text};
use crate::i18n::{SourceCopy, SourcesUi};

const LICENCE_OUVERTE: &str = "https://www.etalab.gouv.fr/licence-ouverte-open-licence/";

pub const SOURCES: SourcesUi = SourcesUi {
    title: "Daten & Quellen - LibreJustice",
    heading: "Daten & Quellen",
    intro: "LibreJustice führt öffentliche Rechtsdaten zusammen: Gerichtsentscheide, Erlasse (Gesetzbücher, Gesetze, Staatsverträge, Unionsrecht), Ausschreibungen des öffentlichen Beschaffungswesens und Register. Jede Quelle wird unten mit ihrer Herkunft und ihren Bedingungen für die Weiterverwendung vorgestellt. LibreJustice verändert deren Inhalt nicht und garantiert weder die Vollständigkeit noch die Aktualität des Datenbestands.",
    reuse: "Bedingungen für die Weiterverwendung: ",
    case_law: "Rechtsprechung",
    norms: "Erlasse",
    commentaries: "Entscheidbesprechungen",
    procurement: "Öffentliches Beschaffungswesen",
    registers: "Unternehmen, Vereine, Anwältinnen und Anwälte",
    judilibre: SourceCopy {
        title: "Judilibre, Cour de cassation",
        body: &[
            &[
                Text("Die Cour de cassation stellt die Entscheide der ordentlichen Gerichtsbarkeit (Cour de cassation, Berufungsgerichte, erstinstanzliche Gerichte, Handelsgerichte und Arbeitsgerichte) über die API Judilibre als Open Data bereit; sie werden vor der Veröffentlichung pseudonymisiert. LibreJustice übernimmt den gesamten Datenstrom und gleicht ihn täglich ab. Das Open-Data-Angebot der Justiz wird schrittweise ausgebaut: Die Strafentscheide erster Instanz sollen 2027 folgen ("),
                Link("https://www.justice.gouv.fr/documentation/open-data-decisions-justice", "Zeitplan der Öffnung"),
                Text(")."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text(", ergänzt durch die "),
            Link("https://www.courdecassation.fr/conditions-generales-dutilisation-pour-la-reutilisation-des-donnees-issues-des-decisions-de-justice", "allgemeinen Nutzungsbedingungen"),
            Text(" der Cour de cassation."),
        ],
    },
    administrative_open_data: SourceCopy {
        title: "Open Data der Verwaltungsjustiz, Conseil d'État",
        body: &[
            &[
                Text("Die Entscheide der Verwaltungsgerichtsbarkeit (Conseil d'État, Verwaltungsberufungsgerichte, Verwaltungsgerichte) verbreitet der Conseil d'État als ZIP-Archive im XML-Format, die täglich aktualisiert und vor der Veröffentlichung pseudonymisiert werden. LibreJustice gleicht sie täglich ab. Der Datenstrom liefert den Volltext der Entscheide, aber weder die Regesten noch die Schlussanträge der Rapporteurs publics, die nicht als Open Data verbreitet werden. Die Regesten stammen aus ArianeWeb, das weiter unten beschrieben ist."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text(", ergänzt durch die "),
            Link("https://opendata.justice-administrative.fr/cgu.html", "allgemeinen Nutzungsbedingungen"),
            Text(" des Conseil d'État."),
        ],
    },
    arianeweb: SourceCopy {
        title: "ArianeWeb, Conseil d'État",
        body: &[
            &[
                Text("ArianeWeb ist die Rechtsprechungsdatenbank des Conseil d'État. LibreJustice übernimmt daraus die Regesten, die zu seinen Entscheiden verfasst werden (Bestand AJCE), die Angabe, ob Schlussanträge des Rapporteur public vorliegen, und einige Entscheide, die in den anderen Kanälen fehlen. LibreJustice ordnet sie den Entscheiden zu und gleicht sie täglich ab. Die vom Conseil d'État veröffentlichte Liste der Leitentscheide liefert zudem den Namen der darin aufgeführten Entscheide."),
            ],
        ],
        reuse: &[
            Text("frei weiterverwendbare öffentliche Informationen, sofern sie nicht verändert und Quelle und Datum angegeben werden, gemäss dem "),
            Link("https://www.conseil-etat.fr/mentions-legales", "Impressum des Conseil d'État"),
            Text("."),
        ],
    },
    consiliaweb: SourceCopy {
        title: "ConsiliaWeb, Conseil d'État",
        body: &[
            &[
                Text("ConsiliaWeb ist die Datenbank der Gutachten, die der Conseil d'État der Regierung und dem Parlament zu Gesetzesentwürfen, Dekreten und den ihm vorgelegten Fragen erstattet. LibreJustice übernimmt täglich die Gutachten, welche die Datenbank veröffentlicht, mit dem Text ihres Originaldokuments und führt sie unter dem Conseil d'État in beratender Besetzung."),
            ],
        ],
        reuse: &[
            Text("frei weiterverwendbare öffentliche Informationen, sofern sie nicht verändert werden, ihr Datum angegeben und die Website des Conseil d'État als Quelle genannt wird, gemäss dem "),
            Link("https://www.conseil-etat.fr/bas-de-page/mentions-legales", "Impressum des Conseil d'État"),
            Text("."),
        ],
    },
    administrative_court_sites: SourceCopy {
        title: "Websites der Verwaltungsgerichte und der Verwaltungsberufungsgerichte",
        body: &[
            &[
                Text("Jedes Verwaltungsgericht und jedes Verwaltungsberufungsgericht veröffentlicht auf seiner Website unter «Dernières décisions» Entscheide, die erst später oder gar nicht ins Open-Data-Angebot gelangen. LibreJustice übernimmt jeden als PDF beigefügten Entscheid so, wie ihn das Gericht veröffentlicht hat. Sobald das Open-Data-Angebot denselben Entscheid enthält, tritt dessen Text an die Stelle des PDF. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Text("Gerichtsentscheide, die das Gericht gemäss Artikel L. 10 des Code de justice administrative öffentlich zugänglich macht."),
        ],
    },
    constitutional_council_site: SourceCopy {
        title: "Website des Conseil constitutionnel",
        body: &[
            &[
                Text("Der Conseil constitutionnel veröffentlicht seine Entscheide auf seiner Website mehrere Wochen, bevor sie in den Bestand CONSTIT der DILA gelangen. LibreJustice übernimmt täglich die Entscheide, welche die Website ankündigt und die dieser Bestand noch nicht enthält. Sobald die DILA denselben Entscheid verbreitet, tritt ihr Text an die Stelle des bisherigen."),
            ],
        ],
        reuse: &[
            Text("Texte, die frei wiedergegeben werden dürfen, sofern die Website als Quelle genannt und ihre Integrität gewahrt wird, gemäss dem "),
            Link("https://www.conseil-constitutionnel.fr/mentions-legales", "Impressum des Conseil constitutionnel"),
            Text("."),
        ],
    },
    cnda: SourceCopy {
        title: "CNDA, Cour nationale du droit d'asile",
        body: &[
            &[
                Text("Die Entscheide der CNDA sind weder in Judilibre noch im Open-Data-Angebot der Verwaltungsjustiz enthalten. LibreJustice übernimmt jene, die das Gericht auf seiner Website veröffentlicht, samt der Regeste, die es dazu verfasst. Der Volltext wird aus dem beigefügten Dokument extrahiert. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Text("jene der Website der CNDA, festgelegt in ihrem "),
            Link("https://www.cnda.fr/mentions-legales", "Impressum"),
            Text("."),
        ],
    },
    ddd: SourceCopy {
        title: "Défenseur des droits",
        body: &[
            &[
                Text("Der Défenseur des droits veröffentlicht in seiner juristischen Datenbank seine Entscheide und jene der Behörden, deren Nachfolge er 2011 angetreten hat: die HALDE, die CNDS, der Défenseur des enfants und der Médiateur de la République. LibreJustice übernimmt daraus die Entscheide mit ihrer Kurzbeschreibung. Der Volltext wird aus dem beigefügten Dokument extrahiert. Die Übernahme erfolgt täglich."),
            ],
            &[
                Text("Dort veröffentlicht er auch die Urteile in Verfahren, an denen er sich beteiligt hat, von ihm selbst pseudonymisiert. LibreJustice übernimmt jene der französischen erstinstanzlichen Gerichte und der Berufungsgerichte, ausser in Verfahren, die eine minderjährige Person betreffen, und ordnet sie dem Entscheid zu, wenn dieser bereits aus einer anderen Quelle bekannt ist."),
            ],
        ],
        reuse: &[
            Text("frei weiterverwendbare öffentliche Informationen im Sinne des Code des relations entre le public et l'administration, gemäss dem "),
            Link("https://www.defenseurdesdroits.fr/mentions-legales-367", "Impressum des Défenseur des droits"),
            Text("."),
        ],
    },
    adlc: SourceCopy {
        title: "Autorité de la concurrence",
        body: &[
            &[
                Text("Die Autorité de la concurrence veröffentlicht auf data.gouv.fr den Volltext ihrer Entscheide und jener des Conseil de la concurrence seit 1988. LibreJustice übernimmt daraus die Entscheide in streitigen Verfahren, die vorsorglichen Massnahmen und die Entscheide der Zusammenschlusskontrolle. Die Gutachten werden nicht übernommen. Die Übernahme erfolgt wöchentlich, im Rhythmus des Datensatzes."),
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
                Text("Die Entscheide der Commission des sanctions der AMF sind in keinem anderen Kanal verfügbar. LibreJustice übernimmt jene, welche die Behörde auf ihrer Website veröffentlicht. Der Volltext wird aus dem beigefügten Dokument extrahiert. Die Übernahme erfolgt täglich."),
            ],
            &[
                Text("Seit 2017 kann ein Entscheid festlegen, wie lange er ohne Anonymisierung online bleibt. LibreJustice entfernt ihn vor diesem Zeitpunkt und übernimmt danach die anonymisierte Fassung, welche die Behörde anschliessend veröffentlicht."),
            ],
        ],
        reuse: &[
            Text("Nutzung zu persönlichen, Bildungs-, Vereins- oder beruflichen Zwecken, unter Ausschluss jeder kommerziellen oder werblichen Nutzung und ohne Inhalt oder Tragweite der Entscheide zu verändern, gemäss dem "),
            Link("https://www.amf-france.org/fr/mentions-legales-et-editoriales", "Impressum und den redaktionellen Hinweisen der AMF"),
            Text(". Quelle: Autorité des marchés financiers, amf-france.org, täglich aktualisiert. ©Autorité des marchés financiers, alle Rechte vorbehalten."),
        ],
    },
    dila_case_law: SourceCopy {
        title: "DILA, Rechtsprechungsbestände",
        body: &[
            &[
                Text("Die Direction de l'information légale et administrative verbreitet auch Bestände von Entscheiden. LibreJustice übernimmt daraus JADE (Verwaltungsrechtsprechung, darunter der Conseil d'État mit seinen Regesten), CONSTIT (Entscheide des Conseil constitutionnel), CNIL (Beschlüsse der CNIL, darunter ihre Sanktionen) sowie die Bestände CASS, INCA und CAPP der ordentlichen Gerichtsbarkeit. JADE, CONSTIT und CNIL werden täglich abgeglichen. Ein Entscheid, der über mehrere Kanäle eingeht, hat nur einen Eintrag."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    hudoc: SourceCopy {
        title: "HUDOC, Europäischer Gerichtshof für Menschenrechte",
        body: &[
            &[
                Text("Der Europäische Gerichtshof für Menschenrechte verbreitet seine Urteile und Entscheide in seiner Datenbank HUDOC. LibreJustice übernimmt daraus die französische und die englische Fassung und gleicht sie täglich ab."),
            ],
        ],
        reuse: &[
            Text("Der Gerichtshof erlaubt die Wiedergabe seiner Texte mit Quellenangabe, unentgeltlich und zu privaten, Informations- und Bildungszwecken. Jede andere Nutzung bedarf seiner schriftlichen Genehmigung ("),
            Link("https://www.echr.coe.int/fr/copyright-and-disclaimer", "Bedingungen des Gerichtshofs"),
            Text(")."),
        ],
    },
    contribution: SourceCopy {
        title: "Von Nutzerinnen und Nutzern eingereichte Entscheide",
        body: &[
            &[
                Text("Über die Seite "),
                Local("/decision/contribute", "Entscheid einreichen"),
                Text(" gelangen Entscheide in den Bestand, die keine offene Quelle verbreitet. Jeder davon wird vor der Veröffentlichung anonymisiert und geprüft."),
            ],
        ],
        reuse: &[],
    },
    dila: SourceCopy {
        title: "DILA, Rechtsdatenbanken des Staates",
        body: &[
            &[
                Text("Die Direction de l'information légale et administrative verbreitet die Datenbanken des französischen Rechts als Open Data. LibreJustice übernimmt daraus LEGI (konsolidierte Gesetzbücher und Gesetze, artikelweise versioniert: Jeder Artikel lässt sich in der geltenden Fassung oder zu einem früheren Datum lesen), das JORF (Journal officiel, darunter die durch Dekret veröffentlichten Staatsverträge), KALI (nationale Gesamtarbeitsverträge) und CIRCULAIRES (Kreisschreiben und Weisungen der Ministerien). Diese Bestände werden täglich anhand der Änderungslieferungen der DILA abgeglichen."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    eur_lex: SourceCopy {
        title: "EUR-Lex, Unionsrecht und EuGH",
        body: &[
            &[
                Text("Das Amt für Veröffentlichungen der Europäischen Union verbreitet das Unionsrecht und die Rechtsprechung der Unionsgerichte. LibreJustice übernimmt daraus die Verordnungen und Richtlinien und, bei konsolidierten Rechtsakten, deren aufeinanderfolgende Fassungen: Jeder Artikel lässt sich zum gewünschten Datum lesen. Zudem übernimmt LibreJustice die Urteile, Beschlüsse und Schlussanträge der Generalanwältinnen und Generalanwälte des Gerichtshofs der Europäischen Union, in der Verfahrenssprache, auf Französisch und auf Englisch. Beide Bestände werden täglich abgeglichen."),
            ],
        ],
        reuse: &[
            Text("freie Weiterverwendung, auch zu kommerziellen Zwecken, gemäss dem "),
            Link("https://eur-lex.europa.eu/content/legal-notice/legal-notice.html", "rechtlichen Hinweis von EUR-Lex"),
            Text(". Die konsolidierten Fassungen stehen unter der Lizenz CC BY 4.0."),
        ],
    },
    curia: SourceCopy {
        title: "Curia, Website des Gerichtshofs der Europäischen Union",
        body: &[
            &[
                Text("Der Gerichtshof der Europäischen Union veröffentlicht auf seiner Website den Volltext seiner Entscheide. LibreJustice übernimmt dort jene, die EUR-Lex nicht verbreitet, sowie jene, zu denen EUR-Lex nur die Zusammenfassung liefert, in denselben Sprachen."),
            ],
        ],
        reuse: &[
            Text("Wiedergabe mit Quellenangabe gestattet, gemäss der "),
            Link("https://curia.europa.eu/site/jcms/d2_5167/fr/copyright", "Urheberrechtsseite des Gerichtshofs"),
            Text("."),
        ],
    },
    comeu: SourceCopy {
        title: "Europäische Kommission, Wettbewerbsfälle",
        body: &[
            &[
                Text("Die Europäische Kommission veröffentlicht ihre Entscheide zu Kartellen und zum Missbrauch einer marktbeherrschenden Stellung, zu Zusammenschlüssen, staatlichen Beihilfen, digitalen Märkten und drittstaatlichen Subventionen. LibreJustice übernimmt jeden veröffentlichten Entscheid mit seinem Dokument, in der Originalsprache, auf Französisch und auf Englisch. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/licenses/by/4.0/deed.fr", "CC BY 4.0"),
            Text(", gemäss dem "),
            Link("https://commission.europa.eu/legal-notice_fr", "rechtlichen Hinweis der Kommission"),
            Text("."),
        ],
    },
    juricaf: SourceCopy {
        title: "JuriCAF, Association des hautes juridictions de cassation francophones",
        body: &[
            &[
                Text("JuriCAF ist die Rechtsprechungsdatenbank der AHJUCAF, die von den angeschlossenen obersten Gerichten gespeist wird. LibreJustice übernimmt daraus die Entscheide, die keine offizielle Quelle online verbreitet: oberste Gerichte von rund dreissig Ländern sowie regionale afrikanische Gerichte (OHADA, UEMOA, CEMAC, CEDEAO, Afrikanischer Gerichtshof für Menschenrechte und Rechte der Völker). Die Übernahme erfolgt täglich."),
            ],
            &[
                Text("Liefert JuriCAF zu einem Entscheid nur die Referenz, übernimmt LibreJustice den Text aus einer Kopie des Entscheids, wie ihn das Gericht gefällt hat. Die Urteile der Cour suprême de Madagascar und die marokkanischen Entscheide in arabischer Sprache, die jurisprudence.ma veröffentlicht hat, stammen aus Kopien, die das Internet Archive aufbewahrt. Die Urteile der Gerichte von Burkina Faso und Kongo werden so übernommen, wie das jeweilige Gericht sie gefällt hat."),
            ],
        ],
        reuse: &[
            Link("https://opendatacommons.org/licenses/odbl/1-0/", "ODbL 1.0"),
            Text(", gemäss dem "),
            Link("https://juricaf.org/static/donneespersonnelles", "Impressum von JuriCAF"),
            Text("."),
        ],
    },
    entscheidsuche: SourceCopy {
        title: "entscheidsuche.ch, Schweizer Rechtsprechung",
        body: &[
            &[
                Text("entscheidsuche.ch ist ein gemeinnütziger Schweizer Verein, der täglich die von den eidgenössischen und kantonalen Gerichten veröffentlichten Entscheide zusammenführt. LibreJustice übernimmt daraus die Entscheide der Gerichte und Verwaltungsbehörden auf Deutsch, Französisch und Italienisch: Bundesgericht, Bundesverwaltungsgericht, Bundesstrafgericht, kantonale Gerichte, Bundesbehörden und kantonale Behörden."),
            ],
        ],
        reuse: &[
            Text("Gerichtsentscheide sind nach schweizerischem Urheberrecht nicht geschützt (Art. 5 URG); entscheidsuche.ch verbreitet sie ohne Lizenz."),
        ],
    },
    opencaselaw: SourceCopy {
        title: "OpenCaseLaw, Obwaldner Rechtsprechung (Schweiz)",
        body: &[
            &[
                Text("OpenCaseLaw veröffentlicht auf Hugging Face Entscheide von Schweizer Gerichten und Behörden als offene Daten. LibreJustice übernimmt daraus die deutschsprachigen Entscheide des Kantons Obwalden aus dessen amtlichen Sammlungen: Obergericht, Verwaltungsgericht, Regierungsrat, Departemente und Kommissionen des Kantons. Die Übernahme erfolgt wöchentlich."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/publicdomain/zero/1.0/deed.fr", "CC0 1.0"),
            Text(", gemäss der "),
            Link("https://huggingface.co/datasets/voilaj/swiss-caselaw", "Beschreibung des Datensatzes"),
            Text("."),
        ],
    },
    legimonaco: SourceCopy {
        title: "Légimonaco, monegassische Rechtsprechung",
        body: &[
            &[
                Text("Légimonaco ist die offizielle Website für die Gesetzgebung und die Rechtsprechung Monacos. LibreJustice übernimmt daraus die Gerichtsentscheide, die anonymisiert veröffentlicht werden: Tribunal suprême, Cour de révision, Cour d'appel, Tribunal de première instance, Tribunal du travail und weitere Gerichte des Fürstentums."),
            ],
        ],
        reuse: &[],
    },
    juportal: SourceCopy {
        title: "Juportal, belgische Rechtsprechung",
        body: &[
            &[
                Text("Juportal ist die öffentliche Datenbank der belgischen Rechtsprechung, betrieben vom FÖD Justiz. LibreJustice übernimmt daraus die auf Französisch ergangenen Entscheide: Kassationshof, Verfassungsgerichtshof, Staatsrat, Appellationshöfe, Arbeitsgerichtshöfe und Gerichte. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Text("Amtliche Akte der Behörden sind nicht urheberrechtlich geschützt (Art. XI.172 § 2 des belgischen Wirtschaftsgesetzbuches)."),
        ],
    },
    rechtspraak: SourceCopy {
        title: "Rechtspraak.nl, niederländische Rechtsprechung",
        body: &[
            &[
                Text("Rechtspraak.nl ist die Website der niederländischen Justiz, die die pseudonymisierten Entscheide der Gerichte als offene Daten veröffentlicht. LibreJustice übernimmt daraus die Entscheide des Hoge Raad, des Raad van State, der Centrale Raad van Beroep, des College van Beroep voor het bedrijfsleven, der Berufungsgerichte und der Bezirksgerichte. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Text("Gerichtsentscheide sind nicht urheberrechtlich geschützt (Art. 11 des niederländischen Urheberrechtsgesetzes, Auteurswet). Der Datensatz ist unter der Public Domain Mark 1.0 veröffentlicht."),
        ],
    },
    cce: SourceCopy {
        title: "CCE, Rat für Ausländerstreitsachen (Belgien)",
        body: &[
            &[
                Text("Der Rat für Ausländerstreitsachen ist das belgische Verwaltungsgericht für das Aufenthalts- und Asylrecht der Ausländerinnen und Ausländer. Er veröffentlicht seine Urteile anonymisiert auf seiner Website. LibreJustice übernimmt daraus die Urteile auf Französisch, Niederländisch und Deutsch, deren Text aus dem veröffentlichten PDF extrahiert wird. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Text("Amtliche Akte der Behörden sind nicht urheberrechtlich geschützt (Art. XI.172 § 2 des belgischen Wirtschaftsgesetzbuches)."),
        ],
    },
    justice_lu: SourceCopy {
        title: "data.public.lu, luxemburgische Rechtsprechung",
        body: &[
            &[
                Text("Die Justizverwaltung des Grossherzogtums Luxemburg veröffentlicht wöchentlich auf data.public.lu, dem luxemburgischen Portal für offene Daten, die pseudonymisierten Entscheide der ordentlichen Gerichtsbarkeit. LibreJustice übernimmt daraus die Entscheide der Cour de cassation, der Cour d'appel, der Bezirksgerichte Luxemburg und Diekirch, der Friedensgerichte und des Conseil supérieur de la sécurité sociale. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Link("https://creativecommons.org/licenses/by-nd/4.0/deed.fr", "CC BY-ND 4.0"),
            Text(", gemäss der Beschreibung des jeweiligen Datensatzes, zum Beispiel jenes der "),
            Link("https://data.public.lu/fr/datasets/cour-de-cassation-1/", "Cour de cassation"),
            Text("."),
        ],
    },
    supreme_court_canada: SourceCopy {
        title: "Cour suprême du Canada",
        body: &[
            &[
                Text("Die Website der Cour suprême du Canada verbreitet deren seit 1970 gefällte Urteile auf Französisch und Englisch. LibreJustice übernimmt daraus die französische Fassung. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Text("Das Dekret über die Vervielfältigung der Bundesgesetzgebung und der Entscheide der vom Bund errichteten Gerichte (TR/97-5) erlaubt ihre Wiedergabe ohne Kosten und ohne Genehmigung, sofern deren Richtigkeit gewährleistet ist und die Wiedergabe nicht als amtliche Fassung ausgegeben wird."),
        ],
    },
    a2aj: SourceCopy {
        title: "A2AJ, kanadische Rechtsprechung",
        body: &[
            &[
                Text("Das Projekt A2AJ (Access to Algorithmic Justice) der York University und der Toronto Metropolitan University veröffentlicht auf Hugging Face die Entscheide kanadischer Gerichte, die es auf deren Websites sammelt, als offene Daten. LibreJustice übernimmt daraus auf Französisch und Englisch die Entscheide der Cour suprême du Canada, der Cour d'appel fédérale, der Cour fédérale, der Cour canadienne de l'impôt, der Cour d'appel de la cour martiale du Canada und von Verwaltungsgerichten des Bundes, darunter das Tribunal de la sécurité sociale, die Commission de l'immigration et du statut de réfugié und das Tribunal canadien du commerce extérieur. Hinzu kommen auf Englisch die Entscheide des Court of Appeal for Ontario und von achtzehn Verwaltungsgerichten Ontarios, darunter das Landlord and Tenant Board und das Human Rights Tribunal, des Court of Appeal und des Supreme Court von British Columbia, des Court of Appeal von Yukon, der fünf Gerichte von Nova Scotia und des Court of Appeal von Neufundland und Labrador sowie die vom Refugee Law Lab veröffentlichten Entscheide der Refugee Protection Division. Die Übernahme erfolgt wöchentlich."),
            ],
        ],
        reuse: &[
            Text("Das Dekret über die Vervielfältigung der Bundesgesetzgebung und der Entscheide der vom Bund errichteten Gerichte (TR/97-5) erlaubt ihre Wiedergabe ohne Kosten und ohne Genehmigung, sofern deren Richtigkeit gewährleistet ist und die Wiedergabe nicht als amtliche Fassung ausgegeben wird. Die Gerichte von Nova Scotia geben an, dass das Urheberrecht auf ihre veröffentlichten Entscheide nicht anwendbar ist, und jene von Neufundland und Labrador erlauben deren freie Wiedergabe. Die Arbeit von A2AJ steht unter der MIT-Lizenz, gemäss der "),
            Link("https://huggingface.co/datasets/a2aj/canadian-case-law", "Beschreibung des Datensatzes"),
            Text("."),
        ],
    },
    african_court_sites: SourceCopy {
        title: "Websites afrikanischer Gerichte",
        body: &[
            &[
                Text("Mehrere Gerichte veröffentlichen ihre Entscheide auf ihrer eigenen Website: die "),
                Link("https://courconstitutionnelle.bj/fr/decisions", "Cour constitutionnelle du Bénin"),
                Text(", der "),
                Link("https://www.conseil-constitutionnel.ci/decisions", "Conseil constitutionnel de Côte d'Ivoire"),
                Text(", der "),
                Link("https://www.conseil-constitutionnel.dj/decisions/", "Conseil constitutionnel de Djibouti"),
                Text(", die "),
                Link("https://www.hcc.gov.mg/", "Haute Cour constitutionnelle de Madagascar"),
                Text(", die "),
                Link("https://jurisprudence.justice.gov.mg/", "Cour de cassation de Madagascar"),
                Text(" und der "),
                Link("https://www.african-court.org/ajudata/latest-decisions/judgments", "Afrikanische Gerichtshof für Menschenrechte und Rechte der Völker"),
                Text(". LibreJustice übernimmt daraus die auf Französisch ergangenen Entscheide so, wie das jeweilige Gericht sie veröffentlicht hat."),
            ],
        ],
        reuse: &[],
    },
    djibouti_journal: SourceCopy {
        title: "Amtsblatt der Republik Dschibuti",
        body: &[
            &[
                Text("Das "),
                Link("https://www.journalofficiel.dj/", "Amtsblatt der Republik Dschibuti"),
                Text(" veröffentlicht seine seit 1900 erschienenen Ausgaben online. LibreJustice übernimmt jeden Text mit der Ausgabe, in der er erschienen ist, und ordnet die Entscheide des Conseil constitutionnel der Rechtsprechung zu. LibreJustice gleicht es täglich ab."),
            ],
        ],
        reuse: &[],
    },
    legalis: SourceCopy {
        title: "Legalis, Digitalrecht",
        body: &[
            &[
                Text("Legalis veröffentlicht seit 1995 Gerichtsentscheide zum Digitalrecht, die es selbst pseudonymisiert. LibreJustice übernimmt daraus den Text der Entscheide und ordnet ihn dem Entscheid zu, wenn dieser bereits aus einer anderen Quelle bekannt ist. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[],
    },
    bofip: SourceCopy {
        title: "BOFiP-Impôts, steuerliche Verwaltungspraxis",
        body: &[
            &[
                Text("Das Bulletin officiel des finances publiques enthält die steuerliche Verwaltungspraxis der DGFiP, die für die Verwaltung verbindlich ist. LibreJustice übernimmt die geltenden Publikationen, die das Wirtschaftsministerium als Open Data verbreitet."),
            ],
        ],
        reuse: &[
            Link("https://data.economie.gouv.fr/explore/dataset/bofip-vigueur/information/", "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    boss: SourceCopy {
        title: "BOSS, Verwaltungspraxis zu den Sozialabgaben",
        body: &[
            &[
                Text("Das Bulletin officiel de la sécurité sociale enthält die Verwaltungspraxis zu den Sozialversicherungsbeiträgen und Sozialabgaben, die für die Verwaltung verbindlich ist. LibreJustice übernimmt daraus jede Rubrik, Randziffer für Randziffer, mit den früheren Fassungen, welche die Website aufbewahrt: Jede Randziffer lässt sich in der geltenden Fassung oder zu einem früheren Datum lesen. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Link("https://opendatacommons.org/licenses/odbl/1-0/", "ODbL 1.0"),
            Text(", gemäss dem "),
            Link("https://boss.gouv.fr/portail/accueil/mentions-legales.html", "Impressum des BOSS"),
            Text("."),
        ],
    },
    bulletins: SourceCopy {
        title: "Amtsblätter der Ministerien",
        body: &[
            &[
                Text("Die Ministerien veröffentlichen ihre Kreisschreiben, Weisungen und Mitteilungen auf den Websites, die "),
                Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/d312-11", "Artikel D. 312-11 des Code des relations entre le public et l'administration"),
                Text(" bezeichnet. LibreJustice übernimmt die Amtsblätter der Ministerien für Ökologie, Justiz, Streitkräfte, Landwirtschaft, Gesundheit und Arbeit, Kultur, auswärtige Angelegenheiten, Bildung, Hochschulwesen, Sport, Inneres und Wirtschaft sowie die Kreisschreiben des öffentlichen Dienstes. Jeder Text wird mit seinen Daten, seiner NOR-Nummer, falls vorhanden, und dem Link zu seiner Publikation übernommen. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Text("frei weiterverwendbare öffentliche Informationen ("),
            Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/l321-1", "Artikel L. 321-1 des Code des relations entre le public et l'administration"),
            Text(")."),
        ],
    },
    acpr: SourceCopy {
        title: "ACPR, amtliches Register",
        body: &[
            &[
                Text("Die Autorité de contrôle prudentiel et de résolution veröffentlicht ihre Rechtsakte in ihrem amtlichen Register. LibreJustice übernimmt jeden dieser Rechtsakte; die Weisungen, Empfehlungen, Stellungnahmen, Merkblätter und Leitlinien erscheinen zudem unter den Erlassen, wobei ihr Text aus dem veröffentlichten PDF gelesen wird. Die Übernahme erfolgt täglich."),
            ],
        ],
        reuse: &[
            Text("frei weiterverwendbare öffentliche Informationen ("),
            Local("/norm/fr/code-des-relations-entre-le-public-et-l-administration/l321-1", "Artikel L. 321-1 des Code des relations entre le public et l'administration"),
            Text(")."),
        ],
    },
    jafbase: SourceCopy {
        title: "JaFBase, internationales Familienrecht",
        body: &[
            &[
                Text("JaFBase wird seit 2008 von Cyril Roth, Magistrat, ehrenamtlich geführt und ist eine der wenigen Datenbanken, die ausländisches Familienrecht zugänglich machen. Mit Genehmigung des Autors übernimmt LibreJustice daraus die ausländischen Gesetzbücher (Familienrecht, Personenstand, Zivilgesetzbuch), Artikel für Artikel aufgeteilt und mit den Entscheiden verknüpft, die sie zitieren. JaFBase ist nicht nur eine Textsammlung, sondern auch eine Methode, eine "),
                Link("http://jafbase.fr/instruments", "Mindmap"),
                Text(" zur Bestimmung der anwendbaren Kollisionsnorm. Diese Texte sind weder vollständig noch garantiert aktuell."),
            ],
        ],
        reuse: &[
            Text("Übernahme mit Genehmigung des Autors."),
        ],
    },
    hal: SourceCopy {
        title: "HAL, offenes Archiv",
        body: &[
            &[
                Text("LibreJustice verknüpft die Entscheide mit den in HAL hinterlegten Entscheidbesprechungen, Rechtsprechungsübersichten und Dissertationen, die sie kommentieren. Die Dokumente bleiben auf HAL oder beim Verlag, und LibreJustice übernimmt nur den Link."),
            ],
        ],
        reuse: &[
            Text("jene, welche die Urheberschaft oder der Verlag des jeweiligen Dokuments festlegt."),
        ],
    },
    cairn: SourceCopy {
        title: "Cairn",
        body: &[
            &[
                Text("Die Entscheidbesprechungen in Droit Ouvrier und in der Revue critique de droit international privé werden anhand der Metadaten, die Cairn veröffentlicht, mit den besprochenen Entscheiden verknüpft. Die Artikel bleiben auf Cairn, einige davon nur für Abonnentinnen und Abonnenten zugänglich, und LibreJustice übernimmt nur den Link."),
            ],
        ],
        reuse: &[
            Text("Metadaten zum freien Harvesting; für die Artikel die Bedingungen von Cairn und der jeweiligen Zeitschrift."),
        ],
    },
    hypotheses: SourceCopy {
        title: "Hypothèses, Forschungsblogs",
        body: &[
            &[
                Text("Die Beiträge der Blogs "),
                Link("https://dsf.hypotheses.org/", "Droit du travail et de la protection sociale"),
                Text(" und "),
                Link("https://m2dsplille.hypotheses.org/", "Droit de la protection sociale"),
                Text(" der Universität Lille werden auf dieselbe Weise verknüpft, anhand der Metadaten, die OpenEdition veröffentlicht."),
            ],
        ],
        reuse: &[
            Text("Metadaten unter der Lizenz CC0; für die Beiträge die Bedingungen des jeweiligen Blogs."),
        ],
    },
    ajdc: SourceCopy {
        title: "Actualité juridique du dommage corporel",
        body: &[
            &[
                Text("Die Kommentare dieser Zeitschrift der Universität Lumière Lyon 2 werden anhand der Seite des jeweiligen Artikels mit den Entscheiden verknüpft, die sie besprechen."),
            ],
        ],
        reuse: &[
            Text("Lizenz CC BY."),
        ],
    },
    boamp: SourceCopy {
        title: "BOAMP",
        body: &[
            &[
                Text("Die laufenden Ausschreibungen und die Zuschläge des Bulletin officiel des annonces des marchés publics werden täglich erfasst."),
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
                Text("Die französischen Publikationen von TED, dem Supplement zum Amtsblatt der Europäischen Union, werden täglich erfasst."),
            ],
        ],
        reuse: &[
            Text("frei weiterverwendbar, auch zu kommerziellen Zwecken ("),
            Link("https://ted.europa.eu/fr/legal-notice", "Bedingungen von TED"),
            Text(")."),
        ],
    },
    decp: SourceCopy {
        title: "Wesentliche Daten der öffentlichen Beschaffung (DECP)",
        body: &[
            &[
                Text("Sie geben an, an wen die abgeschlossenen Aufträge vergeben wurden."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    buyer_platforms: SourceCopy {
        title: "Plattformen der Auftraggeber",
        body: &[
            &[
                Text("LibreJustice erfasst auch die elektronischen Beschaffungsplattformen der Auftraggeber, auf denen viele Aufträge im vereinfachten Verfahren erscheinen, die im BOAMP fehlen. Eine Ausschreibung, die über mehrere Kanäle erscheint, ergibt einen einzigen Eintrag, der auf die Unterlagen auf der Plattform des Auftraggebers verweist."),
            ],
        ],
        reuse: &[],
    },
    sirene: SourceCopy {
        title: "SIRENE, Insee",
        body: &[
            &[
                Text("Das Insee führt das Verzeichnis der Unternehmen und ihrer Betriebe. LibreJustice übernimmt daraus die zur Veröffentlichung freigegebenen Rechtseinheiten, Gesellschaften wie Einzelunternehmen, und erstellt daraus seine Unternehmensseiten."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte 2.0"),
            Text("."),
        ],
    },
    rna: SourceCopy {
        title: "Nationales Vereinsregister (RNA)",
        body: &[
            &[
                Text("Das vom Innenministerium geführte Register verzeichnet die angemeldeten Vereine. LibreJustice erstellt daraus eine Seite pro Verein."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
    lawyers: SourceCopy {
        title: "Anwaltsverzeichnis, Conseil national des barreaux",
        body: &[
            &[
                Text("Das vom Conseil national des barreaux veröffentlichte nationale Verzeichnis führt die Anwältinnen und Anwälte auf, die bei den französischen Anwaltskammern eingetragen sind. LibreJustice liest es wöchentlich neu ein und erstellt daraus eine Seite pro Anwältin oder Anwalt."),
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
                Text("Diese staatliche Schnittstelle speist das Annuaire des entreprises. Beim Aufruf liest die Seite eines Unternehmens dort dessen aktuelle Identitätsdaten, seine Leitungsorgane und seine Finanzdaten ab."),
            ],
        ],
        reuse: &[],
    },
    bodacc: SourceCopy {
        title: "BODACC",
        body: &[
            &[
                Text("Das von der DILA herausgegebene Bulletin officiel des annonces civiles et commerciales macht die Eintragungen, Änderungen, Verkäufe und Insolvenzverfahren der Unternehmen bekannt. Die Seite eines Unternehmens zeigt diese Meldungen in zeitlicher Abfolge, abgerufen im Moment der Anzeige."),
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
                Text("Es wird von der DILA herausgegeben und macht die Gründung, die Änderungen und die Auflösung von Vereinen bekannt. Die Seite eines Vereins zeigt dessen Meldungen an."),
            ],
        ],
        reuse: &[
            Link(LICENCE_OUVERTE, "Licence Ouverte"),
            Text("."),
        ],
    },
};
