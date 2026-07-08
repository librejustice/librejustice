// Denylist des articles de pure procédure (ADR 0058) — port 1:1 de
// `_PROCEDURAL_ARTICLE_DENYLIST` (schemas.py). Inclus par `search.rs` via
// `include!`. Clés = noms normalisés (`text_key`, `normalize_instrument`).
//
// Ces articles sont masqués de la sortie API (facet + détail) sans toucher la
// donnée en base. Les principes directeurs du procès (CPC 4/5/9/12/14/15/16,
// 122) restent INCLUS volontairement.

/// `(instrument, articles procéduraux)`. Recherche linéaire (≤ 6 instruments).
const PROCEDURAL_ARTICLE_DENYLIST: &[(&str, &[&str])] = &[
    (
        "Code de procédure civile",
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
        "Code de procédure pénale",
        &[
            // forme de l'arrêt et procédure du pourvoi
            "567", "567-1-1", "568", "584", "585", "585-1", "586", "590", "591", "592", "593",
            "594", "598", "609-1", "612", "614", "615", "802",
        ],
    ),
    (
        "Code de l'organisation judiciaire",
        &[
            "L. 131-6",
            "L. 131-6-1",
            "L. 431-3",
            "L. 431-4",
            "L. 432-1",
            "R. 431-5",
        ],
    ),
    // frais (équivalent administratif de l'article 700 CPC)
    ("Code de justice administrative", &["L. 761-1"]),
    // aide juridictionnelle
    ("Loi du 10 juillet 1991", &["20", "24", "37", "75"]),
];
