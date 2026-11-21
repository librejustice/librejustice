use crate::i18n::OpenSourceUi;
use crate::i18n::Run::{Link, Text};

pub const OPEN_SOURCE: OpenSourceUi = OpenSourceUi {
    title: "Composants open source - LibreJustice",
    heading: "Composants open source",
    intro: &[
        Text("Le "),
        Link("https://github.com/librejustice/librejustice", "code de LibreJustice"),
        Text(" est publié sous licence Apache 2.0. Il s'appuie sur les composants de tiers ci-dessous, bibliothèques, polices et modèles de lecture des documents, qui gardent chacun leur licence."),
    ],
    component: "Composant",
    version: "Version",
    licence: "Licence",
    notes: "Mentions",
    ocr_weights: "Les deux réseaux PP-OCRv5 sont servis avec leurs poids convertis en float16 et leurs dimensions symboliques renommées. PyLaia RIMES est exporté de PyTorch vers ONNX, avec des poids en int8.",
    word_lists: "Les listes FrequencyWords, de Hermit Dave, sont construites depuis le corpus OpenSubtitles 2018 d'OPUS. Elles sont servies sans modification.",
    font_sources: "Les sources des polices sous copyleft sont servies avec elles. ",
    liberation_sources: "Pour Liberation Sans Narrow, ",
    tahoma_sources: "Pour la Tahoma de Wine, ",
    opensymbol_sources: "Pour OpenSymbol, ",
    and: " et ",
    named: |_| None,
};
