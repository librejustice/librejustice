//! Denylist des articles de pure procédure (ADR 0058, stockage ADR 0211) —
//! copie unique du workspace. Clés = alphabet public (ADR 0209) : slug
//! catalogue du texte + clé d'article slug (`article_key`), les formes que
//! portent `legal_text.slug` et `legal_citation.ref_num_key`.
//!
//! Consommée à la persistance (`replace_citations_bulk` ne stocke pas ces
//! occurrences liées — le stock est propre, aucun masque en sortie). Les
//! principes directeurs du procès (CPC 4/5/9/12/14/15/16, 122) restent
//! INCLUS volontairement.

/// `(slug du texte, clés d'articles procéduraux)`.
pub const PROCEDURAL_ARTICLE_DENYLIST: &[(&str, &[&str])] = &[
    (
        "code-de-procedure-civile",
        &[
            // frais et dépens
            "695", "696", "699", "700", // forme et prononcé du jugement
            "450", "451", "452", "453", "454", "455", "456", "457", "458", "459", "462", "463",
            "464", "465", "466", // mise en état
            "446-1", "446-2", "446-3", "446-4", "763", "776", "778", "779", "780", "785", "786",
            "787", "788", "789", "790", "799", "800", "802", "803", "804", "805", "807", "808",
            // exécution provisoire
            "514", "515", "517", "521", "524",
            // circuits d'appel et forme des conclusions
            "905", "905-1", "905-2", "906", "907", "908", "909", "910", "911", "912", "913", "914",
            "916", "954", "960", "961", "963", // désistement / péremption
            "384", "385", "394", "395", "399", // procédure de cassation
            "627", "974", "978", "979", "982", "1009-1", "1010", "1011", "1014", "1015", "1018",
            "1022", "1026", "1031-1",
        ],
    ),
    (
        "code-de-procedure-penale",
        &[
            // forme de l'arrêt et procédure du pourvoi
            "567", "567-1-1", "568", "584", "585", "585-1", "586", "590", "591", "592", "593",
            "594", "598", "609-1", "612", "614", "615", "802",
        ],
    ),
    (
        "code-de-l-organisation-judiciaire",
        &["l131-6", "l131-6-1", "l431-3", "l431-4", "l432-1", "r431-5"],
    ),
    // frais (équivalent administratif de l'article 700 CPC)
    ("code-de-justice-administrative", &["l761-1"]),
    // aide juridictionnelle
    (
        "loi-n-91-647-du-10-juillet-1991-relative-a-l-aide-juridique",
        &["20", "24", "37", "75"],
    ),
];
