//! Politique d'accès des crawlers — source de vérité unique.
//!
//! Un agent se range par ce qu'il FAIT de la donnée, pas par la société qui
//! l'opère : Anthropic, Google, Meta et OpenAI ont chacun des UAs des deux
//! côtés de la ligne.
//!
//! Deux artefacts sont rendus depuis cette table, et rien d'autre ne les
//! écrit : le `robots.txt` servi par `lj-api`, et les expressions des règles
//! WAF Cloudflare poussées par `mise run waf-policy`. Le fichier porte
//! l'intention opposable (art. 4 directive UE 2019/790), le WAF porte la
//! contrainte — un `Disallow` ne s'auto-exécute pas.

/// Ce que l'agent fait de la donnée. L'ordre des variantes est celui des
/// groupes rendus dans `robots.txt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// Collecte pour pré-entraînement de modèles.
    Training,
    /// Scraping commercial SEO / brand monitoring.
    Seo,
    /// Crawl de fond qui construit un index citant et liant. Que cet index
    /// serve dix liens bleus ou un paragraphe généré ne change rien à ce que
    /// l'agent fait ici : il moissonne, personne n'attend la page.
    Search,
    /// Une personne demande une page à son assistant.
    UserFetch,
    /// Grounding et extraction à la demande, pour un usage applicatif tiers.
    Tooling,
    /// Cartes d'aperçu de lien.
    LinkPreview,
}

impl Category {
    /// `true` si la catégorie est refusée sur tout le site.
    pub fn denies_everything(self) -> bool {
        matches!(self, Self::Training | Self::Seo)
    }

    /// `true` si l'agent parcourt le site **sans qu'une personne attende la
    /// page** : il moissonne pour son compte, à son rythme. Les trois autres
    /// catégories sont déclenchées par une action humaine — quelqu'un demande une page
    /// à son assistant, colle un lien, lance un outil.
    pub fn is_unattended(self) -> bool {
        matches!(self, Self::Training | Self::Seo | Self::Search)
    }
}

/// Motif cherché dans l'UA complet pour reconnaître un token — **sous-chaîne,
/// en minuscules**. Rendu tel quel dans les expressions WAF, et comparé tel quel par
/// [`category_of`].
///
/// Le token en minuscules par défaut. `Amazonbot` fait exception : l'URL de doc
/// `+https://developer.amazon.com/support/amazonbot` voyage dans la chaîne des
/// agents Amazon **autorisés**, et le WAF ne sait pas qu'elle n'est pas le nom du
/// robot. Le séparateur de version distingue les deux.
fn pattern_of(token: &str) -> String {
    match token {
        "Amazonbot" => "amazonbot/".to_string(),
        _ => token.to_lowercase(),
    }
}

/// Catégorie d'un `User-Agent`, ou `None` s'il ne correspond à aucun agent connu
/// de la table.
pub fn category_of(user_agent: &str) -> Option<Category> {
    entry_of(user_agent).map(|(_, category)| category)
}

/// Table row a `User-Agent` matches, token included: one operator publishes
/// several UA strings for the same robot, and the token is what names it.
///
/// Patterns contain one another: `applebot` is a substring of
/// `applebot-extended`, which is not in the same category, so the LONGEST match
/// wins. The WAF has no such resolution and refuses as soon as one pattern of
/// the rule matches.
pub fn entry_of(user_agent: &str) -> Option<(&'static str, Category)> {
    let ua = user_agent.to_lowercase();
    CRAWLERS
        .iter()
        .filter(|(token, _)| ua.contains(pattern_of(token).as_str()))
        .max_by_key(|(token, _)| pattern_of(token).len())
        .copied()
}

/// Table complète. Le nom est celui du token `User-agent:` publié par
/// l'opérateur, rendu tel quel dans `robots.txt` ; ce que WAF et [`category_of`]
/// cherchent dans l'UA complet est le motif qu'en tire [`pattern_of`].
pub const CRAWLERS: &[(&str, Category)] = &[
    // ── Pré-entraînement ────────────────────────────────────────────────────
    ("GPTBot", Category::Training),
    ("ClaudeBot", Category::Training),
    ("anthropic-ai", Category::Training),
    ("Google-Extended", Category::Training),
    ("Applebot-Extended", Category::Training),
    ("Meta-ExternalAgent", Category::Training),
    ("FacebookBot", Category::Training),
    ("CCBot", Category::Training),
    ("Bytespider", Category::Training),
    ("TikTokSpider", Category::Training),
    ("cohere-ai", Category::Training),
    ("cohere-training-data-crawler", Category::Training),
    ("AI2Bot", Category::Training),
    ("AI2Bot-Dolma", Category::Training),
    ("ChatGLM-Spider", Category::Training),
    ("DeepSeekBot", Category::Training),
    ("ImagesiftBot", Category::Training),
    ("Webzio-extended", Category::Training),
    ("Omgili", Category::Training),
    // Amazon le dit de son propre agent : « may be used to train Amazon AI
    // models » (developer.amazon.com/amazonbot). Ses deux autres UA, qui ne
    // servent pas l'entraînement, sont plus bas — et portent cette même URL
    // dans leur chaîne, d'où le motif `amazonbot/` de `pattern_of`.
    ("Amazonbot", Category::Training),
    ("Omgilibot", Category::Training),
    ("FriendlyCrawler", Category::Training),
    ("ICC-Crawler", Category::Training),
    ("Timpibot", Category::Training),
    // ── SEO / brand monitoring ──────────────────────────────────────────────
    ("SemrushBot", Category::Seo),
    ("SemrushBot-OCOB", Category::Seo),
    ("AhrefsBot", Category::Seo),
    ("DataForSeoBot", Category::Seo),
    ("MJ12bot", Category::Seo),
    ("DotBot", Category::Seo),
    ("Diffbot", Category::Seo),
    ("AwarioBot", Category::Seo),
    ("AwarioSmartBot", Category::Seo),
    ("AwarioRssBot", Category::Seo),
    ("VelenPublicWebCrawler", Category::Seo),
    ("Channel3Bot", Category::Seo),
    // ── Recherche et indexation avec citation ───────────────────────────────
    ("Googlebot", Category::Search),
    ("Bingbot", Category::Search),
    ("DuckDuckBot", Category::Search),
    ("Qwantbot", Category::Search),
    ("Applebot", Category::Search),
    ("OAI-SearchBot", Category::Search),
    ("Claude-SearchBot", Category::Search),
    ("Meta-WebIndexer", Category::Search),
    ("PerplexityBot", Category::Search),
    ("DuckAssistBot", Category::Search),
    // Amazon sépare ses trois agents : `Amazonbot` est en `Training` ci-dessus,
    // `Amzn-SearchBot` et `Amzn-User` « do not crawl content for generative AI
    // model training » (developer.amazon.com/amazonbot).
    ("Amzn-SearchBot", Category::Search),
    // « The main indexing robot » (yandex.com/support/webmaster/robot-workings/
    // check-yandex-robots.html). Sa doc ne lui prête aucune collecte
    // d'entraînement : le volet IA est gouverné par les deux jetons ci-dessous.
    ("YandexBot", Category::Search),
    // Jetons de contrôle, pas des robots de crawl : Yandex les décrit comme aidant
    // « to process robots.txt to prevent page content from appearing in Search with
    // Yandex AI responses ». Rangés par ce qu'ils gouvernent, comme `Google-Extended`
    // et `Applebot-Extended`. Les deux sont listés parce que Yandex en publie deux et
    // qu'il faut les nommer : à défaut de section à leur nom, les robots qui
    // téléchargent à d'autres fins que l'indexation se considèrent en accès illimité.
    ("YandexAdditional", Category::Search),
    ("YandexAdditionalBot", Category::Search),
    ("PetalBot", Category::Search),
    ("Youbot", Category::Search),
    // ── Fetch à l'initiative de l'utilisateur ───────────────────────────────
    ("ChatGPT-User", Category::UserFetch),
    ("Claude-User", Category::UserFetch),
    ("Claude-Web", Category::UserFetch),
    ("Perplexity-User", Category::UserFetch),
    ("Meta-ExternalFetcher", Category::UserFetch),
    ("Google-NotebookLM", Category::UserFetch),
    ("Amzn-User", Category::UserFetch),
    // ── Outillage agentique ─────────────────────────────────────────────────
    ("Google-CloudVertexBot", Category::Tooling),
    ("Crawl4AI", Category::Tooling),
    ("FirecrawlAgent", Category::Tooling),
    // ── Aperçus de lien ─────────────────────────────────────────────────────
    ("facebookexternalhit", Category::LinkPreview),
    ("Slackbot", Category::LinkPreview),
    ("Twitterbot", Category::LinkPreview),
    ("Discordbot", Category::LinkPreview),
    ("LinkedInBot", Category::LinkPreview),
    ("WhatsApp", Category::LinkPreview),
];

/// Chemin du `robots.txt`, servi par `lj-api` et **épargné par le refus
/// total** : c'est le fichier qui porte l'intention opposable, il ne peut pas
/// être refusé aux agents auxquels il s'adresse.
pub const ROBOTS_PATH: &str = "/robots.txt";

/// Chemins des sitemaps — l'index puis le préfixe des sous-fichiers. Servis par
/// `lj-api`, réservés aux bots vérifiés par une règle WAF : la route et la règle
/// lisent les mêmes constantes.
pub const SITEMAP_PATH: &str = "/sitemap.xml";
pub const SITEMAPS_PREFIX: &str = "/sitemaps/";

/// Paths withheld from every crawler: the browser's JSON surface with its
/// PDF/DOCX exports, and the result pages, an infinite crawl space whose
/// canonical content is `/decision/{juris}/{id}` and `/norm/{juris}/{slug}`, both in the
/// sitemap. `/api/` left this table when the keyed public surface moved in: a URL
/// the guide sends integrators to cannot be forbidden here.
const DISALLOW_API: [&str; 1] = ["/internal/"];
const DISALLOW_SEARCH: [&str; 3] = ["/decision-search?", "/norm-search?", "/norm/*?place="];

/// Seul des deux culs-de-sac à porter aussi une règle WAF : la règle et le
/// `robots.txt` lisent la même constante.
pub const FEEDBACK_PATH: &str = "/feedback";

/// Screens pointing at each other: the bar sets `/login?next=<page>`, the footer
/// `/feedback?from=<page>`, `/feedback` both, so a client that does not dedupe bounces for ever.
const DISALLOW_DEAD_ENDS: [&str; 2] = ["/login", FEEDBACK_PATH];

fn disallow_crawl_space() -> impl Iterator<Item = &'static str> {
    DISALLOW_SEARCH.into_iter().chain(DISALLOW_DEAD_ENDS)
}

fn uas_of(category: Category) -> impl Iterator<Item = &'static str> {
    CRAWLERS
        .iter()
        .filter(move |(_, c)| *c == category)
        .map(|(ua, _)| *ua)
}

/// Rend le `robots.txt` complet, sitemap incluse.
pub fn robots_txt(sitemap_url: &str) -> String {
    let mut out = String::new();

    out.push_str("User-agent: *\n");
    out.push_str("Content-Signal: search=yes, ai-input=yes, ai-train=no\n");
    for path in DISALLOW_API {
        out.push_str(&format!("Disallow: {path}\n"));
    }
    for path in disallow_crawl_space() {
        out.push_str(&format!("Disallow: {path}\n"));
    }
    out.push_str("Allow: /\n");

    for category in [
        Category::Training,
        Category::Seo,
        Category::Search,
        Category::UserFetch,
        Category::Tooling,
        Category::LinkPreview,
    ] {
        out.push('\n');
        for ua in uas_of(category) {
            out.push_str(&format!("User-agent: {ua}\n"));
        }
        if category.denies_everything() {
            out.push_str("Disallow: /\n");
            continue;
        }
        for path in DISALLOW_API {
            out.push_str(&format!("Disallow: {path}\n"));
        }
        // Un aperçu de lien porte sur l'URL qu'une personne a partagée, y
        // compris une recherche ou un écran d'application : seule l'API lui
        // reste fermée.
        if category != Category::LinkPreview {
            for path in disallow_crawl_space() {
                out.push_str(&format!("Disallow: {path}\n"));
            }
        }
        out.push_str("Allow: /\n");
    }

    out.push_str(&format!("\nSitemap: {sitemap_url}\n"));
    out
}

fn ua_clause(uas: impl Iterator<Item = &'static str>) -> String {
    uas.map(|ua| format!("(lower(http.user_agent) contains \"{}\")", pattern_of(ua)))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Expression WAF du refus total : `Training` + `Seo`, [`ROBOTS_PATH`] excepté.
pub fn waf_deny_all_expression() -> String {
    format!(
        "(http.request.uri.path ne \"{ROBOTS_PATH}\") and ({})",
        ua_clause(uas_of(Category::Training).chain(uas_of(Category::Seo)))
    )
}

/// Expression WAF du refus des surfaces JSON aux crawlers autorisés ailleurs.
///
/// Les catégories refusées partout sont hors de cette expression : elles sont déjà
/// couvertes.
pub fn waf_deny_api_expression() -> String {
    let uas = uas_of(Category::Search)
        .chain(uas_of(Category::UserFetch))
        .chain(uas_of(Category::Tooling))
        .chain(uas_of(Category::LinkPreview));
    let paths = DISALLOW_API
        .iter()
        .map(|path| format!("starts_with(http.request.uri.path, \"{path}\")"))
        .collect::<Vec<_>>()
        .join(" or ");
    format!("({paths}) and ({})", ua_clause(uas))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// `Applebot-Extended` never arrives as a `User-Agent`: Apple uses it as a
    /// `robots.txt` control token. The string below is built; the test locks the
    /// longest-match rule.
    #[test]
    fn category_of_retient_le_match_le_plus_long() {
        const APPLEBOT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15 (Applebot/0.1; +http://www.apple.com/go/applebot)";
        const APPLEBOT_EXTENDED: &str =
            "Mozilla/5.0 (compatible; Applebot-Extended/0.1; +http://www.apple.com/go/applebot)";
        assert_eq!(category_of(APPLEBOT), Some(Category::Search));
        assert_eq!(category_of(APPLEBOT_EXTENDED), Some(Category::Training));
    }

    /// Full `User-Agent` strings and their expected category, as the WAF and
    /// `category_of` see them.
    const FULL_UAS: &[(&str, Option<Category>)] = &[
        ("Mozilla/5.0 (compatible; YandexBot/3.0; +http://yandex.com/bots)", Some(Category::Search)),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Amzn-SearchBot/0.1) Chrome/119.0.6045.214 Safari/537.36", Some(Category::Search)),
        // Variante avec l'URL de doc : l'URL contient le nom du robot
        // d'entraînement.
        ("Mozilla/5.0 (compatible; Amzn-SearchBot/1.0; +https://developer.amazon.com/support/amazonbot)", Some(Category::Search)),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Amazonbot/0.1; +https://developer.amazon.com/support/amazonbot) Chrome/119.0.6045.214 Safari/537.36", Some(Category::Training)),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ClaudeBot/1.0; +claudebot@anthropic.com)", Some(Category::Training)),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Claude-SearchBot/1.0; +searchbot@anthropic.com)", Some(Category::Search)),
        ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36 (compatible; meta-webindexer/1.1 (+https://developers.facebook.com/docs/sharing/webmasters/crawler))", Some(Category::Search)),
        // Un navigateur ordinaire n'est dans aucune catégorie.
        ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36", None),
        // Robot de rendu Yandex : ne contient pas le token `YandexBot`.
        ("Mozilla/5.0 (compatible; YandexRenderResourcesBot/1.0; +http://yandex.com/bots)", None),
    ];

    #[test]
    fn category_of_classe_les_chaines_completes() {
        for (ua, attendu) in FULL_UAS {
            assert_eq!(category_of(ua), *attendu, "UA mal classé : {ua}");
        }
    }

    #[test]
    fn seules_les_categories_sans_humain_sont_non_surveillees() {
        assert!(Category::Training.is_unattended());
        assert!(Category::Seo.is_unattended());
        assert!(Category::Search.is_unattended());
        assert!(!Category::UserFetch.is_unattended());
        assert!(!Category::Tooling.is_unattended());
        assert!(!Category::LinkPreview.is_unattended());
    }

    #[test]
    fn chaque_ua_est_unique() {
        let mut seen = HashSet::new();
        for (ua, _) in CRAWLERS {
            assert!(seen.insert(ua.to_lowercase()), "UA en double : {ua}");
        }
    }

    fn motifs_refuses() -> Vec<String> {
        CRAWLERS
            .iter()
            .filter(|(_, c)| c.denies_everything())
            .map(|(token, _)| pattern_of(token))
            .collect()
    }

    /// Le WAF matche en **sous-chaîne** sur l'UA complet, sans la résolution du match
    /// le plus long : un motif refusé qui est sous-chaîne de la chaîne entière d'un
    /// agent autorisé le bloque, quoi que dise `category_of`. Un opérateur glisse
    /// parfois une adresse de contact ou une URL de doc dans sa chaîne.
    #[test]
    fn aucun_motif_refuse_ne_matche_un_ua_autorise() {
        let refuses = motifs_refuses();
        for (ua, categorie) in FULL_UAS {
            if categorie.is_some_and(Category::denies_everything) {
                continue;
            }
            let complet = ua.to_lowercase();
            for motif in &refuses {
                assert!(
                    !complet.contains(motif.as_str()),
                    "le WAF bloquerait « {ua} » sur le motif refusé « {motif} »"
                );
            }
        }
    }

    /// Covers the agents with no full string in `FULL_UAS`.
    #[test]
    fn aucun_motif_refuse_nest_sous_chaine_dun_token_autorise() {
        let refuses = motifs_refuses();
        for (token, category) in CRAWLERS {
            if category.denies_everything() {
                continue;
            }
            let token = token.to_lowercase();
            for motif in &refuses {
                assert!(
                    !token.contains(motif.as_str()),
                    "l'UA autorisé « {token} » contient le motif refusé « {motif} » : \
                     la règle WAF le bloquerait"
                );
            }
        }
    }

    #[test]
    fn robots_refuse_lapi_a_toutes_les_categories() {
        let txt = robots_txt("https://librejustice.fr/sitemap.xml");
        for group in txt.split("\n\n") {
            let allows = group.contains("Allow: /\n");
            if allows {
                for path in DISALLOW_API {
                    assert!(
                        group.contains(&format!("Disallow: {path}\n")),
                        "groupe autorisé sans exclusion de {path} :\n{group}"
                    );
                }
            }
        }
    }

    /// Un aperçu de lien garde les culs-de-sac : quelqu'un qui colle
    /// `librejustice.fr/login` dans un salon doit obtenir sa vignette.
    #[test]
    fn robots_ferme_les_culs_de_sac_sauf_a_l_apercu_de_lien() {
        let txt = robots_txt("https://librejustice.fr/sitemap.xml");
        let apercu = format!(
            "User-agent: {}\n",
            uas_of(Category::LinkPreview).next().unwrap()
        );
        for group in txt.split("\n\n") {
            if !group.contains("Allow: /\n") {
                continue;
            }
            let attendu = !group.contains(&apercu);
            for path in DISALLOW_DEAD_ENDS {
                assert_eq!(
                    group.contains(&format!("Disallow: {path}\n")),
                    attendu,
                    "{path} mal traité dans :\n{group}"
                );
            }
        }
    }

    #[test]
    fn robots_liste_chaque_ua_une_fois() {
        let txt = robots_txt("https://librejustice.fr/sitemap.xml");
        for (ua, _) in CRAWLERS {
            let line = format!("User-agent: {ua}\n");
            assert_eq!(
                txt.matches(&line).count(),
                1,
                "{ua} doit apparaître exactement une fois"
            );
        }
    }

    #[test]
    fn expressions_waf_couvrent_les_bonnes_categories() {
        let deny_all = waf_deny_all_expression();
        let deny_api = waf_deny_api_expression();
        for (ua, category) in CRAWLERS {
            let needle = format!("contains \"{}\"", pattern_of(ua));
            if category.denies_everything() {
                assert!(deny_all.contains(&needle), "{ua} absent du refus total");
                assert!(
                    !deny_api.contains(&needle),
                    "{ua} dupliqué dans le refus /api/"
                );
            } else {
                assert!(!deny_all.contains(&needle), "{ua} refusé par erreur");
                assert!(deny_api.contains(&needle), "{ua} absent du refus /api/");
            }
        }
        let (paths, _) = deny_api.split_once(") and (").expect("deux groupes");
        for path in DISALLOW_API {
            assert!(
                paths.contains(&format!("starts_with(http.request.uri.path, \"{path}\")")),
                "{path} absent du groupe de chemins : {paths}"
            );
        }
    }

    #[test]
    fn le_refus_total_epargne_le_robots_txt() {
        assert!(waf_deny_all_expression().starts_with(&format!(
            "(http.request.uri.path ne \"{ROBOTS_PATH}\") and ("
        )));
    }

    /// Cloudflare plafonne une expression de règle ; on garde de la marge.
    #[test]
    fn expressions_waf_tiennent_dans_la_limite() {
        assert!(waf_deny_all_expression().len() < 4000);
        assert!(waf_deny_api_expression().len() < 4000);
    }
}
