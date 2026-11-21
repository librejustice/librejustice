use crate::i18n::Block::{Address, List, Para};
use crate::i18n::Run::{Link, Local, Strong, Text};
use crate::i18n::{LegalPage, LegalSection, LegalUi};

pub const LEGAL: LegalUi = LegalUi {
    terms: TERMS,
    privacy: PRIVACY,
    notice: NOTICE,
};

const TERMS: LegalPage = LegalPage {
    title: "Nutzungsbedingungen - LibreJustice",
    heading: "Nutzungsbedingungen",
    in_force: "In Kraft seit dem 6. November 2026",
    sections: &[
        LegalSection {
            title: "Gegenstand",
            blocks: &[
                Para(&[
                    Text("Diese Bedingungen regeln die Nutzung von "),
                    Strong("librejustice.fr"),
                    Text(", des zugehörigen MCP-Connectors und der zugehörigen API (zusammen «der Dienst»), herausgegeben von Iris Sainte Fare Garnot (Einzelunternehmen, SIRET 922 108 600 00029). Sie werden bei der Registrierung angenommen und gelten für jede Nutzung des Dienstes. Sie unterstehen französischem Recht. Kontakt: "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
                Para(&[Text(
                    "Diese Übersetzung dient der Information; massgebend ist die französische Fassung.",
                )]),
            ],
        },
        LegalSection {
            title: "Zugang",
            blocks: &[
                Para(&[
                    Text("Der Dienst ist kostenlos und steht jeder Nutzung offen, auch der kommerziellen. Für die Suche auf der Website braucht es kein Konto, für den MCP-Connector und die API hingegen schon. Die API wird mit einem persönlichen Schlüssel genutzt, den Sie auf der "),
                    Local("/profile", "Profilseite"),
                    Text(" erstellen. Wer ein Konto innehat, ist für dessen Nutzung verantwortlich und kann es jederzeit löschen."),
                ]),
            ],
        },
        LegalSection {
            title: "Nutzung",
            blocks: &[
                Para(&[
                    Text("Mit dem Dienst lassen sich Gerichtsentscheide, Erlasse, Verzeichniseinträge und öffentliche Ausschreibungen suchen und einsehen. Er ist nicht dazu bestimmt, die Vervielfältigung dieser Inhalte in ihrer Gesamtheit zu ermöglichen. Jedes Konto verfügt über ein Abfragekontingent, das für die Website, den MCP-Connector und die API gemeinsam gilt, damit die Kapazität des Dienstes unter allen Nutzerinnen und Nutzern aufgeteilt bleibt. Dieses Kontingent darf nicht umgangen werden. Wenn Sie mehr benötigen, schreiben Sie uns. Es ist verboten, den Dienst zu stören, auf Daten oder Konten zuzugreifen, die nicht für Sie bestimmt sind, oder die Sicherheit des Dienstes ohne Erlaubnis zu testen."),
                ]),
                Para(&[
                    Text("Ebenso ist es verboten, den Dienst zu nutzen, um:"),
                ]),
                List(&[
                    &[
                        Text("eine Person zu identifizieren, die von der Datenquelle anonymisiert oder von der Veröffentlichung ausgenommen wurde;"),
                    ],
                    &[
                        Text("Listen von Personen oder Vereinen anhand der tatsächlichen oder vermuteten Herkunft, Staatsangehörigkeit, Religion, sexuellen Orientierung, Gesundheit oder Gewerkschaftszugehörigkeit dieser Personen, der Mitglieder dieser Vereine oder der Personen, für die sie sich einsetzen, zu erstellen;"),
                    ],
                    &[
                        Text("jemanden aufgrund von Verfahren, die diese Person betreffen, von einer Wohnung oder einer Stelle auszuschliessen;"),
                    ],
                    &[
                        Text("Angaben zu sammeln, mit denen sich Personen, Vereine, Gewerkschaften oder religiöse Stätten auffinden lassen, damit sie Gewalt oder Sachbeschädigungen ausgesetzt werden;"),
                    ],
                    &[
                        Text("die berufliche Praxis von Magistratspersonen und Mitarbeitenden der Gerichtskanzleien anhand ihrer Identität zu bewerten, zu analysieren, zu vergleichen oder vorherzusagen, was die Artikel "),
                        Local("/norm/fr/code-de-l-organisation-judiciaire/l111-13", "L. 111-13 des Code de l'organisation judiciaire"),
                        Text(" und "),
                        Local("/norm/fr/code-de-justice-administrative/l10", "L. 10 des Code de justice administrative"),
                        Text(" unter Strafe stellen."),
                    ],
                ]),
            ],
        },
        LegalSection {
            title: "Eingereichte Entscheide",
            blocks: &[
                Para(&[
                    Text("Wenn Sie einen Entscheid einreichen, bestätigen Sie, dass Sie ihn anonymisiert haben und ihn weitergeben dürfen, und Sie sind damit einverstanden, dass er veröffentlicht und frei weiterverwendet wird. Ein eingereichter Entscheid kann abgelehnt werden."),
                ]),
            ],
        },
        LegalSection {
            title: "Weiterverwendung",
            blocks: &[
                Para(&[
                    Text("Ein Dienst, der über die API, den MCP-Connector oder auf anderem Weg auf LibreJustice zurückgreift, um seinen eigenen Nutzerinnen und Nutzern Treffer zu liefern, gibt diese Herkunft gut sichtbar dort an, wo die Treffer erscheinen. Verlangt er für den Zugang zu diesen Treffern ein Entgelt, weist er zudem darauf hin, dass sie auf librejustice.fr kostenlos abrufbar sind. Die Nutzerin oder der Nutzer ist allein verantwortlich für die Verwendung der erhaltenen Daten und für das, was davon Dritten vorgelegt wird. Die Inhalte unterstehen weiterhin der Lizenz ihrer Quelle, die auf der Seite "),
                    Local("/sources", "Daten & Quellen"),
                    Text(" angegeben ist."),
                ]),
            ],
        },
        LegalSection {
            title: "Haftung",
            blocks: &[
                Para(&[
                    Text("Der Dienst wird so bereitgestellt, wie er ist. Für die Richtigkeit, die Vollständigkeit und die Verfügbarkeit der Daten und des Dienstes wird keine Gewähr übernommen. Die angezeigten Informationen stellen keine Rechtsberatung dar. Personendaten werden nach Massgabe der "),
                    Local("/privacy", "Datenschutzerklärung"),
                    Text(" bearbeitet."),
                ]),
            ],
        },
        LegalSection {
            title: "Änderung und Unterbrechung",
            blocks: &[
                Para(&[
                    Text("Der Dienst kann jederzeit und ohne Vorankündigung ganz oder teilweise geändert, eingeschränkt, unterbrochen oder eingestellt werden. Ebenso kann ein Konto oder ein Schlüssel, dessen Nutzung gegen diese Bedingungen verstösst oder den Dienst stört, ohne Vorankündigung und ohne Entschädigung gesperrt oder gelöscht werden. Wer das Konto oder den Schlüssel innehat, erhält die Begründung und kann dazu Stellung nehmen. Diese Bedingungen können geändert werden. Es gelten jeweils die Bedingungen, die im Zeitpunkt der Nutzung online sind."),
                ]),
            ],
        },
    ],
};

const PRIVACY: LegalPage = LegalPage {
    title: "Datenschutz - LibreJustice",
    heading: "Datenschutzerklärung",
    in_force: "In Kraft seit dem 8. November 2026",
    sections: &[
        LegalSection {
            title: "Verantwortliche Stelle",
            blocks: &[
                Para(&[
                    Text("Iris Sainte Fare Garnot (Einzelunternehmen, SIRET 922 108 600 00029) gibt "),
                    Strong("librejustice.fr"),
                    Text(" heraus. Kontakt: "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
                Para(&[Text(
                    "Diese Übersetzung dient der Information; massgebend ist die französische Fassung.",
                )]),
            ],
        },
        LegalSection {
            title: "Erhobene Daten",
            blocks: &[
                Para(&[
                    Strong("Benutzerkonto."),
                    Text(" Ihre E-Mail-Adresse, Ihr Name, falls Sie ihn angegeben haben, und Ihre Google- oder Microsoft-Kennung, wenn Sie sich über einen dieser Dienste anmelden. Wir erfassen zudem die Sprache Ihres Browsers, Ihr Land, das Datum Ihrer Registrierung und das Ihres letzten Besuchs. Wenn Sie einen KI-Assistenten mit Ihrem Konto verbinden, übermitteln wir ihm Ihre E-Mail-Adresse. Wir halten ausserdem fest, welche E-Mails wir Ihnen senden und ob Sie diese abbestellt haben. Alles wird gelöscht, wenn Sie Ihr Konto löschen."),
                ]),
                Para(&[
                    Strong("Technische Protokolle."),
                    Text(" Der Betrieb des Dienstes stützt sich auf Anwendungsprotokolle und Performance-Traces. Sie dienen dazu, die Antwortzeiten zu messen, Fehler zu diagnostizieren, die Nutzung des Dienstes zu verfolgen und Nutzungen zu erkennen, die nach den Nutzungsbedingungen verboten sind. Sie können die Parameter der Anfragen enthalten, darunter die Suchbegriffe, sowie die Art des verwendeten Clients (Browser, API oder KI-Assistent). Diese Daten sind pseudonymisiert: Namen und E-Mail-Adressen kommen darin nicht vor, und die Nutzerin oder der Nutzer erscheint nur unter einer abgeleiteten technischen Kennung. Sie werden nach 30 Tagen automatisch gelöscht."),
                ]),
                Para(&[
                    Strong("Reichweitenmessung."),
                    Text(" Um die Nutzung des Dienstes zu verfolgen und seine Weiterentwicklung auszurichten, erstellen wir Nutzungskennzahlen, die pro Tag zusammengefasst werden: die Zahl der Suchen und Aufrufe, die meistbesuchten Seiten, die genutzten Funktionen und Filter, die Website, von der die Besucherinnen und Besucher kommen, und die Verteilung nach Zugangskanal. Diese Kennzahlen beschränken sich auf Zählungen und enthalten weder die gesuchten Begriffe noch die Seiten, die eine Person gelesen hat. Dafür werden keine Cookies verwendet und keine IP-Adressen gespeichert."),
                ]),
                Para(&[
                    Strong("Eingereichte Entscheide."),
                    Text(" Die Datei, die Sie einreichen, und Ihr Konto. Die Datei wird gelöscht, sobald sie geprüft ist. Wird der Entscheid veröffentlicht, bleibt Ihr Konto mit seiner Herkunft verknüpft, wird aber nie angezeigt."),
                ]),
                Para(&[
                    Strong("Meldung eines Problems."),
                    Text(" Der Text Ihrer Nachricht, die URL der betreffenden Seite (samt Suchbegriffen, wenn sie solche enthält), ob Sie angemeldet waren oder nicht, und Ihre E-Mail-Adresse, wenn Sie sie angeben oder angemeldet sind. Diese Angaben gehen in unserem Support-Postfach ein und werden dort aufbewahrt, um Ihr Anliegen weiterzuverfolgen."),
                ]),
            ],
        },
        LegalSection {
            title: "Cookies und lokale Speicherung",
            blocks: &[
                Para(&[
                    Text("Wir verwenden keine Werbe-Cookies und keine einwilligungspflichtigen Tracker. Unsere Reichweitenmessung ist so eingerichtet, dass sie Ihre Privatsphäre gemäss den Empfehlungen der CNIL respektiert. Deshalb zeigt diese Website kein Cookie-Banner an. Wenn Sie angemeldet sind, speichert Ihr Browser ein Sitzungstoken, das beim Abmelden gelöscht wird. Ein Cookie speichert die Rechtsordnung und die Sprache, die Sie wählen. Der Browser-Tab behält zudem den Kontext Ihrer Suche, bis er geschlossen wird, damit Sie zu den Treffern zurückkehren können und die gesuchten Begriffe in einem Entscheid hervorgehoben werden."),
                ]),
            ],
        },
        LegalSection {
            title: "Auftragsbearbeiter",
            blocks: &[
                List(&[
                    &[
                        Strong("OVH"),
                        Text(" (Roubaix): Hosting, Datenbank, Telemetrie, E-Mail."),
                    ],
                    &[
                        Strong("Cloudflare Lda."),
                        Text(" (Lissabon): CDN, Schutz, Reichweitenmessung und semantische Analyse der Suchanfragen. Seine "),
                        Link("https://developers.cloudflare.com/workers-ai/platform/data-usage/", "Bedingungen zur Datennutzung"),
                        Text(" schliessen das Training seiner Modelle mit diesen Suchanfragen aus."),
                    ],
                    &[
                        Strong("Grafana Cloud"),
                        Text(" (New York, Daten in Frankfurt): Logs, Traces und Reichweitenmessung."),
                    ],
                    &[
                        Strong("Mistral AI"),
                        Text(" (Paris): Zusammenfassungen der Entscheide und Neusortierung der Suchtreffer. Das Training seiner Modelle mit den Daten, die wir ihm senden, ist deaktiviert, und seine "),
                        Link("https://legal.mistral.ai/terms/privacy-policy/", "Datenschutzerklärung"),
                        Text(" legt die Aufbewahrungsdauer fest."),
                    ],
                    &[
                        Strong("Resend"),
                        Text(" (San Francisco, Versand über AWS Dublin): Service-E-Mails (Bestätigung, Zurücksetzen) und Informations-E-Mails."),
                    ],
                ]),
            ],
        },
        LegalSection {
            title: "In den veröffentlichten Daten genannte Personen",
            blocks: &[
                Para(&[
                    Text("Die angezeigten Daten stammen aus amtlichen Veröffentlichungen und öffentlichen Registern. Die Entscheide werden so übernommen, wie ihre Quelle sie veröffentlicht, mit der Anonymisierung, die diese vorgenommen hat."),
                ]),
                Para(&[
                    Text("Wer einen Entscheid einreicht, hat ihn vor der Veröffentlichung selbst anonymisiert."),
                ]),
                Para(&[
                    Text("Um eine Angabe, die Sie betrifft, anonymisieren oder berichtigen zu lassen, wenden Sie sich an das Gericht oder das Register, das sie veröffentlicht. Steht sie in einem Entscheid, den eine Nutzerin oder ein Nutzer eingereicht hat, schreiben Sie uns an "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text("."),
                ]),
            ],
        },
        LegalSection {
            title: "Ihre Rechte",
            blocks: &[
                Para(&[
                    Text("Sie haben ein Recht auf Auskunft, Berichtigung, Löschung, Einschränkung der Verarbeitung, Widerspruch und Datenübertragbarkeit (DSGVO, Loi Informatique et Libertés)."),
                ]),
                Para(&[
                    Text("Die Schaltfläche «Konto löschen» auf Ihrer "),
                    Local("/profile", "Profilseite"),
                    Text(" löscht Ihr Konto sofort."),
                ]),
                Para(&[
                    Text("Für alle anderen Anliegen schreiben Sie an "),
                    Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    Text(" oder wenden Sie sich an die "),
                    Link("https://www.cnil.fr/fr/plaintes", "CNIL"),
                    Text("."),
                ]),
            ],
        },
    ],
};

const NOTICE: LegalPage = LegalPage {
    title: "Impressum - LibreJustice",
    heading: "Impressum",
    in_force: "In Kraft seit dem 15. Mai 2026",
    sections: &[
        LegalSection {
            title: "Herausgabe der Website",
            blocks: &[
                Address(&[
                    &[
                        Strong("Iris Sainte Fare Garnot"),
                        Text(", Einzelunternehmen"),
                    ],
                    &[
                        Text("zugleich für den Inhalt verantwortlich"),
                    ],
                    &[
                        Text("SIRET: 922 108 600 00029"),
                    ],
                    &[
                        Text("E-Mail: "),
                        Local("mailto:contact@librejustice.fr", "contact@librejustice.fr"),
                    ],
                ]),
                Para(&[Text(
                    "Diese Übersetzung dient der Information; massgebend ist die französische Fassung.",
                )]),
            ],
        },
        LegalSection {
            title: "Hosting",
            blocks: &[
                Para(&[
                    Text("Website, Suchmaschine, API und Datenbank: "),
                    Strong("OVH SAS"),
                    Text(" (Roubaix)."),
                ]),
                Para(&[
                    Text("Auslieferungsnetz (CDN) und Schutz: "),
                    Strong("Cloudflare Lda."),
                    Text(" (Lissabon)."),
                ]),
            ],
        },
        LegalSection {
            title: "Veröffentlichte Daten",
            blocks: &[
                Para(&[
                    Text("Die von LibreJustice verbreiteten Daten stammen aus öffentlichen Quellen und aus Quellen, deren Übernahme erlaubt ist. Jede Quelle wird mit ihren Bedingungen für die Weiterverwendung auf der Seite "),
                    Local("/sources", "Daten & Quellen"),
                    Text(" vorgestellt."),
                ]),
            ],
        },
        LegalSection {
            title: "Geistiges Eigentum",
            blocks: &[
                Para(&[
                    Text("Der "),
                    Link("https://github.com/librejustice/librejustice", "Quellcode der Website"),
                    Text(" ist unter der Lizenz Apache 2.0 veröffentlicht. Die Komponenten Dritter, auf die er sich stützt, also Bibliotheken, Schriften und Werkzeuge zum Lesen von Dokumenten, stehen weiterhin unter "),
                    Local("/open-source", "ihren eigenen Lizenzen"),
                    Text("."),
                ]),
            ],
        },
    ],
};
