//! An address element of a notice, stored once whatever notices name it.

use crate::record::{Codec, Column, Spec};

/// Column ids are written into stored shapes: append only.
pub static SPEC: Spec = Spec::new(&[
    Column {
        name: "numero",
        codec: Codec::Text,
        paths: &["france/numeroVoie", "numeroVoie"],
    },
    Column {
        name: "indice",
        codec: Codec::Text,
        paths: &[
            "france/indiceRepetition",
            "indiceRepetition",
            "extensionVoie",
        ],
    },
    Column {
        name: "type_voie",
        codec: Codec::Text,
        paths: &["france/typeVoie", "typeVoie"],
    },
    Column {
        name: "nom_voie",
        codec: Codec::Text,
        paths: &["france/nomVoie", "nomVoie"],
    },
    Column {
        name: "compl",
        codec: Codec::Text,
        paths: &[
            "france/complGeographique",
            "complGeographique",
            "immeubleBatimentResidence",
            "etageEscalierAppartement",
        ],
    },
    Column {
        name: "localite",
        codec: Codec::Text,
        paths: &[
            "france/localite",
            "localite",
            "BP_ou_Lieudit_ou_CommununeDeleguee",
        ],
    },
    Column {
        name: "bp",
        codec: Codec::Text,
        paths: &["france/BP", "BP"],
    },
    Column {
        name: "cp",
        codec: Codec::Text,
        paths: &["france/codePostal", "codePostal"],
    },
    Column {
        name: "ville",
        codec: Codec::Text,
        paths: &["france/ville", "ville", "commune"],
    },
    Column {
        name: "pays",
        codec: Codec::Text,
        paths: &["etranger/pays", "pays"],
    },
    Column {
        name: "etranger",
        codec: Codec::Text,
        paths: &["etranger/adresse"],
    },
    Column {
        name: "line",
        codec: Codec::Text,
        paths: &["", "unstructured_address"],
    },
]);
