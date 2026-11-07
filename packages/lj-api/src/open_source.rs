use std::sync::LazyLock;

use lj_dtos::{OpenSourceComponent, OpenSourceLink, OpenSourceResponse};

const LIBRARIES: &str = include_str!("../data/open_source_libraries.tsv");

const FONTS: &str = "/redactor/fonts/licenses/";
const HTR: &str = "/redactor/htr/models/licenses/";
const OCR: &str = "/ocr/licenses/";
const NOTICE: &str = "/redactor/NOTICE.txt";

/// Served as is, outside the crate graph: name, version, licence, then the folder and file of its text.
const SERVED: &[(&str, &str, &str, &str, &str)] = &[
    ("Adobe Core 14 AFM", "", "APAFML", NOTICE, ""),
    ("Adobe Glyph List", "2.0", "BSD-3-Clause", NOTICE, ""),
    ("base64-js", "1.5.1", "MIT", OCR, "base64-js-LICENSE.txt"),
    ("bmp-js", "0.1.0", "MIT", OCR, "bmp-js-LICENSE.txt"),
    ("buffer", "6.0.3", "MIT", OCR, "buffer-LICENSE.txt"),
    (
        "Caladea",
        "20130214",
        "Apache-2.0",
        FONTS,
        "Caladea-LICENSE.txt",
    ),
    (
        "Carlito",
        "20130920",
        "OFL-1.1",
        FONTS,
        "Carlito-LICENSE.txt",
    ),
    (
        "CMaps d'Adobe (cmap-resources)",
        "5d490e4b",
        "BSD-3-Clause",
        "/redactor/cmaps/",
        "LICENSE.md",
    ),
    (
        "DejaVu",
        "2.37",
        "Bitstream Vera, Arev",
        FONTS,
        "DejaVu-LICENSE.txt",
    ),
    ("EB Garamond", "", "OFL-1.1", FONTS, "EBGaramond-OFL.txt"),
    (
        "FrequencyWords",
        "525f9b56",
        "CC-BY-SA-4.0",
        HTR,
        "FrequencyWords-README.md",
    ),
    ("Geist", "", "OFL-1.1", "/fonts/", "geist-OFL.txt"),
    ("Gelasio", "", "OFL-1.1", FONTS, "Gelasio-OFL.txt"),
    ("giflib", "", "MIT", OCR, "giflib-COPYING.txt"),
    (
        "idb-keyval",
        "6.2.1",
        "Apache-2.0",
        OCR,
        "idb-keyval-LICENCE.txt",
    ),
    (
        "ieee754",
        "1.2.1",
        "BSD-3-Clause",
        OCR,
        "ieee754-LICENSE.txt",
    ),
    ("is-url", "1.2.4", "MIT", OCR, "is-url-LICENSE-MIT.txt"),
    (
        "Leptonica",
        "",
        "BSD-2-Clause",
        OCR,
        "leptonica-license.txt",
    ),
    (
        "Liberation Sans, Serif et Mono",
        "2.1.5",
        "OFL-1.1",
        FONTS,
        "Liberation-LICENSE.txt",
    ),
    (
        "Liberation Sans Narrow",
        "1.07.6",
        "GPL-2.0 avec exception de police",
        FONTS,
        "LiberationSansNarrow-License.txt",
    ),
    ("libjpeg", "", "IJG", OCR, "libjpeg-README.txt"),
    ("libpng", "", "libpng-2.0", OCR, "libpng-LICENSE.txt"),
    ("libtiff", "", "libtiff", OCR, "libtiff-COPYRIGHT.txt"),
    ("libwebp", "", "BSD-3-Clause", OCR, "libwebp-COPYING.txt"),
    (
        "Noto Sans et Noto Serif",
        "",
        "OFL-1.1",
        FONTS,
        "Noto-OFL.txt",
    ),
    ("office_oxide", "0.1.12", "MIT OR Apache-2.0", NOTICE, ""),
    ("open-redact-pdf", "0.6.0", "MIT OR Apache-2.0", NOTICE, ""),
    ("openlibm", "", "MIT, BSD, ISC", OCR, "openlibm-LICENSE.md"),
    (
        "OpenSymbol",
        "",
        "MPL-2.0",
        FONTS,
        "OpenSymbol-COPYING.MPL.txt",
    ),
    (
        "PP-OCRv5 mobile, détection",
        "e6f4fa85",
        "Apache-2.0",
        HTR,
        "PaddleOCR-LICENSE.txt",
    ),
    (
        "PP-OCRv5 mobile, reconnaissance latine",
        "7b02d0a3",
        "Apache-2.0",
        HTR,
        "PaddleOCR-LICENSE.txt",
    ),
    ("PT Sans", "", "OFL-1.1", FONTS, "PTSans-OFL.txt"),
    ("PyLaia RIMES", "270d9e1d", "MIT", HTR, "MIT.txt"),
    (
        "regenerator-runtime",
        "0.13.11",
        "MIT",
        OCR,
        "regenerator-runtime-LICENSE.txt",
    ),
    ("Selawik", "1.01", "OFL-1.1", FONTS, "Selawik-OFL.txt"),
    (
        "Tahoma de Wine",
        "",
        "LGPL-2.1-or-later",
        FONTS,
        "WineTahoma-COPYING.LIB.txt",
    ),
    (
        "tessdata_fast",
        "4.1.0",
        "Apache-2.0",
        OCR,
        "tessdata_fast-LICENSE.txt",
    ),
    ("Tesseract", "", "Apache-2.0", OCR, "tesseract-LICENSE.txt"),
    (
        "tesseract.js",
        "7.0.0",
        "Apache-2.0",
        OCR,
        "tesseract.js-LICENSE.md",
    ),
    (
        "tesseract.js-core",
        "7.0.0",
        "Apache-2.0",
        OCR,
        "tesseract.js-core-LICENSE.txt",
    ),
    (
        "URW base35 (C059, P052, Nimbus, URW Bookman, URW Gothic)",
        "",
        "licence avec exception de police",
        FONTS,
        "URW-base35-LICENSE.txt",
    ),
    (
        "wasm-feature-detect",
        "1.8.0",
        "Apache-2.0",
        OCR,
        "wasm-feature-detect-LICENSE.txt",
    ),
    ("zlib", "", "Zlib", OCR, "zlib-README.txt"),
    ("zlibjs", "0.3.1", "MIT", OCR, "zlibjs-LICENSE.txt"),
];

static RESPONSE: LazyLock<OpenSourceResponse> = LazyLock::new(|| {
    let served = SERVED
        .iter()
        .map(|&(name, version, licence, dir, file)| OpenSourceComponent {
            name: name.to_string(),
            version: version.to_string(),
            home: None,
            licence: vec![OpenSourceLink {
                label: licence.to_string(),
                href: (!dir.is_empty()).then(|| format!("{dir}{file}")),
            }],
            files: Vec::new(),
        });
    let libraries = LIBRARIES.lines().map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        let (name, version, licence, home, files) = match f[..] {
            [name, version, licence, home] => (name, version, licence, home, ""),
            [name, version, licence, home, files] => (name, version, licence, home, files),
            _ => panic!("open_source_libraries.tsv: four or five columns expected, got {line:?}"),
        };
        library(name, version, licence, home, files)
    });
    let mut components: Vec<OpenSourceComponent> = served.chain(libraries).collect();
    components.sort_by_cached_key(|c| c.name.to_lowercase());
    OpenSourceResponse { components }
});

pub fn response() -> &'static OpenSourceResponse {
    &RESPONSE
}

/// `files` maps a declared licence to the crate's own file (`MIT=<hash>`), and names the files no licence claims (`+NOTICE=<hash>`).
fn library(
    name: &str,
    version: &str,
    licence: &str,
    home: &str,
    files: &str,
) -> OpenSourceComponent {
    let links: Vec<(&str, &str)> = files.split(' ').filter_map(|l| l.split_once('=')).collect();
    let crate_text = |hash: &str| format!("/licenses/crates/{hash}.txt");
    let mut words = Vec::new();
    let mut rest = licence;
    while !rest.is_empty() {
        let end = rest.find([' ', '(', ')']).unwrap_or(rest.len()).max(1);
        let word = &rest[..end];
        let href = (!matches!(word, " " | "(" | ")" | "OR" | "AND" | "WITH")).then(|| match links
            .iter()
            .find(|(k, _)| *k == word)
        {
            Some((_, hash)) => crate_text(hash),
            None => format!("/licenses/{word}.txt"),
        });
        match (words.last_mut(), &href) {
            (Some(OpenSourceLink { label, href: None }), None) => label.push_str(word),
            _ => words.push(OpenSourceLink {
                label: word.to_string(),
                href,
            }),
        }
        rest = &rest[end..];
    }
    OpenSourceComponent {
        name: name.to_string(),
        version: version.to_string(),
        home: Some(home.to_string()),
        licence: words,
        files: links
            .iter()
            .filter_map(|&(k, hash)| {
                k.strip_prefix('+').map(|file| OpenSourceLink {
                    label: file.to_string(),
                    href: Some(crate_text(hash)),
                })
            })
            .collect(),
    }
}
