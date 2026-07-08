// Regex statiques. Port des `re.compile` de judilibre.py.
// Inclus dans `extract/judilibre.rs`.
//
// Survivantes classées (audit ADR 0157) : joint_pourvois + cc_pourvoi_num +
// pourvoi_range = fenêtres `DocScan::joint_pourvois_windows` ; body_* =
// zone bandeau (`DocScan::bandeau_text`) ; chamber_*/pole_chambre =
// chaînes de MÉTADONNÉE greffe (jamais le texte).
//
// `regex` ne supporte pas les lookaround : les patterns Python qui en utilisent
// sont compilés ICI sans l'assertion, et l'assertion est rejouée en Rust côté
// appelant (helpers `*_match`). Chaque divergence est annotée localement.

struct Patterns {
    joint_pourvois: Regex,
    cc_pourvoi_num: Regex,
    pourvoi_range: Regex,
    chamber_code_token: Regex,
    chamber_prononce_cut: Regex,
    body_conseil: Regex,
    body_pole: Regex,
    body_named_chamber: Regex,
    pole_chambre: Regex,
}

fn build_patterns() -> Patterns {
    // _RE_BODY_NAMED_CHAMBER : lookahead négatif interne `(?!(?:stop)\b)` retiré.
    let body_named_chamber = ci(
        r"cour\s+d['\x{2019}]appel\s+de\s+[\w'\x{2019}.\- ]+?\s+(chambre(?:\s+(?:des|du|de\s+la|d['\x{2019}]))?(?:\s+[a-zà-ÿ]+){1,3})\s+(?:arr[êe]t|audience|ordonnance|jugement|du|le|n[o°])\b",
    );

    Patterns {
        joint_pourvois: ci_dotall(
            r"joint(?:es)?\s+les\s+pourvois\b([^;]*?)(?:;|\bSur\b|\bAttendu\b|\bVu\b|$)",
        ),
        cc_pourvoi_num: cs(r"\b\d{2}-\d{2}\.\d{2,3}\b"),
        pourvoi_range: ci(r"\bau\s+n[°o]"),
        chamber_code_token: cs(r"^[A-Za-z]{1,2}\d[\w-]*$"),
        chamber_prononce_cut: ci(r"\s+prononc[ée]e?\b.*$"),
        body_conseil: ci(r"\bchambre\s+du\s+conseil\s*\("),
        body_pole: ci(r"\bp[ôo]le\s*(\d+)\s*-\s*chambre\s*(\d+)\b"),
        body_named_chamber,
        pole_chambre: ci(r"^p[ôo]le\s*(\d+)\s*-\s*chambre\s*(\d+)$"),
    }
}
