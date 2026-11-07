//! A BODACC file written in binary COPY and read back gives its notice whole.
//! Against an empty Postgres with ParadeDB and VectorChord, `LIBREJUSTICE_TEST_DB_URL`.

use std::collections::HashMap;

use lj_gazette::bodacc::Famille;
use lj_gazette::census::originals;
use lj_gazette::record::{recompose, Record};
use lj_gazette::rows::{parse_container, Codes, Rows, ADDRESS, BODACC};
use lj_gazette::Source;
use lj_pg::db::build_pool;
use lj_store::migrator::{apply_migrations, MigrationIntent};
use lj_store::repository::{DecisionRepository, GazetteAxis, GazetteFilter, GazetteQuery};

const RCS_B: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<RCS-B_REDIFF>
   <parution>20990001</parution>
   <dateParution>2099-01-02</dateParution>
   <listeAvis>
      <avis>
         <typeAnnonce><annonce/></typeAnnonce>
         <nojo>BXB990010001</nojo>
         <numeroAnnonce>17</numeroAnnonce>
         <numeroDepartement>75</numeroDepartement>
         <tribunal>GREFFE DU TRIBUNAL DE COMMERCE DE PARIS</tribunal>
         <personnes>
            <personne>
               <personneMorale>
                  <denomination>CENTRE DENTAIRE DU PARC</denomination>
                  <formeJuridique>Association</formeJuridique>
                  <administration>Président : DUPONT Jean, Trésorier : MARTIN Paul</administration>
               </personneMorale>
               <numeroImmatriculation>
                  <numeroIdentificationRCS>123 456 789</numeroIdentificationRCS>
                  <codeRCS>RCS</codeRCS>
                  <nomGreffeImmat>Paris</nomGreffeImmat>
               </numeroImmatriculation>
               <adresse><france><numeroVoie>3</numeroVoie><typeVoie>rue</typeVoie><nomVoie>des Lilas</nomVoie><codePostal>75019</codePostal><ville>Paris</ville></france></adresse>
            </personne>
         </personnes>
         <modificationsGenerales>
            <descriptif>modification survenue sur l'administration</descriptif>
            <dateEffet>2099-01-01</dateEffet>
         </modificationsGenerales>
      </avis>
   </listeAvis>
</RCS-B_REDIFF>"#;

#[tokio::test]
#[ignore = "needs LIBREJUSTICE_TEST_DB_URL"]
async fn a_written_file_comes_back_whole() {
    let pool = build_pool(&std::env::var("LIBREJUSTICE_TEST_DB_URL").unwrap(), 1).unwrap();
    let conn = pool.get().await.unwrap();
    apply_migrations(&conn, MigrationIntent::Apply)
        .await
        .unwrap();
    let repo = DecisionRepository::new(&conn);

    let mut codes = Codes::new(repo.gazette_last_address().await.unwrap());
    for (kind, code, value) in repo.gazette_dicts().await.unwrap() {
        codes.load(kind, code, value).unwrap();
    }
    let raw = vec![RCS_B.as_bytes().to_vec()];
    let parsed =
        parse_container(Source::Bodacc, "test/RCS-B_BXB20990001.taz".into(), &raw).unwrap();
    let mut rows = Rows::default();
    codes.container(Source::Bodacc, parsed, &mut rows).unwrap();
    rows.finish(&mut codes);
    repo.gazette_write(Source::Bodacc, &rows).await.unwrap();

    let stored = repo.gazette_rows(&BODACC.table, &rows.ids).await.unwrap();
    assert_eq!(stored.len(), 1);
    let ids: Vec<i32> = stored.iter().flat_map(|r| BODACC.address_ids(r)).collect();
    let addresses: HashMap<i32, Record> = repo
        .gazette_address_rows(&ids)
        .await
        .unwrap()
        .iter()
        .map(|r| {
            ADDRESS
                .record(r, &codes, &HashMap::new())
                .map(|(id, a)| (id as i32, a))
        })
        .collect::<Result<_, _>>()
        .unwrap();
    let (_, record) = BODACC.record(&stored[0], &codes, &addresses).unwrap();
    let (_, items) = originals(Source::Bodacc, RCS_B.as_bytes()).unwrap();
    assert_eq!(recompose(&record).unwrap(), items[0]);

    let by = |query| GazetteFilter {
        source: Source::Bodacc,
        query,
        classes: None,
        depts: None,
    };
    let siren = by(GazetteQuery::Siren(123_456_789));
    let notices = repo.bodacc_search(&siren, 10, 0).await.unwrap();
    assert_eq!(repo.gazette_total(&siren).await.unwrap(), 1);
    assert_eq!(notices[0].ville.as_deref(), Some("Paris"));
    assert_eq!(notices[0].name.as_deref(), Some("CENTRE DENTAIRE DU PARC"));
    assert_eq!(rows.officers.len(), 2);

    let words = GazetteFilter {
        classes: Some(vec![Famille::Modification as i16]),
        ..by(GazetteQuery::Text("dentaire parc".into()))
    };
    assert_eq!(repo.bodacc_search(&words, 10, 0).await.unwrap().len(), 1);
    let other_family = GazetteFilter {
        classes: Some(vec![Famille::Radiation as i16]),
        ..words.clone()
    };
    assert_eq!(repo.gazette_total(&other_family).await.unwrap(), 0);
    assert_eq!(
        repo.gazette_facet(&other_family, GazetteAxis::Class)
            .await
            .unwrap(),
        [(Famille::Modification as i16, 1)],
        "an axis is counted without its own filter"
    );
}
