use std::collections::HashMap;

use crate::bodacc::{self, Avis, Famille};
use crate::officers::{self, Officer};
use crate::record::{self, format_date, parse_date, Cell};
use crate::tree::{self, Node};
use crate::{rows, Source};

const PCL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<PCL_REDIFF xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="Bodacc_A_PCL_sor_v175.xsd">
   <parution>20260999</parution>
   <dateParution>2026-10-04</dateParution>
   <annonces>
      <annonce>
         <typeAnnonce><creation/></typeAnnonce>
         <nojo>000000099999999</nojo>
         <numeroAnnonce>2759</numeroAnnonce>
         <numeroDepartement>03</numeroDepartement>
         <tribunal>TRIBUNAL DE COMMERCE DE MONTLUÇON</tribunal>
         <personneMorale>
            <denomination>ÉTABLISSEMENTS DURAND &amp; FILS</denomination>
            <formeJuridique>Société à responsabilité limitée</formeJuridique>
         </personneMorale>
         <numeroImmatriculation>
            <numeroIdentificationRCS>123 456 789</numeroIdentificationRCS>
            <codeRCS>RCS</codeRCS>
            <nomGreffeImmat>Montluçon</nomGreffeImmat>
         </numeroImmatriculation>
         <activite>Entretien et réparation de véhicules</activite>
         <adresse>
            <france>
               <numeroVoie>12</numeroVoie>
               <typeVoie>rue</typeVoie>
               <nomVoie>des Tilleuls</nomVoie>
               <codePostal>03100</codePostal>
               <ville>Montluçon</ville>
            </france>
         </adresse>
         <jugement>
            <famille>Jugement d'ouverture</famille>
            <nature>Jugement d'ouverture de liquidation judiciaire</nature>
            <date>2026-09-23</date>
            <complementJugement>Jugement prononçant la liquidation judiciaire.</complementJugement>
         </jugement>
      </annonce>
      <annonce>
         <typeAnnonce><rectificatif/></typeAnnonce>
         <nojo>X</nojo>
         <numeroAnnonce>2760</numeroAnnonce>
         <numeroDepartement>03</numeroDepartement>
         <tribunal>TRIBUNAL DE COMMERCE DE MONTLUÇON</tribunal>
         <numeroImmatriculation>
            <numeroIdentificationRCS>RCS non inscrit</numeroIdentificationRCS>
         </numeroImmatriculation>
         <jugement>
            <famille>Avis de conciliation</famille>
            <date>2026-02-30</date>
         </jugement>
         <parutionAvisPrecedent><numeroAnnonce>12</numeroAnnonce></parutionAvisPrecedent>
      </annonce>
   </annonces>
</PCL_REDIFF>"#;

fn notices(xml: &str) -> (bodacc::File, Vec<Node>) {
    let root = tree::parse(xml).unwrap();
    let (_, items) = bodacc::split(root.clone()).unwrap();
    (bodacc::parse_file(root).unwrap(), items)
}

#[test]
fn a_bodacc_notice_comes_back_whole_from_its_record() {
    let (file, items) = notices(PCL);
    assert_eq!(file.notices.len(), 2);
    for (n, original) in file.notices.iter().zip(&items) {
        assert_eq!(&record::recompose(&n.record).unwrap(), original);
    }
}

#[test]
fn a_bodacc_notice_spreads_into_its_columns() {
    let (file, _) = notices(PCL);
    let n = &file.notices[0];
    let col = |name| n.record.cells[bodacc::SPEC.column(name)].clone();
    assert_eq!(n.id, 100_000_000_000_000 + 20_260_999 * 1_000_000 + 2759);
    assert_eq!(bodacc::split_id(n.id), ('A', 20_260_999, 2759));
    assert_eq!(col("siren"), Some(Cell::Siren(123_456_789)));
    assert_eq!(
        col("name"),
        Some(Cell::Text("ÉTABLISSEMENTS DURAND & FILS".into()))
    );
    assert_eq!(col("jug_date"), parse_date("2026-09-23").map(Cell::Date));
    let Some(Cell::Address(a)) = col("address") else {
        panic!("no address")
    };
    assert_eq!(
        a.text(crate::address::SPEC.column("ville")),
        Some("Montluçon")
    );
    assert_eq!((n.famille, n.avis), (Famille::Collective, Avis::Initial));
    let r = &file.notices[1];
    assert_eq!(r.record.cells[bodacc::SPEC.column("siren")], None);
    assert_eq!(
        r.record.cells[bodacc::SPEC.column("jug_date")],
        None,
        "an impossible date stays text"
    );
    assert_eq!(
        (r.famille, r.avis),
        (Famille::Conciliation, Avis::Rectificatif)
    );
}

#[test]
fn the_header_keeps_the_root_and_drops_the_notices() {
    let (file, _) = notices(PCL);
    let header = record::recompose(&file.header).unwrap();
    assert_eq!(header.name, "PCL_REDIFF");
    assert_eq!(header.attrs.len(), 2);
    assert!(header.child("annonces").unwrap().children().is_empty());
    assert_eq!(format_date(file.date), "2026-10-04");
}

#[test]
fn values_of_every_codec_round_trip() {
    let xml =
        r#"<a k=""><n>0042</n><n>7</n><d>1901-08-27</d><s>123456789</s><t>  padded  </t><e/></a>"#;
    let node = tree::parse(xml).unwrap();
    static COLS: &[record::Column] = &[
        record::Column {
            name: "n",
            codec: record::Codec::Int,
            paths: &["n"],
        },
        record::Column {
            name: "n2",
            codec: record::Codec::Int,
            paths: &["n"],
        },
        record::Column {
            name: "d",
            codec: record::Codec::Date,
            paths: &["d"],
        },
        record::Column {
            name: "s",
            codec: record::Codec::Siren,
            paths: &["s"],
        },
    ];
    static SPEC: record::Spec = record::Spec::new(COLS);
    let rec = record::decompose(&node, &SPEC).unwrap();
    assert_eq!(rec.shape, "a@k$(n%0.4,n%1,d%2,s%3.1,t$,e)");
    assert_eq!(rec.rest.as_deref(), Some("\u{1f}  padded  "));
    assert_eq!(record::recompose(&rec).unwrap(), node);
    assert_eq!(record::rest_paths(&rec.shape).unwrap(), vec!["@k", "t"]);
}

#[test]
fn every_written_form_of_a_date_or_a_siren_comes_back_as_written() {
    static COLS: &[record::Column] = &[
        record::Column {
            name: "d",
            codec: record::Codec::Date,
            paths: &["v"],
        },
        record::Column {
            name: "s",
            codec: record::Codec::Siren,
            paths: &["w"],
        },
    ];
    static SPEC: record::Spec = record::Spec::new(COLS);
    let typed = [
        ("v", "2008-03-01"),
        ("v", "01/03/2008"),
        ("v", "2008/03/01"),
        ("v", "4/04/2007"),
        ("v", "1er\u{a0}mars\u{a0}2008"),
        ("v", "1\u{a0}mars\u{a0}2008"),
        ("v", "15 août 2008"),
        ("v", "1er décembre 2007"),
        ("w", "123 456 789"),
        ("w", "123456789"),
        ("w", "123\u{a0}456\u{a0}789"),
    ];
    let kept = [
        ("v", "01\u{a0}mars\u{a0}2008"),
        ("v", "1er Mars 2008"),
        ("v", "31 février 2008"),
        ("v", "1er\u{a0}mars 2008"),
        ("v", "0/04/2007"),
        ("w", "123 4567 89"),
        ("w", "123\u{a0}456 789"),
    ];
    for (tag, value, is_typed) in typed
        .iter()
        .map(|&(t, v)| (t, v, true))
        .chain(kept.iter().map(|&(t, v)| (t, v, false)))
    {
        let node = tree::parse(&format!("<a><{tag}>{value}</{tag}></a>")).unwrap();
        let rec = record::decompose(&node, &SPEC).unwrap();
        assert_eq!(rec.rest.is_none(), is_typed, "{value}");
        assert_eq!(record::recompose(&rec).unwrap(), node, "{value}");
    }
}

#[test]
fn latin1_files_decode_byte_for_byte() {
    let raw = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>cr\xe9ation</a>";
    let node = tree::parse(&tree::decode(raw).unwrap()).unwrap();
    assert_eq!(node.text(), Some("création"));
    assert!(tree::decode(b"<a>cr\xe9ation</a>").is_err());
}

#[test]
fn officers_take_their_role_from_the_segment_before() {
    let o = |role: &str, name: &str| Officer {
        role: role.into(),
        name: name.into(),
    };
    assert_eq!(
        officers::parse("Président : DUPONT Jean, Directeur général : MARTIN Paul, Commissaire aux comptes titulaire : AUDIT SA"),
        vec![o("president", "DUPONT Jean"), o("directeur general", "MARTIN Paul"), o("commissaire aux comptes titulaire", "AUDIT SA")]
    );
    assert_eq!(
        officers::parse(
            "Gérant partant : DURAND Marie nom d'usage : LEROY, Nomination du Gérant : PETIT Luc"
        ),
        vec![
            o("gerant", "DURAND Marie (LEROY)"),
            o("gerant", "PETIT Luc")
        ]
    );
    assert_eq!(
        officers::parse("modification de la date de clôture"),
        Vec::new()
    );
}

#[test]
fn a_notice_comes_back_whole_from_its_stored_row() {
    let root = tree::parse(PCL).unwrap();
    let (_, items) = bodacc::split(root.clone()).unwrap();
    let raw = PCL.as_bytes().to_vec();
    let parsed = rows::parse_container(Source::Bodacc, "x.taz".into(), &[raw]).unwrap();
    let mut codes = rows::Codes::new(0);
    let mut out = rows::Rows::default();
    codes.container(Source::Bodacc, parsed, &mut out).unwrap();
    out.finish(&mut codes);
    assert_eq!(
        (out.notices.len(), out.addresses.len(), out.files.len()),
        (2, 1, 1)
    );
    let addresses: HashMap<i32, record::Record> = out
        .addresses
        .iter()
        .map(|r| {
            rows::ADDRESS
                .record(r, &codes, &HashMap::new())
                .map(|(id, a)| (id as i32, a))
        })
        .collect::<Result<_, _>>()
        .unwrap();
    for (row, original) in out.notices.iter().zip(&items) {
        let (_, rec) = rows::BODACC.record(row, &codes, &addresses).unwrap();
        assert_eq!(&record::recompose(&rec).unwrap(), original);
    }
}

#[test]
fn an_address_key_never_changes() {
    let node = tree::parse("<adresse><france><ville>Montluçon</ville></france></adresse>").unwrap();
    let rec = record::decompose(&node, &crate::address::SPEC).unwrap();
    assert_eq!(rows::address_key(&rec), 4_188_680_534_107_952_685);
}

#[test]
fn stray_text_between_notices_stays_in_the_header() {
    let xml = PCL.replace(
        "</annonce>\n      <annonce>",
        "</annonce>nce>\n      <annonce>",
    );
    let root = tree::parse(&xml).unwrap();
    let (header, items) = bodacc::split(root).unwrap();
    assert_eq!(items.len(), 2);
    let list = header.child("annonces").unwrap();
    assert_eq!(list.children()[0].name, tree::STRAY_TEXT);
    assert_eq!(list.children()[0].text(), Some("nce>\n      "));
}

#[test]
fn an_homologated_agreement_is_a_conciliation() {
    for nature in [
        "Jugement d'homologation de l'accord",
        "Jugement de résolution de l'accord homologué",
    ] {
        let xml = PCL.replace("Jugement d'ouverture de liquidation judiciaire", nature);
        let (file, _) = notices(&xml);
        assert_eq!(file.notices[0].famille, Famille::Conciliation, "{nature}");
    }
}

#[test]
fn a_joafe_notice_takes_its_action_from_its_element_or_else_its_type_code() {
    let annonce = |num: u32, code: &str, contenu: &str| {
        format!(
            "<annonce><metadonnees><numAnnonce>{num}</numAnnonce><type code=\"{code}\"/></metadonnees>\
             <contenu>{contenu}</contenu></annonce>"
        )
    };
    let xml = format!(
        "<parution><dateParution>2026-10-06</dateParution><numParution>20260040</numParution><listeAnnonces>{}</listeAnnonces></parution>",
        [
            annonce(1, "1", "<assoLoi1901><dateDeclaration>2026-09-23</dateDeclaration><creation><titre>A</titre></creation></assoLoi1901>"),
            annonce(2, "909", "<fondDotation><dateDeclaration>2026-09-23</dateDeclaration><suspension><titre>B</titre></suspension></fondDotation>"),
            annonce(3, "201", "<assoSyndicale><titre>C</titre></assoSyndicale>"),
            annonce(4, "906", "<fondDotation><titre>D</titre></fondDotation>"),
            annonce(5, "2009", "<fondPerennite><titre>E</titre></fondPerennite>"),
            annonce(6, "900", "<fondDotation><titre>F</titre></fondDotation>"),
            annonce(7, "8", "<assoLoi1901><titre>G</titre></assoLoi1901>"),
        ]
        .concat()
    );
    let doc = crate::joafe::parse(tree::parse(&xml).unwrap()).unwrap();
    use crate::joafe::Action::*;
    assert_eq!(
        doc.notices.iter().map(|n| n.action).collect::<Vec<_>>(),
        [
            Creation,
            Suspension,
            Modification,
            Dissolution,
            Saisine,
            Creation,
            Autre
        ]
    );
}

#[test]
fn a_joafe_department_reads_as_the_official_code() {
    use crate::canonical_dept;
    for (written, code) in [
        ("1", "01"),
        ("01", "01"),
        ("20A", "2A"),
        ("2B", "2B"),
        ("971", "971"),
        ("20", "20"),
    ] {
        assert_eq!(canonical_dept(written), code);
    }
}
