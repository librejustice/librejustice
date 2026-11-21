use crate::i18n::ProvenanceUi;

pub const PROVENANCE: ProvenanceUi = ProvenanceUi {
    title: "Quelle",
    source: "Quelle",
    ecli: "ECLI",
    diffuser: "Plattform",
    identifier: "Kennung",
    freshness: "Datenstand",
    authority: "Art der Quelle",
    authority_value,
    official_page: "Offizielle Seite",
    diffuser_page: "Seite der Plattform",
    upstream: "Ursprüngliche Quelle",
    permalink: "Permalink",
};

fn authority_value(french: &str) -> Option<&'static str> {
    Some(match french {
        "source gouvernementale" => "Amtliche Quelle",
        "source institutionnelle" => "Institutionelle Quelle",
        "agrégateur de référence" => "Anerkannte Rechtsdatenbank",
        "agrégateur tiers" => "Rechtsdatenbank eines Dritten",
        "source privée" => "Private Quelle",
        "source non vérifiée" => "Ungeprüfte Quelle",
        _ => return None,
    })
}
