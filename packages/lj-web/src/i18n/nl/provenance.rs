use crate::i18n::ProvenanceUi;

pub const PROVENANCE: ProvenanceUi = ProvenanceUi {
    title: "Bron",
    source: "Bron",
    ecli: "ECLI",
    diffuser: "Verspreider",
    identifier: "Identificatiecode",
    freshness: "Actualiteit van de bron",
    authority: "Instantie",
    authority_value,
    official_page: "Officiële pagina",
    diffuser_page: "Pagina van de verspreider",
    upstream: "Oorspronkelijke bron",
    permalink: "Permalink",
};

fn authority_value(french: &str) -> Option<&'static str> {
    Some(match french {
        "source gouvernementale" => "Officiële bron",
        "source institutionnelle" => "Institutionele bron",
        "agrégateur de référence" => "Erkende juridische databank",
        "agrégateur tiers" => "Juridische databank van derden",
        "source privée" => "Private bron",
        "source non vérifiée" => "Niet-geverifieerde bron",
        _ => return None,
    })
}
