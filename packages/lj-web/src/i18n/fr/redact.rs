use crate::i18n::RedactUi;
use crate::pages::redact::Fault;

pub const REDACT: RedactUi = RedactUi {
    page_title: "Anonymisation - LibreJustice",
    page_description:
        "Anonymiser un PDF, un document Word ou une image sans que le fichier quitte votre navigateur.",
    page_heading: "Anonymiser un document dans votre navigateur",
    local_processing: "Traitement 100 % local",
    reading: "Lecture du document...",
    not_opened: "Ce document n'a pas été ouvert.",
    choose_other: "Choisir un autre fichier",
    stays_local: "Votre document ne quitte pas votre appareil",
    stays_local_body: "Il est lu et anonymisé dans cette page. Le navigateur bloque tout envoi vers un serveur.",
    drop_here: "Déposez un document ici",
    formats: "PDF, Word, OpenDocument ou image.",
    choose: "Choisir un fichier",
    loading_tool: "Chargement de l'outil...",
    public_code: "Le code de l'outil est public",
    prepare: "Préparer l'outil hors ligne",
    prepare_body: "Télécharge l'outil complet, environ 30 Mo. Vous pourrez ensuite couper votre connexion avant de déposer un document.",
    downloading,
    ready_offline: "L'outil est prêt hors ligne",
    ready_offline_body: "Vous pouvez couper votre connexion, puis déposer votre document.",
    prepare_stopped,
    retry: "Réessayer",
    export: "Exporter",
    add_to_chat: "Ajouter à la conversation",
    sent_to_chat: "Le document est envoyé à la conversation.",
    download: "Télécharger",
    opens_in_tab: "Le fichier s'ouvre dans un nouvel onglet.",
    downloaded: "Fichier téléchargé. Vous pouvez le joindre à la conversation.",
    workbench: "Atelier d'anonymisation",
    collapse: "Réduire",
    expand: "Agrandir",
    previous_page: "Page précédente",
    next_page: "Page suivante",
    zoom: "Zoom",
    zoom_out: "Réduire",
    zoom_in: "Agrandir",
    percent,
    fit: "Ajuster",
    fit_window: "Ajuster à la fenêtre",
    original: "Original",
    text: "Texte",
    mask_area: "Masquer une zone",
    other_file: "Autre fichier",
    options: "Options",
    remove_rect: "Retirer ce rectangle",
    remove_rect_key: "Retirer (Suppr)",
    erased: "effacé",
    include: "Inclure dans l'anonymisation",
    next_occurrence,
    remove_from_list: "Retirer de la liste",
    edit_label: "Modifier l'étiquette",
    reset_label: "Revenir à l'étiquette proposée",
    auto_label: "auto",
    label: "Étiquette",
    families: [
        "Ajoutés",
        "Noms",
        "Adresses",
        "Courriels",
        "Téléphones",
        "Identifiants",
        "Numéros de dossier",
        "Dates de naissance",
        "Lieux de naissance",
    ],
    ocr_loading: "chargement du lecteur",
    ocr_language: "chargement du français",
    ocr_reading: "lecture des pages",
    use_ocr: "Utiliser la reconnaissance de caractères :",
    reading_stopped,
    ocr_fast: "Rapide",
    ocr_fast_title: "1,4 Mo",
    ocr_slow: "Performant",
    ocr_slow_title: "Lit aussi l'écriture à la main, 13 Mo",
    one_moment: "Un instant...",
    send_stopped,
    add_placeholder: "Un nom, une adresse, un numéro",
    add_label: "Ajouter un texte à retirer",
    add: "Ajouter",
    nothing_yet_draw: "Rien à retirer pour l'instant. Tapez ce qui doit disparaître, ou tracez un rectangle sur la page.",
    nothing_yet: "Rien à retirer pour l'instant. Tapez ce qui doit disparaître.",
    rectangles: "Rectangles",
    clear_all: "Tout effacer",
    drawn,
    tally,
    checking,
    removing: "Retrait en cours...",
    anonymise: "Anonymiser",
    stale_title: "Ce fichier ne contient pas vos dernières modifications",
    stale_body: "Anonymisez à nouveau pour y inclure les rectangles et les cases cochées depuis.",
    anyway,
    apply_again: "Anonymiser à nouveau",
    not_downloadable: "Le fichier anonymisé n'est pas téléchargeable.",
    still_readable,
    recheck_stopped,
    checking_pages: "Vérification des pages...",
    redacted: "anonymise",
    neutral_name: "document-anonymise",
    attach_prompt,
    mcp_loading: "Chargement de l'anonymisation...",
    mcp_failed: "L'anonymisation n'a pas pu se charger.",
    fault,
    reading_starts: "chargement",
};

fn downloading(share: f64) -> String {
    format!("Téléchargement de l'outil : {:.0} %", share * 100.0)
}

fn prepare_stopped(why: &str) -> String {
    format!("La préparation s'est arrêtée : {why}")
}

fn percent(value: f64) -> String {
    format!("{value} %")
}

fn next_occurrence(text: &str) -> String {
    format!("{text} : aller à l'occurrence suivante")
}

fn reading_stopped(why: &str) -> String {
    format!("La lecture s'est arrêtée : {why}")
}

fn send_stopped(why: &str) -> String {
    format!("L'envoi s'est arrêté : {why}")
}

fn drawn(rects: usize) -> String {
    format!("{rects} tracé(s). Les pixels dessous partent aussi.")
}

fn tally(ticked: usize, drawn: usize) -> String {
    format!("{ticked} chaîne(s) cochée(s), {drawn} rectangle(s)")
}

fn checking(at: &str) -> String {
    format!("Vérification : {at}")
}

fn anyway(label: &str) -> String {
    format!("{label} quand même")
}

fn still_readable(leaks: &[String]) -> String {
    format!("Encore lisible : {}", leaks.join(", "))
}

fn attach_prompt(name: &str, id: &str) -> String {
    format!("Ajoute à la conversation le document anonymisé « {name} » (fichier {id}).")
}

fn recheck_stopped(why: &str) -> String {
    format!("La relecture des pages s'est arrêtée : {why}")
}

fn fault(said: &Fault) -> String {
    match said {
        Fault::Said(words) => words.clone(),
        Fault::Unknown => "erreur inconnue".into(),
        Fault::NotExposed(name) => format!("le module n'expose pas `{name}`"),
        Fault::NoWindow => "pas de fenêtre".into(),
        Fault::NoDocument => "pas de document".into(),
        Fault::LoaderMissing => "le chargeur du module n'a pas été posé".into(),
        Fault::LoaderSilent => "le chargeur du module n'a pas répondu".into(),
        Fault::ModuleUnloaded(why) => format!(
            "le module d'anonymisation n'a pas pu être chargé ({})",
            fault(why)
        ),
        Fault::NoPromise => "le lecteur n'a pas rendu de promesse".into(),
        Fault::NoPixels => "la préparation n'a pas rendu de pixels".into(),
        Fault::NoBytes => "le retrait n'a rendu aucun octet".into(),
        Fault::NoPageImage => "l'image de la page n'a pas été rendue".into(),
        Fault::BadMatrix => "matrice d'image invalide".into(),
        Fault::NoPageDrawing => "le module n'a pas rendu de page".into(),
        Fault::NoFontList => "le module n'a pas rendu de liste de polices".into(),
        Fault::FontWithoutFamily => "une police sans famille".into(),
        Fault::FontWithoutSource(family) => format!("la police {family} n'a ni fichier ni octets"),
        Fault::FileUnread => "le fichier n'a pas pu être lu".into(),
        Fault::FileUnassembled => "le fichier n'a pas pu être assemblé".into(),
        Fault::Unsupported => {
            "seuls les PDF, les documents Word ou OpenDocument et les images s'ouvrent ici".into()
        }
        Fault::ImageUndecoded => "cette image n'a pas pu être décodée".into(),
        Fault::ImageUndrawn => "cette image n'a pas pu être dessinée".into(),
        Fault::ImageUnencoded => "l'image n'a pas pu être réencodée".into(),
        Fault::FontNotReceived(file) => format!("la police {file} n'a pas été reçue"),
        Fault::FontUnread => "une police du document n'a pas pu être lue".into(),
        Fault::FilesUnloaded => {
            "les polices ou les tables du document n'ont pas pu être chargées".into()
        }
        Fault::NotServed(url, status) => format!("{url} n'a pas été servi ({status})"),
        Fault::FontUndeclared(family) => format!("la police {family} n'a pas pu être déclarée"),
        Fault::NothingOpen => "aucun document ouvert".into(),
        Fault::NoImageOpen => "aucune image ouverte".into(),
        Fault::No2d => "le navigateur n'a pas rendu de contexte 2d".into(),
        Fault::PageUnread => "la page n'a pas pu être lue".into(),
        Fault::PageUndrawn => "la page n'a pas pu être dessinée".into(),
        Fault::PageUnconverted => "la page n'a pas pu être convertie".into(),
        Fault::NotText(folder) => format!("{folder}/index.txt n'est pas du texte"),
        Fault::NoCanvas => "canevas impossible".into(),
        Fault::HostSilent => "l'hôte n'a pas répondu".into(),
        Fault::HostRefused => "refusé par l'hôte".into(),
        Fault::HostLacks(method) => format!("{method} manque à l'hôte"),
        Fault::HostGaveNoAddress => "l'hôte n'a pas donné d'adresse au fichier".into(),
        Fault::HostKeepsNoFile => "l'hôte ne garde pas de fichier".into(),
        Fault::HostKeptNoFile => "l'hôte n'a pas gardé le fichier".into(),
        Fault::MessageNotSent => "l'hôte n'a pas envoyé le message".into(),
    }
}
