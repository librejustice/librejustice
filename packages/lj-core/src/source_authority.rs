//! Autorité du **diffuseur** d'un texte (ADR 0129/0130) — mapping pur `source → autorité`.
//!
//! `legal_article.source` est STRICTEMENT un **libellé de diffuseur** (ADR 0131) :
//! `legifrance`, `jorf`, `jafbase`, `europa-eu`, `coe`, `boe.es`… — jamais une URL
//! (→ `source_url`), une catégorie de texte (la nature « TRAITE » vit dans
//! `legal_text.nature`, pas dans `source`) ni une méthode de traduction (→ colonne
//! `translation`, ADR 0116). L'autorité s'en dérive par ce mapping ; l'URL précise
//! d'où on dérive le libellé canonique vit dans [`diffuseur_label_from_url`].
//!
//! Axe DISTINCT de `translation` (officialité du *texte*, ADR 0116) et NON substituable :
//! un texte officiel peut être diffusé par un non-gouvernemental (droit-afrique, jafbase,
//! cabinet d'avocats), et une traduction automatique d'un texte officiel reste une
//! traduction. On sépare donc « le texte est-il officiel » (`translation`) de « qui le
//! diffuse » (cette autorité) — d'où l'absence de toute variante « traduction » ici.

/// Autorité du diffuseur d'un `source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAuthority {
    /// Diffuseur gouvernemental / officiel (JO, ministère, parlement, legifrance, fedlex…).
    Gouvernemental,
    /// Organisation institutionnelle (OIT, FAO, OMS, UE, Conseil de l'Europe…).
    Institutionnel,
    /// Agrégateur de droit **de référence** : éditorialisé/curé par un acteur identifié
    /// et reconnu (ex. jafbase, tenu par un magistrat français) — non officiel mais d'une
    /// fiabilité supérieure à un agrégateur anonyme. Rang distinct d'[`Self::Agregateur`].
    AgregateurDeConfiance,
    /// Agrégateur tiers de droit anonyme / non éditorialisé : republie des textes
    /// officiels, fiable mais non officiel et sans curation identifiée.
    Agregateur,
    /// Source privée / secondaire (cabinet, éditeur, site privé, universitaire, ONG).
    Prive,
    /// Source non classée (libellé générique ou domaine inconnu du mapping).
    Inconnu,
}

impl SourceAuthority {
    /// Libellé court FR pour l'affichage (badge de provenance).
    pub fn label(self) -> &'static str {
        match self {
            SourceAuthority::Gouvernemental => "source gouvernementale",
            SourceAuthority::Institutionnel => "source institutionnelle",
            SourceAuthority::AgregateurDeConfiance => "agrégateur de référence",
            SourceAuthority::Agregateur => "agrégateur tiers",
            SourceAuthority::Prive => "source privée",
            SourceAuthority::Inconnu => "source non vérifiée",
        }
    }
}

/// Classe l'autorité du diffuseur à partir du **libellé** `legal_article.source`.
pub fn source_authority(source: &str) -> SourceAuthority {
    use SourceAuthority::*;
    match source {
        // --- FR officiel (DILA : LEGI, KALI, Journal officiel) ---
        "legifrance" | "kali" | "jorf" => Gouvernemental,
        // --- Gouvernemental / public officiel étranger ---
        "fedlex"
        | "legilux"
        | "boe.es"
        | "parlamento.pt"
        | "pgdlisboa.pt"
        | "justice.sec.gouv.sn"
        | "sgg-mali.ml"
        | "cnlegis.gov.mg"
        | "rabat.eregulations.org"
        | "onousc.ma"
        | "casainvest.ma"
        | "botschaft-madagaskar.de"
        | "cmf.tn"
        | "academiedepolice.bf"
        | "ejustice-be"
        | "cgra-be"
        | "ris-at"
        | "wetten-nl"
        | "birosag-hu"
        | "legislatie-ro"
        | "lexpol.cloud.pf"
        // eRegulations (plateforme CNUCED déployée par l'agence nationale), parallèle
        // à rabat.eregulations.org ; assemblée nationale CI ; Journal officiel Djibouti.
        | "senegal.eregulations.org"
        | "assnat.ci"
        | "journalofficiel.dj"
        // Portail législatif officiel de l'Assemblée Nationale du Bénin (diffuseur
        // first-party des lois qu'elle promulgue ; appui PNUD = financement, pas opérateur).
        | "documentation-anbenin.org" => Gouvernemental,
        // --- Institutionnel (organisations internationales, UE, Conseil de l'Europe) ---
        "europa-eu" | "coe" | "natlex.ilo.org" | "faolex.fao.org" | "who.int" | "a-mla.org"
        // Conférence de La Haye de droit international privé ; division statistique ONU ; UNICEF.
        | "assets.hcch.net" | "unstats.un.org" | "data.unicef.org"
        // OMPI (WIPO Lex republie le droit national, agence spécialisée ONU comme l'OMS/l'OIT).
        | "wipo.int"
        // Banque nationale de Belgique (banque centrale = institution publique).
        | "nbb.be" => Institutionnel,
        // --- Agrégateur de référence : curé par un acteur identifié et reconnu ---
        // jafbase : base tenue par un magistrat français, citée en source de référence
        // (page « Données & sources »). Le libellé est canonicalisé en `jafbase` (la
        // variante d'hôte `jafbase.fr` est rabattue par `diffuseur_label_from_url`, ADR 0131).
        "jafbase" => AgregateurDeConfiance,
        // --- Agrégateurs tiers de droit anonymes (republient des textes officiels) ---
        "droitcamerounais.info" | "mjp" | "ecoi"
        | "droit-afrique.com"
        | "droitci.info"
        | "droitcongolais.info"
        | "legigabon.com"
        | "africa-laws.org"
        | "jurisitetunisie.com"
        // Justia (éditeur juridique commercial US, republie le droit de multiples pays).
        | "venezuela.justia.com"
        // Manshurat (منشورات قانونية, base de droit arabe) ; Internet Archive (dépôt non-profit
        // hébergeant une copie republiée) : republient un texte officiel sans curation identifiée.
        | "manshurat.org"
        | "archive.org" => Agregateur,
        // --- Privé / secondaire / universitaire / ONG ---
        "brocardi.it"
        | "dejure.org"
        | "cabinetnfm.com"
        | "zakonrf.info"
        | "cdpc.univ-tln.fr"
        | "citizenshiprightsafrica.org"
        | "cvuc-uccc.com"
        | "clr.africanchildforum.org"
        | "policehumanrightsresources.org"
        | "humanium"
        | "wikisource"
        | "info-droits-etrangers"
        // ONG / think tank de vérification (VERTIC + son projet BWC) ; projets
        // universitaires (usage de la force policière, observatoire GLOBALCIT/EUI) ;
        // cabinet d'avocats ; initiative privée individuelle (droit ivoirien).
        | "vertic.org"
        | "bwcimplementation.org"
        | "policinglaw.info"
        | "cabinetbelbachir.ma"
        | "data.globalcit.eu"
        | "loidici.biz"
        // Sami Aldeeb (juriste, traductions privées de codes arabes) ; UAIPIT (projet
        // universitaire de l'Université d'Alicante, propriété intellectuelle).
        | "sami-aldeeb.com"
        | "uaipit.com"
        // Portails juridiques privés/commerciaux mono-pays (republient un code national) :
        // Jogtár/Wolters Kluwer (HU, ≠ njt.hu officiel), Société des sciences juridiques (RO),
        // LegeAZ (RO), OENET (GR, sur abonnement), lexlege (PL).
        | "net.jogtar.hu"
        | "codulcivil.ro"
        | "legeaz.net"
        | "oenet.gr"
        | "lexlege.pl" => Prive,
        // Libellé non listé (souvent un domaine brut dérivé d'une URL) : heuristique TLD.
        other => authority_from_host_heuristic(other),
    }
}

/// Autorité d'un libellé inconnu du mapping explicite, par heuristique sur la forme
/// du domaine (les libellés-fallback sont des hôtes bruts, cf. [`diffuseur_label_from_url`]).
/// Gouvernemental si le domaine porte un marqueur public reconnu ; sinon `Inconnu`.
fn authority_from_host_heuristic(label: &str) -> SourceAuthority {
    use SourceAuthority::*;
    let l = label.to_ascii_lowercase();
    if l.contains(".gouv.")
        || l.contains(".gov.")
        || l.contains(".gv.")
        || l.ends_with(".gov")
        || l.contains(".fgov.")
        || l.contains("overheid")
        || l.ends_with(".europa.eu")
    {
        Gouvernemental
    } else {
        Inconnu
    }
}

/// Libellé canonique de diffuseur dérivé d'une `source_url` (ADR 0131) : l'hôte
/// privé de `www.`, avec quelques alias courts pour les diffuseurs récurrents
/// (`publications.europa.eu` → `europa-eu`). `None` si l'URL est vide/illisible.
///
/// Réutilisé par le retrofit (`relabel-sources`) ET le loader de corpus (fallback
/// quand la curation ne fournit pas de libellé) : **source de vérité unique**.
pub fn diffuseur_label_from_url(url: &str) -> Option<String> {
    let host = url_host(url)?;
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let label = match host {
        "publications.europa.eu" | "eur-lex.europa.eu" => "europa-eu",
        "rm.coe.int" | "coe.int" => "coe",
        "humanium.org" => "humanium",
        "cgra.be" => "cgra-be",
        "fr.wikisource.org" | "wikisource.org" => "wikisource",
        // jafbase a deux hôtes (jafbase.fr) ; un seul libellé canonique (ADR 0131).
        "jafbase.fr" => "jafbase",
        "info-droits-etrangers.org" => "info-droits-etrangers",
        "ejustice.just.fgov.be" => "ejustice-be",
        "ecoi.net" => "ecoi",
        "ris.bka.gv.at" => "ris-at",
        "wetten.overheid.nl" => "wetten-nl",
        "birosag.hu" => "birosag-hu",
        "legislatie.just.ro" => "legislatie-ro",
        // Fallback : l'hôte brut EST le libellé (cohérent avec boe.es, brocardi.it…).
        h => h,
    };
    Some(label.to_string())
}

/// Hôte d'une URL (sans schéma, credentials, port), en minuscules. `None` si vide.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host); // credentials
    let host = host.split(':').next().unwrap_or(host); // port
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// Sources *live* re-synchronisées **quotidiennement** (sync-legi/sync-kali, cron) : leur
/// fraîcheur « as-of » vit dans `ingest_freshness` (une ligne/source, rafraîchie par
/// `stamp-freshness`), pas par ligne d'article (éviter de réécrire ~1,9 M lignes/jour).
/// jorf (bulk DILA, dont les traités) et le curé viennent par get ponctuel → fraîcheur
/// = date de get par ligne.
pub fn is_live_authoritative(source: &str) -> bool {
    matches!(source, "legifrance" | "kali")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_to_canonical_label() {
        // Alias courts pour les diffuseurs récurrents.
        assert_eq!(
            diffuseur_label_from_url("http://publications.europa.eu/resource/celex/12016E%2FTXT")
                .as_deref(),
            Some("europa-eu")
        );
        assert_eq!(
            diffuseur_label_from_url("https://rm.coe.int/1680a2353e").as_deref(),
            Some("coe")
        );
        // www. retiré ; hôte brut = libellé pour les diffuseurs non aliasés.
        assert_eq!(
            diffuseur_label_from_url("https://www.brocardi.it/codice-civile/").as_deref(),
            Some("brocardi.it")
        );
        assert_eq!(
            diffuseur_label_from_url("https://www.ris.bka.gv.at/").as_deref(),
            Some("ris-at")
        );
        // jafbase : hôte .fr rabattu sur le libellé canonique unique (ADR 0131).
        assert_eq!(
            diffuseur_label_from_url("http://jafbase.fr/docAfrique/Tchad/x.pdf").as_deref(),
            Some("jafbase")
        );
        // URL vide/sans hôte ⇒ pas de libellé.
        assert_eq!(diffuseur_label_from_url(""), None);
    }

    #[test]
    fn authority_axes_are_clean() {
        // Diffuseur officiel FR vs agrégateur vs privé.
        assert_eq!(source_authority("jorf"), SourceAuthority::Gouvernemental);
        // jafbase = agrégateur de référence (curé par un magistrat), rang au-dessus
        // de l'agrégateur anonyme.
        assert_eq!(
            source_authority("jafbase"),
            SourceAuthority::AgregateurDeConfiance
        );
        assert_eq!(
            source_authority("droit-afrique.com"),
            SourceAuthority::Agregateur
        );
        assert_eq!(source_authority("humanium"), SourceAuthority::Prive);
        assert_eq!(
            source_authority("europa-eu"),
            SourceAuthority::Institutionnel
        );
        // Heuristique TLD pour un domaine inconnu du mapping explicite.
        assert_eq!(
            source_authority("legifrance.gouv.fr"),
            SourceAuthority::Gouvernemental
        );
        assert_eq!(
            source_authority("exemple-prive.com"),
            SourceAuthority::Inconnu
        );
    }

    #[test]
    fn classifies_curated_foreign_diffusers() {
        // Diffuseurs des codes étrangers curés, anciennement non classés (Inconnu).
        use SourceAuthority::*;
        // Gouvernemental : assemblée nationale, JO, plateforme eRegulations étatique.
        assert_eq!(source_authority("assnat.ci"), Gouvernemental);
        assert_eq!(source_authority("journalofficiel.dj"), Gouvernemental);
        assert_eq!(source_authority("senegal.eregulations.org"), Gouvernemental);
        // Institutionnel : conférence de La Haye, ONU.
        assert_eq!(source_authority("assets.hcch.net"), Institutionnel);
        assert_eq!(source_authority("unstats.un.org"), Institutionnel);
        // Agrégateurs tiers anonymes : africains.
        assert_eq!(source_authority("droit-afrique.com"), Agregateur);
        assert_eq!(source_authority("africa-laws.org"), Agregateur);
        // Privé : ONG/think tank, projet universitaire, cabinet, initiative privée.
        assert_eq!(source_authority("vertic.org"), Prive);
        assert_eq!(source_authority("policinglaw.info"), Prive);
        assert_eq!(source_authority("cabinetbelbachir.ma"), Prive);
        assert_eq!(source_authority("loidici.biz"), Prive);
        // Vague de classification tick 16 (8 diffuseurs anciennement Inconnu) :
        assert_eq!(
            source_authority("documentation-anbenin.org"),
            Gouvernemental
        );
        assert_eq!(source_authority("nbb.be"), Institutionnel);
        assert_eq!(source_authority("venezuela.justia.com"), Agregateur);
        // Portails juridiques privés mono-pays (≠ diffuseur officiel) :
        assert_eq!(source_authority("net.jogtar.hu"), Prive);
        assert_eq!(source_authority("codulcivil.ro"), Prive);
        assert_eq!(source_authority("legeaz.net"), Prive);
        assert_eq!(source_authority("oenet.gr"), Prive);
        assert_eq!(source_authority("lexlege.pl"), Prive);
        // Diffuseurs des codes arabes/égyptiens & haïtien (anciennement Inconnu, tick 39) :
        assert_eq!(source_authority("wipo.int"), Institutionnel);
        assert_eq!(source_authority("manshurat.org"), Agregateur);
        assert_eq!(source_authority("archive.org"), Agregateur);
        assert_eq!(source_authority("sami-aldeeb.com"), Prive);
        assert_eq!(source_authority("uaipit.com"), Prive);
    }
}
