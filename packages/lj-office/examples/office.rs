//! One Word file laid out by lj-office, one process per file for a bench:
//! `render` leaves `page-NNNN.png`, `render.json`, `text.txt` and
//! `outcome.json` in `--out`, and `rgb-NNNN.png` in colour with `--rgb-dpi`.

use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use resvg::{tiny_skia, usvg};
use serde::Serialize;

const USAGE: &str =
    "usage: office render --file F --format docx|odt|doc --out DIR --fonts DIR [--rgb-dpi N]";

/// Raster resolution of every page image, the LibreOffice reference's too.
const DPI: f64 = 48.0;

/// On one thread, resvg makes a long file outlast the deadline.
const RASTER_THREADS: usize = 4;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or_else(|| anyhow!("{USAGE}"))?;
    let (mut file, mut format, mut out, mut fonts, mut rgb_dpi) = (None, None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| anyhow!("{flag} takes a value\n{USAGE}"))?;
        match flag.as_str() {
            "--file" => file = Some(PathBuf::from(value)),
            "--format" => format = Some(value),
            "--out" => out = Some(PathBuf::from(value)),
            "--fonts" => fonts = Some(PathBuf::from(value)),
            "--rgb-dpi" => match value.parse::<f64>() {
                Ok(dpi) if dpi > 0.0 => rgb_dpi = Some(dpi),
                _ => bail!("--rgb-dpi {value}: a positive resolution\n{USAGE}"),
            },
            _ => bail!("unknown argument {flag}\n{USAGE}"),
        }
    }
    let (Some(file), Some(format), Some(out)) = (file, format, out) else {
        bail!("--file, --format and --out are required\n{USAGE}");
    };
    if !matches!(format.as_str(), "docx" | "odt" | "doc") {
        bail!("--format {format}: docx, odt or doc\n{USAGE}");
    }
    match (mode.as_str(), fonts) {
        ("render", Some(fonts)) => render(&file, &out, &fonts, rgb_dpi),
        _ => bail!("{USAGE}"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Ok,
    Failed,
}

#[derive(Serialize)]
struct Outcome {
    status: Status,
    detail: String,
    seconds: f64,
}

/// A word as the layout placed it: text and box in points, origin top-left, y down.
#[derive(Serialize)]
struct Word {
    text: String,
    bounds: [f64; 4],
}

#[derive(Serialize)]
struct Page {
    /// Width and height in points.
    size: [f64; 2],
    lines: Vec<Vec<Word>>,
}

#[derive(Serialize)]
struct Render {
    pages: Vec<Page>,
}

fn render(file: &Path, out: &Path, fonts: &Path, rgb_dpi: Option<f64>) -> Result<()> {
    let started = Instant::now();
    std::fs::create_dir_all(out)?;
    let outcome = match draw(&std::fs::read(file)?, fonts, out, rgb_dpi) {
        Ok((render, text)) => {
            std::fs::write(out.join("render.json"), serde_json::to_vec(&render)?)?;
            std::fs::write(out.join("text.txt"), text)?;
            Outcome {
                status: Status::Ok,
                detail: String::new(),
                seconds: started.elapsed().as_secs_f64(),
            }
        }
        Err(e) => Outcome {
            status: Status::Failed,
            detail: format!("{e:#}").chars().take(400).collect(),
            seconds: started.elapsed().as_secs_f64(),
        },
    };
    std::fs::write(
        out.join("outcome.json"),
        serde_json::to_vec_pretty(&outcome)?,
    )?;
    Ok(())
}

struct Laid {
    /// Width and height of each page, in points.
    sizes: Vec<(f64, f64)>,
    svgs: Vec<String>,
    /// Each page's words in reading order, boxes top-left origin.
    words: Vec<Vec<(String, [f64; 4])>>,
    text: String,
    /// The font files the document embeds, which its pages may draw with.
    embedded: Vec<Arc<[u8]>>,
}

fn layout(bytes: &[u8], fonts: &Path) -> Result<Laid> {
    let document = lj_office::open(bytes)?;
    let mut faces = lj_office::Fonts::from_dir(fonts)?;
    for file in &document.embedded_fonts {
        faces.add(file.to_vec())?;
    }
    let laid = document.layout(&faces);
    let count = laid.page_count();
    Ok(Laid {
        sizes: (0..count).map(|p| laid.page_size(p)).collect(),
        svgs: (0..count).map(|p| laid.svg(p)).collect(),
        words: (0..count).map(|p| laid.words(p)).collect(),
        text: document.text(),
        embedded: document.embedded_fonts.clone(),
    })
}

fn draw(bytes: &[u8], fonts: &Path, out: &Path, rgb_dpi: Option<f64>) -> Result<(Render, String)> {
    let Laid {
        sizes,
        svgs,
        words,
        text,
        embedded,
    } = layout(bytes, fonts)?;
    let mut options = usvg::Options::default();
    by_postscript_name(options.fontdb_mut(), fonts, &embedded)?;
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..RASTER_THREADS)
            .map(|_| {
                scope.spawn(|| -> Result<()> {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(svg) = svgs.get(i) else {
                            return Ok(());
                        };
                        let (width, height) = sizes[i];
                        let tree = usvg::Tree::from_str(svg, &options)?;
                        let gray = rasterise(&tree, width, height, DPI)?;
                        write_png(&gray, png::ColorType::Grayscale, &out.join(page_png(i)))?;
                        if let Some(dpi) = rgb_dpi {
                            let colour = if dpi == DPI {
                                gray
                            } else {
                                rasterise(&tree, width, height, dpi)?
                            };
                            write_png(&colour, png::ColorType::Rgb, &out.join(rgb_png(i)))?;
                        }
                    }
                })
            })
            .collect();
        workers.into_iter().try_for_each(|worker| {
            worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    })?;

    let pages = sizes
        .iter()
        .zip(&words)
        .map(|(&(width, height), words)| Page {
            size: [width, height],
            lines: lines(words, height),
        })
        .collect();
    Ok((Render { pages }, text))
}

fn page_png(page: usize) -> String {
    format!("page-{:04}.png", page + 1)
}

fn rgb_png(page: usize) -> String {
    format!("rgb-{:04}.png", page + 1)
}

/// The SVG names each face by its PostScript name alone. The files are read
/// once, fontdb mapping a file anew at each use of a face it loaded from disk.
fn by_postscript_name(
    db: &mut usvg::fontdb::Database,
    fonts: &Path,
    embedded: &[Arc<[u8]>],
) -> Result<()> {
    for entry in std::fs::read_dir(fonts)? {
        let path = entry?.path();
        let font = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"));
        if font {
            db.load_font_data(std::fs::read(path)?);
        }
    }
    for file in embedded {
        db.load_font_data(file.to_vec());
    }
    let faces: Vec<usvg::fontdb::FaceInfo> = db.faces().cloned().collect();
    for mut face in faces {
        db.remove_face(face.id);
        face.families = vec![(
            face.post_script_name.clone(),
            usvg::fontdb::Language::English_UnitedStates,
        )];
        db.push_face_info(face);
    }
    Ok(())
}

fn rasterise(tree: &usvg::Tree, width: f64, height: f64, dpi: f64) -> Result<tiny_skia::Pixmap> {
    let scale = dpi / 72.0;
    let (w, h) = (
        ((width * scale).round() as u32).max(1),
        ((height * scale).round() as u32).max(1),
    );
    let mut pixmap =
        tiny_skia::Pixmap::new(w, h).ok_or_else(|| anyhow!("a {w}x{h} page has no pixmap"))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let size = tree.size();
    let transform =
        tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}

fn write_png(pixmap: &tiny_skia::Pixmap, colour: png::ColorType, path: &Path) -> Result<()> {
    let pixels = pixmap.pixels().iter().map(|p| p.demultiply());
    let data: Vec<u8> = match colour {
        png::ColorType::Grayscale => pixels
            .map(|c| {
                ((299 * c.red() as u32 + 587 * c.green() as u32 + 114 * c.blue() as u32) / 1000)
                    as u8
            })
            .collect(),
        png::ColorType::Rgb => pixels
            .flat_map(|c| [c.red(), c.green(), c.blue()])
            .collect(),
        _ => bail!("{colour:?} is not a page colour type"),
    };
    let file = BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, pixmap.width(), pixmap.height());
    encoder.set_color(colour);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&data)?;
    Ok(())
}

/// Two words share a line when their boxes overlap over half the smaller
/// height and the second does not jump back left, as for PDF glyphs.
fn same_line(previous: &[f64; 4], next: &[f64; 4]) -> bool {
    let overlap = previous[3].min(next[3]) - previous[1].max(next[1]);
    let smaller = (previous[3] - previous[1])
        .min(next[3] - next[1])
        .max(f64::EPSILON);
    overlap >= 0.5 * smaller && next[0] >= previous[0] - smaller
}

/// The words of a page in lines, those wholly above or below it left out as
/// pdftotext leaves them.
fn lines(words: &[(String, [f64; 4])], height: f64) -> Vec<Vec<Word>> {
    let mut lines: Vec<Vec<Word>> = Vec::new();
    let shown = |b: &[f64; 4]| b[3] > 0.0 && b[1] < height;
    for (text, bounds) in words.iter().filter(|(_, b)| shown(b)) {
        let continues = lines
            .last()
            .and_then(|line| line.last())
            .is_some_and(|previous| same_line(&previous.bounds, bounds));
        if !continues {
            lines.push(Vec::new());
        }
        lines.last_mut().expect("a line was opened").push(Word {
            text: text.clone(),
            bounds: *bounds,
        });
    }
    lines
}
