use crate::i18n::RedactUi;
use crate::pages::redact::Fault;

pub const REDACT: RedactUi = RedactUi {
    page_title: "Anonimisering - LibreJustice",
    page_description:
        "Anonimiseer een PDF, een Word-document of een afbeelding zonder dat het bestand uw browser verlaat.",
    page_heading: "Een document anonimiseren in uw browser",
    local_processing: "100 % lokale verwerking",
    reading: "Document wordt gelezen...",
    not_opened: "Dit document kon niet worden geopend.",
    choose_other: "Ander bestand kiezen",
    stays_local: "Uw document verlaat uw toestel niet",
    stays_local_body: "Het wordt op deze pagina gelezen en geanonimiseerd. De browser blokkeert elke verzending naar een server.",
    drop_here: "Sleep een document hierheen",
    formats: "PDF, Word, OpenDocument of afbeelding.",
    choose: "Bestand kiezen",
    loading_tool: "De tool wordt geladen...",
    public_code: "De broncode van de tool is openbaar",
    prepare: "Tool klaarzetten voor offline gebruik",
    prepare_body: "Downloadt de volledige tool, ongeveer 30 MB. Daarna kunt u uw internetverbinding verbreken voordat u een document toevoegt.",
    downloading,
    ready_offline: "De tool is klaar voor offline gebruik",
    ready_offline_body: "U kunt uw internetverbinding verbreken en daarna uw document toevoegen.",
    prepare_stopped,
    retry: "Opnieuw proberen",
    export: "Exporteren",
    add_to_chat: "Toevoegen aan het gesprek",
    sent_to_chat: "Het document is naar het gesprek verzonden.",
    download: "Downloaden",
    opens_in_tab: "Het bestand wordt in een nieuw venster geopend.",
    downloaded: "Bestand gedownload. U kunt het aan het gesprek toevoegen.",
    workbench: "Anonimiseringstool",
    collapse: "Verkleinen",
    expand: "Vergroten",
    previous_page: "Vorige pagina",
    next_page: "Volgende pagina",
    zoom: "Zoom",
    zoom_out: "Uitzoomen",
    zoom_in: "Inzoomen",
    percent,
    fit: "Passend",
    fit_window: "Passend in venster",
    original: "Origineel",
    text: "Tekst",
    mask_area: "Zone afschermen",
    other_file: "Ander bestand",
    options: "Opties",
    remove_rect: "Deze rechthoek verwijderen",
    remove_rect_key: "Verwijderen (Delete)",
    erased: "gewist",
    include: "Opnemen in de anonimisering",
    next_occurrence,
    remove_from_list: "Uit de lijst verwijderen",
    edit_label: "Label wijzigen",
    reset_label: "Terug naar het voorgestelde label",
    auto_label: "auto",
    label: "Label",
    families: [
        "Toegevoegd",
        "Namen",
        "Adressen",
        "E-mailadressen",
        "Telefoonnummers",
        "Identificatienummers",
        "Zaaknummers",
        "Geboortedata",
        "Geboorteplaatsen",
    ],
    ocr_loading: "laden van de tekenherkenning",
    ocr_language: "laden van het Franse taalmodel",
    ocr_reading: "lezen van de pagina's",
    use_ocr: "Tekenherkenning gebruiken:",
    reading_stopped,
    ocr_fast: "Snel",
    ocr_fast_title: "1,4 MB",
    ocr_slow: "Krachtig",
    ocr_slow_title: "Leest ook handschrift, 13 MB",
    one_moment: "Even geduld...",
    send_stopped,
    add_placeholder: "Een naam, een adres, een nummer",
    add_label: "Te verwijderen tekst toevoegen",
    add: "Toevoegen",
    nothing_yet_draw: "Nog niets te verwijderen. Typ wat moet verdwijnen of teken een rechthoek op de pagina.",
    nothing_yet: "Nog niets te verwijderen. Typ wat moet verdwijnen.",
    rectangles: "Rechthoeken",
    clear_all: "Alles wissen",
    drawn,
    tally,
    checking,
    removing: "Bezig met verwijderen...",
    anonymise: "Anonimiseren",
    stale_title: "Dit bestand bevat uw laatste wijzigingen niet",
    stale_body: "Anonimiseer opnieuw om de sindsdien getekende rechthoeken en aangevinkte vakjes op te nemen.",
    anyway,
    apply_again: "Opnieuw anonimiseren",
    not_downloadable: "Het geanonimiseerde bestand kan niet worden gedownload.",
    still_readable,
    recheck_stopped,
    checking_pages: "Pagina's worden gecontroleerd...",
    redacted: "geanonimiseerd",
    neutral_name: "geanonimiseerd-document",
    attach_prompt,
    mcp_loading: "Anonimisering wordt geladen...",
    mcp_failed: "De anonimisering kon niet worden geladen.",
    fault,
    reading_starts: "laden",
};

fn downloading(share: f64) -> String {
    format!("De tool wordt gedownload: {:.0} %", share * 100.0)
}

fn prepare_stopped(why: &str) -> String {
    format!("Het klaarzetten is gestopt: {why}")
}

fn percent(value: f64) -> String {
    format!("{value} %")
}

fn next_occurrence(text: &str) -> String {
    format!("{text}: naar de volgende treffer")
}

fn reading_stopped(why: &str) -> String {
    format!("Het lezen is gestopt: {why}")
}

fn send_stopped(why: &str) -> String {
    format!("Het verzenden is gestopt: {why}")
}

fn drawn(rects: usize) -> String {
    match rects {
        1 => "1 rechthoek getekend. De pixels eronder verdwijnen ook.".into(),
        _ => format!("{rects} rechthoeken getekend. De pixels eronder verdwijnen ook."),
    }
}

fn tally(ticked: usize, drawn: usize) -> String {
    let texts = match ticked {
        1 => "aangevinkte tekst",
        _ => "aangevinkte teksten",
    };
    let rects = match drawn {
        1 => "rechthoek",
        _ => "rechthoeken",
    };
    format!("{ticked} {texts}, {drawn} {rects}")
}

fn checking(at: &str) -> String {
    format!("Controle: {at}")
}

fn anyway(label: &str) -> String {
    format!("Toch {}", label.to_lowercase())
}

fn still_readable(leaks: &[String]) -> String {
    format!("Nog leesbaar: {}", leaks.join(", "))
}

fn attach_prompt(name: &str, id: &str) -> String {
    format!("Voeg het geanonimiseerde document \"{name}\" (bestand {id}) toe aan het gesprek.")
}

fn recheck_stopped(why: &str) -> String {
    format!("Het herlezen van de pagina's is gestopt: {why}")
}

fn fault(said: &Fault) -> String {
    match said {
        Fault::Said(words) => words.clone(),
        Fault::Unknown => "onbekende fout".into(),
        Fault::NotExposed(name) => format!("de module stelt `{name}` niet beschikbaar"),
        Fault::NoWindow => "geen venster".into(),
        Fault::NoDocument => "geen document".into(),
        Fault::LoaderMissing => "de lader van de module ontbreekt".into(),
        Fault::LoaderSilent => "de lader van de module heeft niet gereageerd".into(),
        Fault::ModuleUnloaded(why) => format!(
            "de anonimiseringsmodule kon niet worden geladen ({})",
            fault(why)
        ),
        Fault::NoPromise => "de tekenherkenning gaf geen promise terug".into(),
        Fault::NoPixels => "de voorbereiding gaf geen pixels terug".into(),
        Fault::NoBytes => "het verwijderen gaf geen bytes terug".into(),
        Fault::NoPageImage => "de afbeelding van de pagina is niet gerenderd".into(),
        Fault::BadMatrix => "ongeldige afbeeldingsmatrix".into(),
        Fault::NoPageDrawing => "de module gaf geen pagina terug".into(),
        Fault::NoFontList => "de module gaf geen lijst van lettertypen terug".into(),
        Fault::FontWithoutFamily => "een lettertype zonder familie".into(),
        Fault::FontWithoutSource(family) => {
            format!("het lettertype {family} heeft geen bestand en geen bytes")
        }
        Fault::FileUnread => "het bestand kon niet worden gelezen".into(),
        Fault::FileUnassembled => "het bestand kon niet worden samengesteld".into(),
        Fault::Unsupported => {
            "hier kunnen alleen PDF's, Word- of OpenDocument-documenten en afbeeldingen worden geopend".into()
        }
        Fault::ImageUndecoded => "deze afbeelding kon niet worden gedecodeerd".into(),
        Fault::ImageUndrawn => "deze afbeelding kon niet worden getekend".into(),
        Fault::ImageUnencoded => "de afbeelding kon niet opnieuw worden gecodeerd".into(),
        Fault::FontNotReceived(file) => format!("het lettertype {file} is niet ontvangen"),
        Fault::FontUnread => "een lettertype van het document kon niet worden gelezen".into(),
        Fault::FilesUnloaded => {
            "de lettertypen of tabellen van het document konden niet worden geladen".into()
        }
        Fault::NotServed(url, status) => format!("{url} is niet geleverd ({status})"),
        Fault::FontUndeclared(family) => {
            format!("het lettertype {family} kon niet worden gedeclareerd")
        }
        Fault::NothingOpen => "geen document geopend".into(),
        Fault::NoImageOpen => "geen afbeelding geopend".into(),
        Fault::No2d => "de browser gaf geen 2d-context terug".into(),
        Fault::PageUnread => "de pagina kon niet worden gelezen".into(),
        Fault::PageUndrawn => "de pagina kon niet worden getekend".into(),
        Fault::PageUnconverted => "de pagina kon niet worden geconverteerd".into(),
        Fault::NotText(folder) => format!("{folder}/index.txt is geen tekst"),
        Fault::NoCanvas => "geen canvas beschikbaar".into(),
        Fault::HostSilent => "de host heeft niet gereageerd".into(),
        Fault::HostRefused => "geweigerd door de host".into(),
        Fault::HostLacks(method) => format!("de host ondersteunt {method} niet"),
        Fault::HostGaveNoAddress => "de host gaf het bestand geen adres".into(),
        Fault::HostKeepsNoFile => "de host bewaart geen bestanden".into(),
        Fault::HostKeptNoFile => "de host heeft het bestand niet bewaard".into(),
        Fault::MessageNotSent => "de host heeft het bericht niet verzonden".into(),
    }
}
