use crate::i18n::ProvenanceUi;

pub const PROVENANCE: ProvenanceUi = ProvenanceUi {
    title: "Source",
    source: "Source",
    ecli: "ECLI",
    diffuser: "Diffuseur",
    identifier: "Identifiant",
    freshness: "Fraîcheur de la source",
    authority: "Autorité",
    authority_value: |_| None,
    official_page: "Page officielle",
    diffuser_page: "Page du diffuseur",
    upstream: "Source d'origine",
    permalink: "Permalien",
};
