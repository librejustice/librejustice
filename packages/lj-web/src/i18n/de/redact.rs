use crate::i18n::RedactUi;
use crate::pages::redact::Fault;

pub const REDACT: RedactUi = RedactUi {
    page_title: "Anonymisierung - LibreJustice",
    page_description:
        "Ein PDF, ein Word-Dokument oder ein Bild anonymisieren, ohne dass die Datei Ihren Browser verlässt.",
    page_heading: "Ein Dokument in Ihrem Browser anonymisieren",
    local_processing: "Verarbeitung zu 100\u{a0}% lokal",
    reading: "Dokument wird gelesen …",
    not_opened: "Dieses Dokument wurde nicht geöffnet.",
    choose_other: "Andere Datei wählen",
    stays_local: "Ihr Dokument verlässt Ihr Gerät nicht",
    stays_local_body: "Es wird auf dieser Seite gelesen und anonymisiert. Der Browser blockiert jede Übertragung an einen Server.",
    drop_here: "Dokument hier ablegen",
    formats: "PDF, Word, OpenDocument oder Bild.",
    choose: "Datei wählen",
    loading_tool: "Werkzeug wird geladen …",
    public_code: "Der Code des Werkzeugs ist öffentlich",
    prepare: "Werkzeug offline vorbereiten",
    prepare_body: "Lädt das vollständige Werkzeug herunter, rund 30 MB. Danach können Sie Ihre Verbindung trennen, bevor Sie ein Dokument ablegen.",
    downloading,
    ready_offline: "Das Werkzeug ist offline bereit",
    ready_offline_body: "Sie können Ihre Verbindung trennen und dann Ihr Dokument ablegen.",
    prepare_stopped,
    retry: "Erneut versuchen",
    export: "Exportieren",
    add_to_chat: "Zum Chat hinzufügen",
    sent_to_chat: "Das Dokument wurde an den Chat gesendet.",
    download: "Herunterladen",
    opens_in_tab: "Die Datei öffnet sich in einem neuen Tab.",
    downloaded: "Datei heruntergeladen. Sie können sie dem Chat anhängen.",
    workbench: "Anonymisierungswerkzeug",
    collapse: "Verkleinern",
    expand: "Vergrössern",
    previous_page: "Vorherige Seite",
    next_page: "Nächste Seite",
    zoom: "Zoom",
    zoom_out: "Verkleinern",
    zoom_in: "Vergrössern",
    percent,
    fit: "Einpassen",
    fit_window: "An Fenster anpassen",
    original: "Original",
    text: "Text",
    mask_area: "Bereich schwärzen",
    other_file: "Andere Datei",
    options: "Optionen",
    remove_rect: "Dieses Rechteck entfernen",
    remove_rect_key: "Entfernen (Entf)",
    erased: "gelöscht",
    include: "In die Anonymisierung einbeziehen",
    next_occurrence,
    remove_from_list: "Aus der Liste entfernen",
    edit_label: "Bezeichnung ändern",
    reset_label: "Zur vorgeschlagenen Bezeichnung zurückkehren",
    auto_label: "auto",
    label: "Bezeichnung",
    families: [
        "Hinzugefügt",
        "Namen",
        "Adressen",
        "E-Mail-Adressen",
        "Telefonnummern",
        "Kennnummern",
        "Geschäftsnummern",
        "Geburtsdaten",
        "Geburtsorte",
    ],
    ocr_loading: "Texterkennung wird geladen",
    ocr_language: "Französisch wird geladen",
    ocr_reading: "Seiten werden gelesen",
    use_ocr: "Texterkennung verwenden:",
    reading_stopped,
    ocr_fast: "Schnell",
    ocr_fast_title: "1.4 MB",
    ocr_slow: "Gründlich",
    ocr_slow_title: "Liest auch Handschrift, 13 MB",
    one_moment: "Einen Moment …",
    send_stopped,
    add_placeholder: "Ein Name, eine Adresse, eine Nummer",
    add_label: "Zu entfernenden Text hinzufügen",
    add: "Hinzufügen",
    nothing_yet_draw: "Noch nichts zu entfernen. Tippen Sie ein, was verschwinden soll, oder zeichnen Sie ein Rechteck auf die Seite.",
    nothing_yet: "Noch nichts zu entfernen. Tippen Sie ein, was verschwinden soll.",
    rectangles: "Rechtecke",
    clear_all: "Alle löschen",
    drawn,
    tally,
    checking,
    removing: "Wird entfernt …",
    anonymise: "Anonymisieren",
    stale_title: "Diese Datei enthält Ihre letzten Änderungen nicht",
    stale_body: "Anonymisieren Sie erneut, um die seither gezeichneten Rechtecke und angekreuzten Felder einzubeziehen.",
    anyway,
    apply_again: "Erneut anonymisieren",
    not_downloadable: "Die anonymisierte Datei kann nicht heruntergeladen werden.",
    still_readable,
    recheck_stopped,
    checking_pages: "Seiten werden geprüft …",
    redacted: "anonymisiert",
    neutral_name: "dokument-anonymisiert",
    attach_prompt,
    mcp_loading: "Anonymisierung wird geladen …",
    mcp_failed: "Die Anonymisierung konnte nicht geladen werden.",
    fault,
    reading_starts: "wird geladen",
};

fn downloading(share: f64) -> String {
    format!("Werkzeug wird heruntergeladen: {:.0}\u{a0}%", share * 100.0)
}

fn prepare_stopped(why: &str) -> String {
    format!("Die Vorbereitung wurde abgebrochen: {why}")
}

fn percent(value: f64) -> String {
    format!("{value}\u{a0}%")
}

fn next_occurrence(text: &str) -> String {
    format!("{text}: zum nächsten Vorkommen")
}

fn reading_stopped(why: &str) -> String {
    format!("Das Lesen wurde abgebrochen: {why}")
}

fn send_stopped(why: &str) -> String {
    format!("Die Übermittlung wurde abgebrochen: {why}")
}

fn drawn(rects: usize) -> String {
    format!("{rects} gezeichnet. Die Pixel darunter werden ebenfalls entfernt.")
}

fn tally(ticked: usize, drawn: usize) -> String {
    let strings = match ticked {
        1 => "1 Textstelle ausgewählt".to_string(),
        n => format!("{n} Textstellen ausgewählt"),
    };
    let rects = match drawn {
        1 => "1 Rechteck".to_string(),
        n => format!("{n} Rechtecke"),
    };
    format!("{strings}, {rects}")
}

fn checking(at: &str) -> String {
    format!("Prüfung: {at}")
}

fn anyway(label: &str) -> String {
    let mut chars = label.chars();
    let first: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_lowercase)
        .collect();
    format!("Trotzdem {first}{}", chars.as_str())
}

fn still_readable(leaks: &[String]) -> String {
    format!("Noch lesbar: {}", leaks.join(", "))
}

fn attach_prompt(name: &str, id: &str) -> String {
    format!("Füge das anonymisierte Dokument «{name}» (Datei {id}) zum Chat hinzu.")
}

fn recheck_stopped(why: &str) -> String {
    format!("Die erneute Prüfung der Seiten wurde abgebrochen: {why}")
}

fn fault(said: &Fault) -> String {
    match said {
        Fault::Said(words) => words.clone(),
        Fault::Unknown => "unbekannter Fehler".into(),
        Fault::NotExposed(name) => format!("das Modul stellt `{name}` nicht bereit"),
        Fault::NoWindow => "kein Fenster".into(),
        Fault::NoDocument => "kein Dokument".into(),
        Fault::LoaderMissing => "der Lader des Moduls wurde nicht eingerichtet".into(),
        Fault::LoaderSilent => "der Lader des Moduls hat nicht geantwortet".into(),
        Fault::ModuleUnloaded(why) => format!(
            "das Anonymisierungsmodul konnte nicht geladen werden ({})",
            fault(why)
        ),
        Fault::NoPromise => "der Leser hat kein Promise zurückgegeben".into(),
        Fault::NoPixels => "die Vorbereitung hat keine Pixel zurückgegeben".into(),
        Fault::NoBytes => "das Entfernen hat keine Bytes zurückgegeben".into(),
        Fault::NoPageImage => "das Bild der Seite wurde nicht erzeugt".into(),
        Fault::BadMatrix => "ungültige Bildmatrix".into(),
        Fault::NoPageDrawing => "das Modul hat keine Seite zurückgegeben".into(),
        Fault::NoFontList => "das Modul hat keine Schriftenliste zurückgegeben".into(),
        Fault::FontWithoutFamily => "eine Schrift ohne Familie".into(),
        Fault::FontWithoutSource(family) => {
            format!("die Schrift {family} hat weder Datei noch Bytes")
        }
        Fault::FileUnread => "die Datei konnte nicht gelesen werden".into(),
        Fault::FileUnassembled => "die Datei konnte nicht zusammengesetzt werden".into(),
        Fault::Unsupported => {
            "hier lassen sich nur PDF, Word- oder OpenDocument-Dokumente und Bilder öffnen".into()
        }
        Fault::ImageUndecoded => "dieses Bild konnte nicht decodiert werden".into(),
        Fault::ImageUndrawn => "dieses Bild konnte nicht gezeichnet werden".into(),
        Fault::ImageUnencoded => "das Bild konnte nicht neu codiert werden".into(),
        Fault::FontNotReceived(file) => format!("die Schrift {file} wurde nicht empfangen"),
        Fault::FontUnread => "eine Schrift des Dokuments konnte nicht gelesen werden".into(),
        Fault::FilesUnloaded => {
            "die Schriften oder Tabellen des Dokuments konnten nicht geladen werden".into()
        }
        Fault::NotServed(url, status) => format!("{url} wurde nicht ausgeliefert ({status})"),
        Fault::FontUndeclared(family) => {
            format!("die Schrift {family} konnte nicht deklariert werden")
        }
        Fault::NothingOpen => "kein Dokument geöffnet".into(),
        Fault::NoImageOpen => "kein Bild geöffnet".into(),
        Fault::No2d => "der Browser hat keinen 2D-Kontext zurückgegeben".into(),
        Fault::PageUnread => "die Seite konnte nicht gelesen werden".into(),
        Fault::PageUndrawn => "die Seite konnte nicht gezeichnet werden".into(),
        Fault::PageUnconverted => "die Seite konnte nicht konvertiert werden".into(),
        Fault::NotText(folder) => format!("{folder}/index.txt ist kein Text"),
        Fault::NoCanvas => "Canvas nicht verfügbar".into(),
        Fault::HostSilent => "der Host hat nicht geantwortet".into(),
        Fault::HostRefused => "vom Host abgelehnt".into(),
        Fault::HostLacks(method) => format!("dem Host fehlt {method}"),
        Fault::HostGaveNoAddress => "der Host hat der Datei keine Adresse gegeben".into(),
        Fault::HostKeepsNoFile => "der Host speichert keine Datei".into(),
        Fault::HostKeptNoFile => "der Host hat die Datei nicht gespeichert".into(),
        Fault::MessageNotSent => "der Host hat die Nachricht nicht gesendet".into(),
    }
}
