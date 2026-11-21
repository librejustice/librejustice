use crate::i18n::OpenSourceUi;
use crate::i18n::Run::{Link, Text};

pub const OPEN_SOURCE: OpenSourceUi = OpenSourceUi {
    title: "Opensourcecomponenten - LibreJustice",
    heading: "Opensourcecomponenten",
    intro: &[
        Text("De "),
        Link("https://github.com/librejustice/librejustice", "code van LibreJustice"),
        Text(" wordt gepubliceerd onder de Apache 2.0-licentie. Hij steunt op de onderstaande componenten van derden, bibliotheken, lettertypen en modellen die documenten lezen, die elk hun eigen licentie behouden."),
    ],
    component: "Component",
    version: "Versie",
    licence: "Licentie",
    notes: "Vermeldingen",
    ocr_weights: "De twee PP-OCRv5-netwerken worden geleverd met hun gewichten omgezet naar float16 en hun symbolische dimensies hernoemd. PyLaia RIMES is van PyTorch naar ONNX geëxporteerd, met gewichten in int8.",
    word_lists: "De FrequencyWords-lijsten van Hermit Dave zijn opgebouwd uit het OpenSubtitles 2018-corpus van OPUS. Ze worden ongewijzigd geleverd.",
    font_sources: "De bronbestanden van de lettertypen onder copyleft worden mee geleverd. ",
    liberation_sources: "Voor Liberation Sans Narrow, ",
    tahoma_sources: "Voor de Tahoma van Wine, ",
    opensymbol_sources: "Voor OpenSymbol, ",
    and: " en ",
    named: |french| {
        Some(match french {
            "Liberation Sans, Serif et Mono" => "Liberation Sans, Serif en Mono",
            "Noto Sans et Noto Serif" => "Noto Sans en Noto Serif",
            "Tahoma de Wine" => "Tahoma van Wine",
            "GPL-2.0 avec exception de police" => "GPL-2.0 met uitzondering voor lettertypen",
            "licence avec exception de police" => "licentie met uitzondering voor lettertypen",
            _ => return None,
        })
    },
};
