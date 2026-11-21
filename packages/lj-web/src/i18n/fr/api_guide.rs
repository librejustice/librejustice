use crate::i18n::ApiGuideUi;
use crate::i18n::Run::{Code, File, Local, Text};

pub const API_GUIDE: ApiGuideUi = ApiGuideUi {
    title: "API publique - LibreJustice",
    description: "L'API REST de LibreJustice : recherche de jurisprudence et de normes, clé gratuite.",
    eyebrow: "API publique · bêta",
    heading: "Une API ",
    heading_law,
    intro: "L'API donne accès en REST à la même base que le site et le connecteur MCP. La jurisprudence et les normes sont mises à jour chaque jour depuis les sources officielles et la clé d'accès est gratuite.",
    key_heading: "Générer une clé",
    key_steps: &[
        Text("Créez un compte, puis une clé depuis "),
        Local("/profile", "votre profil"),
        Text(". Le secret n'est affiché qu'une fois. Une clé se révoque à tout moment."),
    ],
    example,
    key_header: &[
        Text("L'en-tête "),
        Code("x-lj-api-key"),
        Text(" est accepté à la place du "),
        Code("Bearer"),
        Text("."),
    ],
    served_heading: "Ce qui est servi",
    routes: &[
        (
            "GET /api/search",
            "Recherche de décisions, sous les noms de paramètres de l'outil MCP `search_decisions` : `query`, `limit`, `offset`, filtres de juridiction et de date.",
        ),
        (
            "GET /api/search-textes",
            "Recherche d'articles dans le référentiel de normes. `query`, `code` pour borner à un texte, `as_of` pour chercher dans les versions en vigueur ce jour-là.",
        ),
        (
            "GET /api/decision/{id}",
            "Une décision entière, avec son texte et ses métadonnées.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}",
            "Un texte sans articles rend son corps ; un texte qui en porte rend son sommaire. La juridiction est son code ISO en minuscules (`fr`, `be`, `eu`) ; le code se nomme par son slug, son nom ou son sigle.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}/{article}",
            "Un article dans sa version en vigueur, ou telle qu'elle était au jour passé en `as_of`.",
        ),
        (
            "GET /api/norm/{jurisdiction}/{code}/{path}",
            "Une division du texte et ses articles, nommée par le chemin que porte le sommaire (`livre-i/titre-v`).",
        ),
        (
            "GET /api/article/{LEGIARTI…}",
            "Le même article, adressé par son identifiant Légifrance, qui peut aussi désigner un texte ou une section.",
        ),
    ],
    beta: &[
        Text("L'API est en bêta : ses noms de paramètres et de champs peuvent encore changer. Le contrat complet, paramètres et réponses, se lit en OpenAPI : "),
        File("/openapi.json", "openapi.json"),
        Text(". Si votre usage demande autre chose, "),
        Local("/feedback", "dites-le nous"),
        Text("."),
    ],
    rate_heading: "Débit et bonne conduite",
    rate: &[
        Text("Vos appels API, vos appels MCP et vos recherches sur le site s'imputent au même compteur. Un dépassement rend un "),
        Code("429"),
        Text(" avec un "),
        Code("Retry-After"),
        Text(". Si votre usage légitime dépasse ce plafond, écrivez-nous plutôt que de contourner : le fonds est public et gratuit, on souhaite que ça le reste, en faisant attention à partager les ressources entre tous les utilisateurs."),
    ],
    terms: &[
        Text("L'API est régie par les "),
        Local("/terms", "conditions d'utilisation"),
        Text(" du service."),
    ],
    assistant_heading: "Plutôt un assistant ?",
    assistant: &[
        Text("Si le consommateur est une IA et non votre code, le "),
        Local("/mcp-guide", "connecteur MCP"),
        Text(" fait le même travail sans que vous écriviez de requête : l'assistant enchaîne les recherches lui-même."),
    ],
};

fn heading_law(adjective: Option<&str>) -> String {
    match adjective {
        Some(adjective) => format!("pour le droit {adjective}."),
        None => "pour le droit.".to_string(),
    }
}

fn example(base: &str) -> String {
    format!(
        "curl -H 'Authorization: Bearer ljk_votre_cle' \\\n  \
         '{base}/api/search?query=trouble+anormal+de+voisinage&limit=3'"
    )
}
