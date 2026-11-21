//! Section ids are link targets: a code page links to `/sources#dila`, a decision page to its source's section.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::components::{prose, LINK};
use crate::i18n::SourceCopy;

#[component]
pub fn SourcesPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let ui = &crate::i18n::use_ui().sources;
    view! {
        <Title text=ui.title />
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">{ui.heading}</h1>
            <p class="mt-3 max-w-prose text-[var(--color-ink-muted)]">{ui.intro}</p>

            <Group titre=ui.case_law />

            <Source id="judilibre" href="https://www.courdecassation.fr/acces-rapide-judilibre" copy=&ui.judilibre />
            <Source id="opendata-administratif" href="https://opendata.justice-administrative.fr/" copy=&ui.administrative_open_data />
            <Source id="arianeweb" href="https://www.conseil-etat.fr/arianeweb/" copy=&ui.arianeweb />
            <Source id="consiliaweb" href="https://www.conseil-etat.fr/avis-consultatifs/rechercher-un-avis-consiliaweb" copy=&ui.consiliaweb />
            <Source id="sites-juridictions-administratives" copy=&ui.administrative_court_sites />
            <Source id="site-conseil-constitutionnel" copy=&ui.constitutional_council_site />
            <Source id="cnda" href="https://www.cnda.fr/decisions-de-justice/jurisprudence" copy=&ui.cnda />
            <Source id="ddd" href="https://juridique.defenseurdesdroits.fr" copy=&ui.ddd />
            <Source id="adlc" href="https://www.data.gouv.fr/fr/datasets/decisions-publiees-par-lautorite-de-la-concurrence-depuis-1988/" copy=&ui.adlc />
            <Source id="amf" href="https://www.amf-france.org/fr/sanction-transaction/Decisions-de-la-commission-des-sanctions" copy=&ui.amf />
            <Source id="dila-jurisprudence" href="https://echanges.dila.gouv.fr/OPENDATA/" copy=&ui.dila_case_law />
            <Source id="hudoc" href="https://hudoc.echr.coe.int/fre" copy=&ui.hudoc />
            <Source id="contribution" copy=&ui.contribution />

            <Group titre=ui.norms />

            <Source id="dila" href="https://echanges.dila.gouv.fr/OPENDATA/" copy=&ui.dila />
            <Source id="eur-lex" href="https://eur-lex.europa.eu/" copy=&ui.eur_lex />
            <Source id="curia" href="https://curia.europa.eu/" copy=&ui.curia />
            <Source id="comeu" href="https://competition-cases.ec.europa.eu/" copy=&ui.comeu />
            <Source id="juricaf" href="https://juricaf.org/" copy=&ui.juricaf />
            <Source id="entscheidsuche" href="https://entscheidsuche.ch/" copy=&ui.entscheidsuche />
            <Source id="opencaselaw" href="https://opencaselaw.ch/" copy=&ui.opencaselaw />
            <Source id="legimonaco" href="https://legimonaco.mc/" copy=&ui.legimonaco />
            <Source id="juportal" href="https://juportal.be/" copy=&ui.juportal />
            <Source id="rechtspraak" href="https://uitspraken.rechtspraak.nl/" copy=&ui.rechtspraak />
            <Source id="cce" href="https://www.rvv-cce.be/fr/arr" copy=&ui.cce />
            <Source id="justice-lu" href="https://data.public.lu/fr/organizations/administration-judiciaire/" copy=&ui.justice_lu />
            <Source id="cour-supreme-canada" href="https://decisions.scc-csc.ca/scc-csc/scc-csc/fr/nav_date.do" copy=&ui.supreme_court_canada />
            <Source id="a2aj" href="https://a2aj.ca/" copy=&ui.a2aj />
            <Source id="sites-juridictions-africaines" copy=&ui.african_court_sites />
            <Source id="journal-officiel-djibouti" href="https://www.journalofficiel.dj/" copy=&ui.djibouti_journal />
            <Source id="legalis" href="https://www.legalis.net/" copy=&ui.legalis />
            <Source id="bofip" href="https://bofip.impots.gouv.fr/" copy=&ui.bofip />
            <Source id="boss" href="https://boss.gouv.fr/" copy=&ui.boss />
            <Source id="bulletins-officiels" copy=&ui.bulletins />
            <Source id="acpr" href="https://acpr.banque-france.fr/fr/reglementation/registre-officiel" copy=&ui.acpr />
            <Source id="jafbase" href="http://jafbase.fr" copy=&ui.jafbase />

            <Group titre=ui.commentaries />

            <Source id="hal" href="https://hal.science/" copy=&ui.hal />
            <Source id="cairn" href="https://www.cairn.info/" copy=&ui.cairn />
            <Source id="hypotheses" href="https://hypotheses.org/" copy=&ui.hypotheses />
            <Source id="ajdc" href="https://publications-prairial.fr/ajdc/" copy=&ui.ajdc />

            <Group titre=ui.procurement />

            <Source id="boamp" href="https://www.boamp.fr/" copy=&ui.boamp />
            <Source id="ted" href="https://ted.europa.eu/fr/" copy=&ui.ted />
            <Source id="decp" href="https://data.economie.gouv.fr/explore/dataset/decp-2022-marches-valides/information/" copy=&ui.decp />
            <Source id="plateformes" copy=&ui.buyer_platforms />

            <Group titre=ui.registers />

            <Source id="sirene" href="https://www.data.gouv.fr/fr/datasets/base-sirene-des-entreprises-et-de-leurs-etablissements-siren-siret/" copy=&ui.sirene />
            <Source id="rna" href="https://www.data.gouv.fr/fr/datasets/repertoire-national-des-associations/" copy=&ui.rna />
            <Source id="avocats" href="https://www.data.gouv.fr/fr/datasets/annuaire-des-avocats-de-france/" copy=&ui.lawyers />
            <Source id="recherche-entreprises" href="https://recherche-entreprises.api.gouv.fr/" copy=&ui.company_search />
            <Source id="bodacc" href="https://www.bodacc.fr/" copy=&ui.bodacc />
            <Source id="joafe" href="https://www.journal-officiel.gouv.fr/associations/" copy=&ui.joafe />
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
    #[prop(optional)] href: Option<&'static str>,
    copy: &'static SourceCopy,
) -> impl IntoView {
    let reuse = crate::i18n::use_ui().sources.reuse;
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
                                {copy.title}
                            </a>
                        }
                            .into_any()
                    }
                    None => copy.title.into_any(),
                }}
            </h3>
            <div class="mt-2 max-w-prose space-y-2 text-[var(--color-ink-muted)]">
                {copy.body.iter().map(|runs| view! { <p>{prose(runs, LINK)}</p> }).collect_view()}
                {(!copy.reuse.is_empty())
                    .then(|| {
                        view! {
                            <p class="text-sm">
                                <span class="text-[var(--color-ink)]">{reuse}</span>
                                {prose(copy.reuse, LINK)}
                            </p>
                        }
                    })}
            </div>
        </section>
    }
}
