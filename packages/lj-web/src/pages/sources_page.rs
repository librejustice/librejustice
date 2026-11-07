//! Section ids are link targets: the code pages link to `/sources#dila`.

use leptos::prelude::*;
use leptos_meta::Title;

const LICENCE_OUVERTE: &str = "https://www.etalab.gouv.fr/licence-ouverte-open-licence/";

#[component]
pub fn SourcesPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    view! {
        <Title text="Données & sources - LibreJustice" />
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Données & sources"</h1>
            <p class="mt-3 max-w-prose text-[var(--color-ink-muted)]">
                "LibreJustice agrège des données juridiques publiques : décisions de justice, "
                "textes (codes, lois, traités, droit de l'Union), avis de marchés publics et "
                "registres. Chaque source est présentée ci-dessous avec son origine et ses "
                "conditions de réutilisation. LibreJustice n'en modifie pas le contenu et ne "
                "garantit ni l'exhaustivité ni l'actualité du corpus."
            </p>

            <Group titre="Jurisprudence" />

            <Source
                id="judilibre"
                titre="Judilibre, Cour de cassation"
                href="https://www.courdecassation.fr/acces-rapide-judilibre"
            >
                <p>
                    "Décisions de l'ordre judiciaire (Cour de cassation, cours d'appel, "
                    "tribunaux judiciaires et de commerce, conseils de prud'hommes), "
                    "diffusées en open data par la Cour de cassation via l'API Judilibre, "
                    "pseudonymisées avant publication. LibreJustice en ingère l'intégralité "
                    "du flux et le resynchronise quotidiennement. L'open data judiciaire "
                    "s'élargit progressivement : le pénal de première instance est attendu "
                    "en 2027 ("
                    <Ext href="https://www.justice.gouv.fr/documentation/open-data-decisions-justice">
                        "calendrier d'ouverture"
                    </Ext>
                    ")."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte 2.0"</Ext>
                    ", complétée par les "
                    <Ext href="https://www.courdecassation.fr/conditions-generales-dutilisation-pour-la-reutilisation-des-donnees-issues-des-decisions-de-justice">
                        "conditions générales d'utilisation"
                    </Ext>
                    " de la Cour de cassation."
                </Conditions>
            </Source>

            <Source
                id="opendata-administratif"
                titre="Open data de la justice administrative, Conseil d'État"
                href="https://opendata.justice-administrative.fr/"
            >
                <p>
                    "Décisions de l'ordre administratif (Conseil d'État, cours "
                    "administratives d'appel, tribunaux administratifs), diffusées par le "
                    "Conseil d'État en archives ZIP XML mises à jour quotidiennement et "
                    "pseudonymisées avant publication. LibreJustice les resynchronise "
                    "chaque jour. Le flux fournit le texte intégral des décisions, mais ni "
                    "les analyses ni les conclusions des rapporteurs publics, qui ne sont "
                    "pas diffusées en open data. Les analyses viennent d'ArianeWeb, "
                    "présenté plus bas."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte 2.0"</Ext>
                    ", complétée par les "
                    <Ext href="https://opendata.justice-administrative.fr/cgu.html">
                        "conditions générales d'utilisation"
                    </Ext>
                    " du Conseil d'État."
                </Conditions>
            </Source>

            <Source
                id="arianeweb"
                titre="ArianeWeb, Conseil d'État"
                href="https://www.conseil-etat.fr/arianeweb/"
            >
                <p>
                    "ArianeWeb est la base de jurisprudence du Conseil d'État. LibreJustice "
                    "y reprend les analyses rédigées pour ses décisions (fonds AJCE), "
                    "l'existence des conclusions du rapporteur public et quelques décisions "
                    "absentes des autres canaux. Il les rattache aux décisions et les "
                    "resynchronise chaque jour. La liste des grandes décisions publiée par le "
                    "Conseil d'État donne aussi leur nom aux décisions qu'elle recense."
                </p>
                <Conditions>
                    "informations publiques librement réutilisables, à condition de ne pas "
                    "les altérer et d'en indiquer la source et la date, selon les "
                    <Ext href="https://www.conseil-etat.fr/mentions-legales">
                        "mentions légales du Conseil d'État"
                    </Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="cnda"
                titre="CNDA, Cour nationale du droit d'asile"
                href="https://www.cnda.fr/decisions-de-justice/jurisprudence"
            >
                <p>
                    "Les décisions de la CNDA ne figurent ni dans Judilibre ni dans l'open "
                    "data administratif. LibreJustice reprend celles que la Cour publie sur "
                    "son site avec l'analyse qu'elle en rédige. Le texte intégral est extrait "
                    "du document joint. La reprise est quotidienne."
                </p>
                <Conditions>
                    "celles du site de la CNDA, fixées par ses "
                    <Ext href="https://www.cnda.fr/mentions-legales">"mentions légales"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="ddd"
                titre="Défenseur des droits"
                href="https://juridique.defenseurdesdroits.fr"
            >
                <p>
                    "Le Défenseur des droits publie dans sa base juridique ses décisions et "
                    "celles des autorités auxquelles il a succédé en 2011 : la HALDE, la CNDS, "
                    "le Défenseur des enfants et le Médiateur de la République. LibreJustice en "
                    "reprend les décisions avec leur notice. Le texte intégral est extrait du "
                    "document joint. La reprise est quotidienne."
                </p>
                <Conditions>
                    "informations publiques librement réutilisables au sens du code des "
                    "relations entre le public et les administrations, selon les "
                    <Ext href="https://www.defenseurdesdroits.fr/mentions-legales-367">
                        "mentions légales du Défenseur des droits"
                    </Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="adlc"
                titre="Autorité de la concurrence"
                href="https://www.data.gouv.fr/fr/datasets/decisions-publiees-par-lautorite-de-la-concurrence-depuis-1988/"
            >
                <p>
                    "L'Autorité de la concurrence publie sur data.gouv.fr le texte intégral "
                    "de ses décisions et de celles du Conseil de la concurrence depuis 1988. "
                    "LibreJustice en reprend les décisions contentieuses, les mesures "
                    "conservatoires et les décisions de contrôle des concentrations. Les avis "
                    "ne sont pas repris. La reprise est hebdomadaire, au rythme du jeu de données."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte 2.0"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="dila-jurisprudence"
                titre="DILA, fonds de jurisprudence"
                href="https://echanges.dila.gouv.fr/OPENDATA/"
            >
                <p>
                    "La Direction de l'information légale et administrative diffuse aussi des "
                    "fonds de décisions. LibreJustice en reprend JADE (jurisprudence "
                    "administrative, dont le Conseil d'État avec ses analyses), CONSTIT "
                    "(décisions du Conseil constitutionnel), CNIL (délibérations de la CNIL, "
                    "dont ses sanctions) et les fonds judiciaires CASS, INCA et CAPP. JADE, "
                    "CONSTIT et CNIL sont resynchronisés chaque jour. Une décision reçue par "
                    "plusieurs canaux n'a qu'une fiche."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="hudoc"
                titre="HUDOC, Cour européenne des droits de l'homme"
                href="https://hudoc.echr.coe.int/fre"
            >
                <p>
                    "Arrêts et décisions de la Cour européenne des droits de l'homme, "
                    "diffusés par la Cour dans sa base HUDOC. LibreJustice en reprend les "
                    "versions française et anglaise et les resynchronise chaque jour."
                </p>
                <Conditions>
                    "la Cour autorise la reproduction de ses textes, source mentionnée, à "
                    "titre gratuit et à des fins d'usage privé, d'information et "
                    "d'éducation. Les autres usages demandent son autorisation écrite ("
                    <Ext href="https://www.echr.coe.int/fr/copyright-and-disclaimer">
                        "conditions de la Cour"
                    </Ext>
                    ")."
                </Conditions>
            </Source>

            <Group titre="Normes" />

            <Source
                id="dila"
                titre="DILA, bases juridiques de l'État"
                href="https://echanges.dila.gouv.fr/OPENDATA/"
            >
                <p>
                    "La Direction de l'information légale et administrative diffuse en open "
                    "data les bases du droit français. LibreJustice en reprend LEGI (codes et "
                    "lois consolidés, versionnés par article : chaque article se lit en "
                    "vigueur ou à une date passée), le JORF (Journal officiel, dont les "
                    "traités publiés par décret), KALI (conventions collectives nationales) "
                    "et CIRCULAIRES (circulaires et instructions des ministères). Ces fonds "
                    "sont resynchronisés chaque jour à partir des incréments de la DILA."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="eur-lex"
                titre="EUR-Lex, droit de l'Union et CJUE"
                href="https://eur-lex.europa.eu/"
            >
                <p>
                    "L'Office des publications de l'Union européenne diffuse le droit de "
                    "l'Union et la jurisprudence de ses juridictions. LibreJustice en reprend "
                    "les règlements et directives et, pour les actes consolidés, leurs "
                    "versions successives : chaque article se lit à la date demandée. Il "
                    "reprend aussi les arrêts, ordonnances et conclusions des avocats "
                    "généraux de la Cour de justice de l'Union européenne, dans leur langue "
                    "de procédure, en français et en anglais. Les deux fonds sont "
                    "resynchronisés chaque jour."
                </p>
                <Conditions>
                    "réutilisation libre, y compris commerciale, selon l'"
                    <Ext href="https://eur-lex.europa.eu/content/legal-notice/legal-notice.html">
                        "avis juridique d'EUR-Lex"
                    </Ext>
                    ". Les textes consolidés sont sous licence CC BY 4.0."
                </Conditions>
            </Source>

            <Source
                id="curia"
                titre="Curia, site de la Cour de justice de l'Union européenne"
                href="https://curia.europa.eu/"
            >
                <p>
                    "La Cour de justice de l'Union européenne publie sur son site le texte "
                    "intégral de ses décisions. LibreJustice y reprend celles qu'EUR-Lex ne "
                    "diffuse pas et celles dont il ne donne que le résumé, dans les mêmes "
                    "langues."
                </p>
                <Conditions>
                    "reproduction autorisée avec mention de la source, selon la "
                    <Ext href="https://curia.europa.eu/site/jcms/d2_5167/fr/copyright">
                        "page droits d'auteur de la Cour"
                    </Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="comeu"
                titre="Commission européenne, affaires de concurrence"
                href="https://competition-cases.ec.europa.eu/"
            >
                <p>
                    "La Commission européenne publie ses décisions en matière d'ententes et "
                    "d'abus de position dominante, de concentrations, d'aides d'État, de marchés "
                    "numériques et de subventions étrangères. LibreJustice reprend chaque décision "
                    "publiée avec son document, dans sa langue d'origine, en français et en "
                    "anglais. La reprise est quotidienne."
                </p>
                <Conditions>
                    <Ext href="https://creativecommons.org/licenses/by/4.0/deed.fr">
                        "CC BY 4.0"
                    </Ext>
                    ", selon l'"
                    <Ext href="https://commission.europa.eu/legal-notice_fr">
                        "avis juridique de la Commission"
                    </Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="juricaf"
                titre="JuriCAF, Association des hautes juridictions de cassation francophones"
                href="https://juricaf.org/"
            >
                <p>
                    "JuriCAF est la base de jurisprudence de l'AHJUCAF, alimentée par les cours "
                    "suprêmes membres. LibreJustice en reprend les décisions qu'aucune source "
                    "officielle ne diffuse en ligne : cours suprêmes d'une trentaine de pays, "
                    "et juridictions régionales africaines (OHADA, UEMOA, CEMAC, CEDEAO, Cour "
                    "africaine des droits de l'homme et des peuples). La reprise est quotidienne."
                </p>
                <Conditions>
                    <Ext href="https://opendatacommons.org/licenses/odbl/1-0/">"ODbL 1.0"</Ext>
                    ", selon les "
                    <Ext href="https://juricaf.org/static/donneespersonnelles">
                        "mentions légales de JuriCAF"
                    </Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="bofip"
                titre="BOFiP-Impôts, doctrine fiscale"
                href="https://bofip.impots.gouv.fr/"
            >
                <p>
                    "Le Bulletin officiel des finances publiques porte la doctrine fiscale "
                    "de la DGFiP, opposable à l'administration. LibreJustice reprend les "
                    "publications en vigueur, diffusées en open data par le ministère de "
                    "l'Économie."
                </p>
                <Conditions>
                    <Ext href="https://data.economie.gouv.fr/explore/dataset/bofip-vigueur/information/">
                        "Licence Ouverte 2.0"
                    </Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="jafbase"
                titre="JaFBase, droit international privé de la famille"
                href="http://jafbase.fr"
            >
                <p>
                    "Base bénévole tenue depuis 2008 par Cyril Roth, magistrat, l'une des "
                    "seules à rendre accessible le droit étranger de la famille. Avec "
                    "l'autorisation de l'auteur, LibreJustice y reprend les codes étrangers "
                    "(famille, état des personnes, code civil) découpés article par article "
                    "et reliés aux décisions qui les citent. JaFBase n'est pas qu'un recueil "
                    "de textes : c'est aussi une méthode, une "
                    <Ext href="http://jafbase.fr/instruments">"carte mentale"</Ext>
                    " pour déterminer la règle de conflit applicable. Ces textes ne sont ni "
                    "exhaustifs ni garantis à jour."
                </p>
                <Conditions>"reprise avec l'autorisation de l'auteur."</Conditions>
            </Source>

            <Group titre="Commentaires de décisions" />

            <Source id="hal" titre="HAL, archive ouverte" href="https://hal.science/">
                <p>
                    "LibreJustice relie aux décisions les notes d'arrêt, chroniques et thèses "
                    "déposées dans HAL qui les commentent. Les documents restent sur HAL ou chez "
                    "leur éditeur et LibreJustice n'en reprend que le lien."
                </p>
                <Conditions>"celles que fixe l'auteur ou l'éditeur de chaque document."</Conditions>
            </Source>

            <Source id="cairn" titre="Cairn" href="https://www.cairn.info/">
                <p>
                    "Les notes d'arrêt du Droit Ouvrier et de la Revue critique de droit "
                    "international privé sont reliées aux décisions qu'elles commentent, à partir "
                    "des métadonnées que Cairn publie. Les articles restent sur Cairn, certains "
                    "réservés aux abonnés, et LibreJustice n'en reprend que le lien."
                </p>
                <Conditions>
                    "métadonnées en libre moissonnage ; pour les articles, celles de Cairn et de "
                    "chaque revue."
                </Conditions>
            </Source>

            <Source id="hypotheses" titre="Hypothèses, carnets de recherche" href="https://hypotheses.org/">
                <p>
                    "Les notes des carnets "
                    <Ext href="https://dsf.hypotheses.org/">"Droit du travail et de la protection sociale"</Ext>
                    " et "
                    <Ext href="https://m2dsplille.hypotheses.org/">"Droit de la protection sociale"</Ext>
                    ", de l'université de Lille, sont reliées de la même façon, à partir des "
                    "métadonnées qu'OpenEdition publie."
                </p>
                <Conditions>"métadonnées sous licence CC0 ; pour les billets, celles de chaque carnet."</Conditions>
            </Source>

            <Source
                id="ajdc"
                titre="Actualité juridique du dommage corporel"
                href="https://publications-prairial.fr/ajdc/"
            >
                <p>
                    "Les commentaires de la revue de l'université Lumière Lyon 2 sont reliés aux "
                    "décisions qu'ils commentent, lus sur la page de chaque article."
                </p>
                <Conditions>"licence CC BY."</Conditions>
            </Source>

            <Group titre="Marchés publics" />

            <Source id="boamp" titre="BOAMP" href="https://www.boamp.fr/">
                <p>
                    "Avis de marchés publics en cours et avis d'attribution du Bulletin "
                    "officiel des annonces des marchés publics, relevés chaque jour."
                </p>
                <Conditions>
                    <Ext href="https://www.data.gouv.fr/fr/datasets/boamp/">"Licence Ouverte"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source id="ted" titre="TED" href="https://ted.europa.eu/fr/">
                <p>
                    "Les avis français du TED, supplément au Journal officiel de l'Union "
                    "européenne, relevés chaque jour."
                </p>
                <Conditions>
                    "librement réutilisables, y compris à des fins commerciales ("
                    <Ext href="https://ted.europa.eu/fr/legal-notice">"conditions de TED"</Ext>
                    ")."
                </Conditions>
            </Source>

            <Source
                id="decp"
                titre="Données essentielles de la commande publique (DECP)"
                href="https://data.economie.gouv.fr/explore/dataset/decp-2022-marches-valides/information/"
            >
                <p>"Elles donnent le titulaire des marchés conclus."</p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte 2.0"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source id="plateformes" titre="Plateformes des acheteurs">
                <p>
                    "LibreJustice relève aussi les plateformes de dématérialisation des "
                    "acheteurs, où paraissent beaucoup de marchés à procédure adaptée absents "
                    "du BOAMP. Une consultation vue sur plusieurs canaux fait une seule fiche, "
                    "qui renvoie au dossier sur la plateforme de l'acheteur."
                </p>
            </Source>

            <Group titre="Entreprises, associations et avocats" />

            <Source
                id="sirene"
                titre="SIRENE, Insee"
                href="https://www.data.gouv.fr/fr/datasets/base-sirene-des-entreprises-et-de-leurs-etablissements-siren-siret/"
            >
                <p>
                    "Le répertoire des entreprises et de leurs établissements, tenu par "
                    "l'Insee. LibreJustice en reprend les unités légales diffusibles, sociétés "
                    "comme entrepreneurs individuels, et en fait ses fiches entité."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte 2.0"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="rna"
                titre="Répertoire national des associations (RNA)"
                href="https://www.data.gouv.fr/fr/datasets/repertoire-national-des-associations/"
            >
                <p>
                    "Tenu par le ministère de l'Intérieur, il recense les associations "
                    "déclarées. LibreJustice en tire une fiche par association."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="avocats"
                titre="Annuaire des avocats, Conseil national des barreaux"
                href="https://www.data.gouv.fr/fr/datasets/annuaire-des-avocats-de-france/"
            >
                <p>
                    "L'annuaire national publié par le Conseil national des barreaux recense "
                    "les avocats inscrits aux barreaux français. LibreJustice le relit chaque "
                    "semaine et en tire une fiche par avocat."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte 2.0"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="recherche-entreprises"
                titre="API Recherche d'entreprises"
                href="https://recherche-entreprises.api.gouv.fr/"
            >
                <p>
                    "Le service public qui alimente l'Annuaire des entreprises. À l'affichage, "
                    "la fiche d'une entreprise y lit son identité à jour, ses dirigeants et "
                    "ses données financières."
                </p>
            </Source>

            <Source id="bodacc" titre="BODACC" href="https://www.bodacc.fr/">
                <p>
                    "Le Bulletin officiel des annonces civiles et commerciales, publié par la "
                    "DILA, annonce les immatriculations, modifications, ventes et procédures "
                    "collectives des entreprises. La fiche d'une entreprise en affiche la "
                    "chronologie, lue au moment de l'affichage."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte"</Ext>
                    "."
                </Conditions>
            </Source>

            <Source
                id="joafe"
                titre="Journal officiel des associations et fondations d'entreprise (JOAFE)"
                href="https://www.journal-officiel.gouv.fr/associations/"
            >
                <p>
                    "Publié par la DILA, il annonce la création, les modifications et la "
                    "dissolution des associations. La fiche d'une association en affiche les "
                    "annonces."
                </p>
                <Conditions>
                    <Ext href=LICENCE_OUVERTE>"Licence Ouverte"</Ext>
                    "."
                </Conditions>
            </Source>
        </div>
    }
}

#[component]
fn Group(titre: &'static str) -> impl IntoView {
    view! {
        <h2 class="mt-12 border-b border-[var(--color-rule)] pb-2 font-sans text-xl text-[var(--color-ink)]">
            {titre}
        </h2>
    }
}

#[component]
fn Source(
    id: &'static str,
    titre: &'static str,
    #[prop(optional)] href: Option<&'static str>,
    children: Children,
) -> impl IntoView {
    view! {
        <section id=id class="mt-8 scroll-mt-24">
            <h3 class="font-sans text-base font-semibold text-[var(--color-ink)]">
                {match href {
                    Some(href) => {
                        view! {
                            <a
                                class="underline-offset-4 hover:underline"
                                href=href
                                rel="noreferrer"
                                target="_blank"
                            >
                                {titre}
                            </a>
                        }
                            .into_any()
                    }
                    None => titre.into_any(),
                }}
            </h3>
            <div class="mt-2 max-w-prose space-y-2 text-[var(--color-ink-muted)]">{children()}</div>
        </section>
    }
}

#[component]
fn Conditions(children: Children) -> impl IntoView {
    view! {
        <p class="text-sm">
            <span class="text-[var(--color-ink)]">"Conditions de réutilisation : "</span>
            {children()}
        </p>
    }
}

#[component]
fn Ext(href: &'static str, children: Children) -> impl IntoView {
    view! {
        <a
            class="text-[var(--color-accent)] underline-offset-4 hover:underline"
            href=href
            rel="noreferrer"
            target="_blank"
        >
            {children()}
        </a>
    }
}
