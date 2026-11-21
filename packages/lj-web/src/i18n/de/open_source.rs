use crate::i18n::OpenSourceUi;
use crate::i18n::Run::{Link, Text};

pub const OPEN_SOURCE: OpenSourceUi = OpenSourceUi {
    title: "Open-Source-Komponenten - LibreJustice",
    heading: "Open-Source-Komponenten",
    intro: &[
        Text("Der "),
        Link("https://github.com/librejustice/librejustice", "Quellcode von LibreJustice"),
        Text(" ist unter der Lizenz Apache 2.0 veröffentlicht. Er stützt sich auf die unten aufgeführten Komponenten Dritter, Bibliotheken, Schriften und Modelle zum Lesen von Dokumenten, die jeweils ihre eigene Lizenz behalten."),
    ],
    component: "Komponente",
    version: "Version",
    licence: "Lizenz",
    notes: "Hinweise",
    ocr_weights: "Die beiden PP-OCRv5-Netze werden mit nach float16 umgewandelten Gewichten und umbenannten symbolischen Dimensionen ausgeliefert. PyLaia RIMES ist aus PyTorch nach ONNX exportiert, mit Gewichten in int8.",
    word_lists: "Die FrequencyWords-Listen von Hermit Dave sind aus dem Korpus OpenSubtitles 2018 von OPUS erstellt. Sie werden unverändert ausgeliefert.",
    font_sources: "Die Quellen der Schriften unter Copyleft werden mit ihnen ausgeliefert. ",
    liberation_sources: "Für Liberation Sans Narrow: ",
    tahoma_sources: "Für die Tahoma von Wine: ",
    opensymbol_sources: "Für OpenSymbol: ",
    and: " und ",
    named: |french| {
        Some(match french {
            "Liberation Sans, Serif et Mono" => "Liberation Sans, Serif und Mono",
            "Noto Sans et Noto Serif" => "Noto Sans und Noto Serif",
            "Tahoma de Wine" => "Tahoma von Wine",
            "GPL-2.0 avec exception de police" => "GPL-2.0 mit Ausnahme für Schriften",
            "licence avec exception de police" => "Lizenz mit Ausnahme für Schriften",
            _ => return None,
        })
    },
};
