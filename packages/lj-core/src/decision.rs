//! Modèle `Decision` / `DecisionSection` (port de `parsing/decision_input.py`).
//!
//! Modèle canonique unique pour les sources XML opendata (TA/CAA/CE) et
//! Judilibre (CC/CA/TJ/TCOM). Tous les champs source-spécifiques ont une
//! valeur par défaut (`None` / `""` / `vec![]`) pour permettre le padding.

use serde::{Deserialize, Serialize};

/// Cible `visa_trim` : 500 tokens estimés via `CHARS_PER_TOKEN_MEDIAN` (3.41).
pub const VISA_TRIM_TARGET_TOKENS: usize = 500;
/// `VISA_TRIM_MAX_CHARS` ≈ 1705 (= 500 × 3.41 tronqué).
pub const VISA_TRIM_MAX_CHARS: usize =
    (VISA_TRIM_TARGET_TOKENS as f64 * crate::tokens::CHARS_PER_TOKEN_MEDIAN) as usize;

/// Décision attaquée telle que la source la déclare (champ `contested`
/// Judilibre, ADR 0161) : matière brute du lien de chronologie — les clés
/// canoniques se dérivent dans `lj-extract::chrono`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackedRef {
    /// Libellé juridiction verbatim (« Cour d'appel de Nancy »).
    pub jurisdiction: Option<String>,
    /// Numéro (RG ou pourvoi) verbatim.
    pub number: Option<String>,
    /// Date ISO.
    pub date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionSection {
    pub label: String,
    /// `preamble` | `procedure` | `visa` | `motivations` | `dispositif`.
    pub kind: String,
    pub start_char: usize,
    pub end_char: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    // ── Identité (obligatoire) ───────────────────────────────────────────
    /// Provenance source exacte (ex. `TA_202208.zip/TA34/DTA_2204150_…xml`).
    pub source_uid: String,
    /// Nom du fichier dans le ZIP.
    pub member_name: String,
    /// ECLI lu verbatim de la source (`payload["ecli"]` Judilibre) — jamais
    /// dérivé. `None` quand la source n'en porte pas (XML opendata). Pivot
    /// d'identité inter-sources (ADR 0080).
    pub ecli: Option<String>,
    // ── Optionnels (padding inter-sources) ───────────────────────────────
    /// Code juridiction du greffe opendata (`Code_Juridiction`, ex. « TA13 »).
    pub jurisdiction_source_code: Option<String>,
    /// Chambre Judilibre (payload `chamber`) : code CC (`soc`, `civ1`…) ou
    /// texte libre de greffe CA/TJ/TCOM.
    pub chamber: Option<String>,
    /// Code NAC (nomenclature des affaires civiles, payload Judilibre `nac`,
    /// ex. `14H`) posé par le greffe à l'enregistrement de l'affaire — porté
    /// par les TJ (100 %) et CA (90 %), absent ailleurs.
    #[serde(default)]
    pub nac: Option<String>,
    pub jurisdiction_name: Option<String>,
    /// TA / CAA / CE / CC / CA / TJ / TCOM (déduit du préfixe uid ou Judilibre).
    pub jurisdiction_type: Option<String>,
    /// Code Judilibre (ca_paris, tj75056…).
    pub jurisdiction_location: Option<String>,
    pub numero_dossier: Option<String>,
    /// Liste propre des numéros (Judilibre `numbers`, multi-valeur).
    pub numero_dossiers: Option<Vec<String>>,
    /// XML opendata uniquement.
    pub numero_role: Option<String>,
    /// ISO, pris sur `Date_Lecture` ou `Date_Audience`.
    pub date_lecture: Option<String>,
    pub date_audience: Option<String>,
    pub date_mise_jour: Option<String>,
    pub formation: Option<String>,
    /// XML opendata uniquement.
    pub type_decision: Option<String>,
    pub type_recours: Option<String>,
    pub solution: Option<String>,
    /// Codes de publication bruts (cf. ADR 0054). Multi-valeur côté judiciaire.
    pub publication_codes: Vec<String>,
    /// XML opendata uniquement.
    pub avocat_requerant: Option<String>,
    /// Thèmes Judilibre (`payload["themes"]` : matière → chaîne de mots-clés),
    /// verbatim. Vide pour les autres sources.
    pub themes: Vec<String>,
    /// Décision attaquée déclarée par la source (Judilibre `contested`,
    /// ADR 0161). `None` pour les autres sources (le texte prend le relais).
    #[serde(default)]
    pub attacked: Option<AttackedRef>,
    /// Texte brut depuis l'XML (avec `<br/>`) ou Judilibre.
    pub texte_integral_raw: String,
    pub texte_integral_clean: String,
    pub sections: Vec<DecisionSection>,
    // ── Préfixes embedding (jamais stockés en DB ; cf. ADR 0018) ──────────
    /// 3 lignes : jur|date / recours|solution / formation.
    pub metadata_header: String,
    /// Début du visa, ≤ `VISA_TRIM_MAX_CHARS`, paragraph-aware.
    pub visa_trim: String,
    pub parse_warnings: Vec<String>,
}

impl Decision {
    /// Première section dont le `kind` correspond.
    pub fn get_section(&self, kind: &str) -> Option<&DecisionSection> {
        self.sections.iter().find(|s| s.kind == kind)
    }

    /// Concatène (`\n\n`) le texte des sections dont le `kind` est demandé.
    pub fn get_section_text(&self, kinds: &[&str]) -> String {
        self.sections
            .iter()
            .filter(|s| kinds.contains(&s.kind.as_str()))
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
