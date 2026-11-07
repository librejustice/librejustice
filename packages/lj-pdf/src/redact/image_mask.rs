//! Partial image rewriting.
//!
//! When a redaction target only partly covers an image, the image is decoded
//! as the draw decodes it, the targeted samples are painted with the plan's
//! fill colour (a mask's with what shows the image), and the image is stored
//! back as `FlateDecode`, at its own bit depth, in its own colour space, under
//! its own Decode array, a one bit image as CCITT Group 4 when that is
//! smaller. A JPEG whose coefficients read stays a JPEG: the blocks under
//! the rectangles become flat, every other block keeps its bits. An image whose codec does not decode returns
//! [`PdfError::Unsupported`], and the caller stops drawing it.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::draw::color::ColorSpace;
use crate::draw::image::{
    Codec, Samples, dct, decoded, encoded, inline_dict, is_stencil, space_and_decode,
};
use crate::graphics::Color;
use crate::objects::{
    ObjectRef, PdfDictionary, PdfError, PdfFile, PdfObject, PdfResult, PdfStream, PdfValue,
};

/// Pixel-space rectangle. `(x, y)` is the top-left corner; `w` and `h`
/// are the width and height in pixels, on the grid of the image's declared
/// `/Width` and `/Height`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImagePixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// What the painted samples show.
#[derive(Clone, Copy)]
enum Paint {
    /// The fill colour, or for a stencil mask its painted side.
    Fill(Color),
    /// Full opacity, for a soft mask.
    Opaque,
}

/// An image stored back, its dictionary with full keys, and the alpha a
/// JPEG 2000 image carried, to become its soft mask.
struct Rewritten {
    dict: PdfDictionary,
    data: Vec<u8>,
    alpha: Option<(u32, u32, Vec<u8>)>,
}

/// Entries that name pixels the rewrite does not paint, or a codec it
/// removes.
const STALE_KEYS: [&str; 6] = [
    "Filter",
    "DecodeParms",
    "Length",
    "SMaskInData",
    "Alternates",
    "OPI",
];

/// Paint `rects` in the Image XObject `stream` and in the masks it names.
/// The masks painted and the soft mask a JPEG 2000 image carried are new
/// objects of `file`, which the returned stream names.
pub(crate) fn mask_image_xobject(
    file: &mut PdfFile,
    stream: &PdfStream,
    resources: Option<&PdfDictionary>,
    rects: &[ImagePixelRect],
    fill: Color,
) -> PdfResult<PdfStream> {
    let dict = &stream.dict;
    let grid = grid(file, dict)?;
    let smask = stream_behind(file, dict.get("SMask"));
    let mask = stream_behind(file, dict.get("Mask"));
    let jpx_alpha = smask.is_none()
        && dict
            .get("SMaskInData")
            .and_then(|value| file.resolve(value).ok())
            .and_then(PdfValue::as_integer)
            > Some(0);
    let paint = Paint::Fill(fill);
    let image = match jpeg_in_place(file, dict, &stream.data, resources, rects, grid, paint)? {
        Some(image) => image,
        None => rewrite(
            file,
            dict,
            &stream.data,
            resources,
            rects,
            grid,
            paint,
            jpx_alpha,
        )?,
    };
    let mut painted_masks = Vec::new();
    for (key, mask, paint) in [
        ("SMask", smask, Paint::Opaque),
        ("Mask", mask, Paint::Fill(fill)),
    ] {
        if let Some(mask) = mask {
            let painted =
                match jpeg_in_place(file, &mask.dict, &mask.data, None, rects, grid, paint)? {
                    Some(painted) => painted,
                    None => rewrite(
                        file, &mask.dict, &mask.data, None, rects, grid, paint, false,
                    )?,
                };
            painted_masks.push((key, painted));
        }
    }
    let mut new_dict = image.dict;
    for (key, painted) in painted_masks {
        let mask_ref = insert(file, painted.dict, painted.data);
        new_dict.insert(key.into(), PdfValue::Reference(mask_ref));
    }
    if let Some((width, height, alpha)) = image.alpha {
        let mut smask = PdfDictionary::new();
        for (key, value) in [
            ("Type", PdfValue::Name("XObject".into())),
            ("Subtype", PdfValue::Name("Image".into())),
            ("Width", PdfValue::Integer(width.into())),
            ("Height", PdfValue::Integer(height.into())),
            ("ColorSpace", PdfValue::Name("DeviceGray".into())),
            ("BitsPerComponent", PdfValue::Integer(8)),
            ("Filter", PdfValue::Name("FlateDecode".into())),
            ("DecodeParms", predictor(1, 8, width)),
        ] {
            smask.insert(key.into(), value);
        }
        let data = deflate_rows_up(&alpha, width as usize);
        let smask_ref = insert(file, smask, data);
        new_dict.insert("SMask".into(), PdfValue::Reference(smask_ref));
    }
    Ok(PdfStream {
        dict: new_dict,
        data: image.data,
    })
}

/// Paint `rects` in an inline image, `written` its dictionary as the content
/// stream spells it. The data comes back in ASCIIHex, in which no `EI` can
/// end the image early.
pub(crate) fn mask_inline_image(
    file: &PdfFile,
    written: &PdfDictionary,
    data: &[u8],
    resources: &PdfDictionary,
    rects: &[ImagePixelRect],
    fill: Color,
) -> PdfResult<(PdfDictionary, Vec<u8>)> {
    let dict = inline_dict(written);
    let grid = grid(file, &dict)?;
    let paint = Paint::Fill(fill);
    let image = match jpeg_in_place(file, &dict, data, Some(resources), rects, grid, paint)? {
        Some(image) => image,
        None => rewrite(
            file,
            &dict,
            data,
            Some(resources),
            rects,
            grid,
            paint,
            false,
        )?,
    };
    let codec = match image.dict.get("Filter").and_then(PdfValue::as_name) {
        Some("DCTDecode") => "DCT",
        Some("CCITTFaxDecode") => "CCF",
        _ => "Fl",
    };
    let parms = image.dict.get("DecodeParms").cloned();
    let mut abbreviated = PdfDictionary::new();
    for (key, value) in image.dict {
        let key = match key.as_str() {
            "Width" => "W",
            "Height" => "H",
            "BitsPerComponent" => "BPC",
            "ColorSpace" => "CS",
            "ImageMask" => "IM",
            "Decode" => "D",
            "Interpolate" => "I",
            "Filter" | "DecodeParms" => continue,
            other => other,
        };
        abbreviated.insert(key.to_string(), value);
    }
    abbreviated.insert(
        "F".into(),
        PdfValue::Array(vec![
            PdfValue::Name("AHx".into()),
            PdfValue::Name(codec.into()),
        ]),
    );
    abbreviated.insert(
        "DP".into(),
        PdfValue::Array(vec![PdfValue::Null, parms.unwrap_or(PdfValue::Null)]),
    );
    let mut hex = Vec::with_capacity(image.data.len() * 2 + 1);
    for byte in &image.data {
        hex.extend_from_slice(format!("{byte:02X}").as_bytes());
    }
    hex.push(b'>');
    Ok((abbreviated, hex))
}

/// Decode the image as the draw does, paint `rects`, given on `grid`, and
/// store it back as Flate. A codec that does not decode is `Unsupported`.
#[allow(clippy::too_many_arguments)]
fn rewrite(
    file: &PdfFile,
    dict: &PdfDictionary,
    data: &[u8],
    resources: Option<&PdfDictionary>,
    rects: &[ImagePixelRect],
    grid: (u32, u32),
    paint: Paint,
    jpx_alpha: bool,
) -> PdfResult<Rewritten> {
    // The limit the draw puts on a raw image. A JPEG 2000 image of 12608 x
    // 16806 overflows the decoder's buffers in wasm32, reduced or not.
    if u64::from(grid.0) * u64::from(grid.1) > lj_image::MAX_PIXELS {
        return Err(PdfError::Unsupported("image over the pixel limit".into()));
    }
    let decoded = decoded(file, dict, data, resources, jpx_alpha)
        .map_err(|error| PdfError::Unsupported(format!("image does not decode: {error}")))?;
    let mut samples = decoded.samples;
    if samples.components == 0 {
        return Err(PdfError::Unsupported("image without components".into()));
    }
    let rects: Vec<ImagePixelRect> = rects
        .iter()
        .map(|rect| scale(*rect, grid, (samples.width, samples.height)))
        .collect();
    let value = match &decoded.space {
        None => vec![u16::from(decoded.decode.first() == Some(&1.0))],
        Some(space) => sample_value(space, &decoded.decode, samples.bits, paint),
    };
    for rect in &rects {
        paint_samples(&mut samples, *rect, &value);
    }
    let alpha = samples.alpha.take().map(|mut alpha| {
        let width = samples.width as usize;
        alpha.resize(width * samples.height as usize, 255);
        for rect in &rects {
            for y in rect.y..rect.y + rect.h {
                let row = y as usize * width;
                alpha[row + rect.x as usize..row + (rect.x + rect.w) as usize].fill(255);
            }
        }
        (samples.width, samples.height, alpha)
    });

    let mut new = dict.clone();
    for key in STALE_KEYS {
        new.remove(key);
    }
    new.insert("Width".into(), PdfValue::Integer(samples.width.into()));
    new.insert("Height".into(), PdfValue::Integer(samples.height.into()));
    new.insert(
        "BitsPerComponent".into(),
        PdfValue::Integer(samples.bits.into()),
    );
    if decoded.space.is_some() {
        let named = dict
            .get("ColorSpace")
            .and_then(|space| ColorSpace::parse(file, space, resources).ok())
            .map(|space| space.components());
        if named != Some(samples.components) {
            let device = match samples.components {
                1 => "DeviceGray",
                4 => "DeviceCMYK",
                _ => "DeviceRGB",
            };
            new.insert("ColorSpace".into(), PdfValue::Name(device.into()));
        }
    }
    if dict.contains_key("Decode") {
        let default = match &decoded.space {
            Some(space) => space.default_decode(samples.bits),
            None => vec![0.0, 1.0],
        };
        if decoded.decode == default {
            new.remove("Decode");
        } else {
            let values = decoded.decode.iter().map(|v| PdfValue::Number(*v));
            new.insert("Decode".into(), PdfValue::Array(values.collect()));
        }
    }
    let fax = (samples.bits == 1 && samples.components == 1).then(|| group4(&samples));
    // Group 4 under an eighth of the raw rows beats Flate on one bit images;
    // above, a dithered page can favour Flate.
    let (filter, data) = match fax {
        Some((parms, fax)) if fax.len() <= samples.data.len() / 8 => {
            (("CCITTFaxDecode", parms), fax)
        }
        fax => {
            let flate = deflate_rows_up(&samples.data, samples.stride());
            match fax {
                Some((parms, fax)) if fax.len() < flate.len() => (("CCITTFaxDecode", parms), fax),
                _ => {
                    let parms = predictor(samples.components, samples.bits, samples.width);
                    (("FlateDecode", parms), flate)
                }
            }
        }
    };
    new.insert("Filter".into(), PdfValue::Name(filter.0.into()));
    new.insert("DecodeParms".into(), filter.1);
    Ok(Rewritten {
        dict: new,
        data,
        alpha,
    })
}

/// The samples each component takes under a painted pixel.
fn sample_value(space: &ColorSpace, decode: &[f64], bits: u8, paint: Paint) -> Vec<u16> {
    let max = (1u32 << bits) - 1;
    let fill = match paint {
        Paint::Opaque => return vec![invert_decode(1.0, decode[0], decode[1], max)],
        Paint::Fill(fill) => fill,
    };
    let template = |components: u8| pixel_template(components, fill).map(|v| f64::from(v) / 255.0);
    let target = match space {
        ColorSpace::Gray => Some(template(1)[..1].to_vec()),
        ColorSpace::Rgb => Some(template(3)[..3].to_vec()),
        ColorSpace::Cmyk => Some(template(4).to_vec()),
        _ => None,
    };
    if let Some(target) = target {
        return target
            .iter()
            .enumerate()
            .map(|(c, t)| invert_decode(*t, decode[2 * c], decode[2 * c + 1], max))
            .collect();
    }
    // Other spaces: the candidate whose colour lies nearest the fill, among
    // every palette entry of an Indexed space, the corners of the others.
    let n = space.components();
    let candidates: Vec<Vec<u16>> = match space {
        ColorSpace::Indexed { .. } => (0..=max.min(255) as u16).map(|s| vec![s]).collect(),
        _ => (0..1u32 << n.min(8))
            .map(|corner| {
                (0..n)
                    .map(|c| match c < 8 && corner & (1 << c) != 0 {
                        true => max as u16,
                        false => 0,
                    })
                    .collect()
            })
            .collect(),
    };
    let distance = |candidate: &Vec<u16>| {
        let values: Vec<f64> = candidate
            .iter()
            .enumerate()
            .map(|(c, s)| {
                let (low, high) = (decode[2 * c], decode[2 * c + 1]);
                low + f64::from(*s) * (high - low) / f64::from(max)
            })
            .collect();
        let rgb = space.rgb_bytes(&values);
        [fill.r, fill.g, fill.b]
            .iter()
            .zip(rgb)
            .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
            .sum::<i32>()
    };
    candidates
        .into_iter()
        .min_by_key(distance)
        .expect("a colour space has a candidate")
}

/// The sample whose Decode mapping lands nearest `target`.
fn invert_decode(target: f64, low: f64, high: f64, max: u32) -> u16 {
    if (high - low).abs() < f64::EPSILON {
        return 0;
    }
    ((target - low) / (high - low) * f64::from(max))
        .round()
        .clamp(0.0, f64::from(max)) as u16
}

fn paint_samples(samples: &mut Samples, rect: ImagePixelRect, value: &[u16]) {
    let stride = samples.stride();
    let components = samples.components;
    let bits = usize::from(samples.bits);
    for y in rect.y..rect.y + rect.h {
        let row = &mut samples.data[y as usize * stride..(y as usize + 1) * stride];
        for x in rect.x..rect.x + rect.w {
            for (c, &v) in value.iter().enumerate().take(components) {
                let index = x as usize * components + c;
                match bits {
                    8 => row[index] = v as u8,
                    16 => row[2 * index..2 * index + 2].copy_from_slice(&v.to_be_bytes()),
                    _ => {
                        let bit = index * bits;
                        let shift = 8 - bits - bit % 8;
                        let ones = ((1u16 << bits) - 1) as u8;
                        let byte = &mut row[bit / 8];
                        *byte = (*byte & !(ones << shift)) | ((v as u8 & ones) << shift);
                    }
                }
            }
        }
    }
}

/// `rect` on the `from` grid, carried to the `to` grid and grown to whole
/// pixels there.
fn scale(rect: ImagePixelRect, from: (u32, u32), to: (u32, u32)) -> ImagePixelRect {
    let axis = |start: u32, length: u32, from: u32, to: u32| {
        let (from, to) = (u64::from(from), u64::from(to));
        let low = (u64::from(start) * to / from).min(to);
        let high = (u64::from(start + length) * to).div_ceil(from).min(to);
        (low as u32, (high - low) as u32)
    };
    let (x, w) = axis(rect.x, rect.w, from.0, to.0);
    let (y, h) = axis(rect.y, rect.h, from.1, to.1);
    ImagePixelRect { x, y, w, h }
}

/// The declared `/Width` and `/Height`, the grid the rectangles are given on.
fn grid(file: &PdfFile, dict: &PdfDictionary) -> PdfResult<(u32, u32)> {
    let side = |key: &str| {
        dict.get(key)
            .and_then(|value| file.resolve(value).ok())
            .and_then(PdfValue::as_integer)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|&n| n > 0)
            .ok_or_else(|| PdfError::Unsupported(format!("image without a /{key}")))
    };
    Ok((side("Width")?, side("Height")?))
}

fn predictor(colors: usize, bits: u8, columns: u32) -> PdfValue {
    let parms = [
        ("Predictor", 12),
        ("Colors", colors as i64),
        ("BitsPerComponent", i64::from(bits)),
        ("Columns", i64::from(columns)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), PdfValue::Integer(value)))
    .collect();
    PdfValue::Dictionary(parms)
}

fn stream_behind(file: &PdfFile, value: Option<&PdfValue>) -> Option<PdfStream> {
    match value? {
        PdfValue::Reference(object_ref) => match file.get_object(*object_ref).ok()? {
            PdfObject::Stream(stream) => Some(stream.clone()),
            PdfObject::Value(_) => None,
        },
        _ => None,
    }
}

fn insert(file: &mut PdfFile, dict: PdfDictionary, data: Vec<u8>) -> ObjectRef {
    let object_ref = file.allocate_object_ref();
    file.objects
        .insert(object_ref, PdfObject::Stream(PdfStream { dict, data }));
    object_ref
}

/// The rows under the PNG Up filter, deflated at the fastest level: several times
/// faster than unfiltered rows at the default level, for about the same size.
fn deflate_rows_up(pixels: &[u8], stride: usize) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::with_capacity(pixels.len() / 4), Compression::fast());
    let mut line = vec![0u8; stride + 1];
    line[0] = 2;
    let zero = vec![0u8; stride];
    let mut above: &[u8] = &zero;
    for row in pixels.chunks_exact(stride) {
        for ((out, value), up) in line[1..].iter_mut().zip(row).zip(above) {
            *out = value.wrapping_sub(*up);
        }
        encoder
            .write_all(&line)
            .expect("writing to a Vec does not fail");
        above = row;
    }
    encoder.finish().expect("writing to a Vec does not fail")
}

/// One bit rows as CCITT Group 4, the majority colour coded as white.
fn group4(samples: &Samples) -> (PdfValue, Vec<u8>) {
    let ones: u64 = samples.data.iter().map(|b| u64::from(b.count_ones())).sum();
    let black_is_1 = ones * 2 < samples.data.len() as u64 * 8;
    let fax = match black_is_1 {
        true => {
            let inverted: Vec<u8> = samples.data.iter().map(|b| !b).collect();
            lj_image::g4::encode(&inverted, samples.width, samples.height)
        }
        false => lj_image::g4::encode(&samples.data, samples.width, samples.height),
    };
    let parms = [
        ("K", PdfValue::Integer(-1)),
        ("Columns", PdfValue::Integer(samples.width.into())),
        ("Rows", PdfValue::Integer(samples.height.into())),
        ("BlackIs1", PdfValue::Bool(black_is_1)),
        ("EncodedByteAlign", PdfValue::Bool(false)),
        ("EndOfBlock", PdfValue::Bool(true)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    (PdfValue::Dictionary(parms), fax)
}

/// How a JPEG stores the colours the draw decodes, as the draw's decoder
/// chooses.
enum Stored {
    Direct,
    YCbCr,
    Ycck,
}

fn stored(jpeg: &lj_image::jpeg::Jpeg, parms: &PdfDictionary) -> Option<Stored> {
    let count = jpeg.components.len();
    if jpeg.adobe.is_none() && parms.get("ColorTransform").and_then(PdfValue::as_integer) == Some(0)
    {
        return Some(Stored::Direct);
    }
    let ids: Vec<u8> = jpeg.components.iter().map(|c| c.id).collect();
    match (count, jpeg.adobe) {
        (1, _) => Some(Stored::Direct),
        (3, _) if ids == [1, 2, 3] => Some(Stored::YCbCr),
        (3, _) if ids == [82, 71, 66] => Some(Stored::Direct),
        (3, _) if ids == [1, 34, 35] || ids == [114, 103, 98] => None,
        (3, _) if jpeg.jfif => Some(Stored::YCbCr),
        (3, None | Some(1)) => Some(Stored::YCbCr),
        (3, Some(2)) | (4, Some(1)) => None,
        (3, Some(_)) => Some(Stored::Direct),
        (4, Some(2)) => Some(Stored::Ycck),
        (4, _) => Some(Stored::Direct),
        _ => None,
    }
}

/// JFIF's RGB to YCbCr.
fn ycbcr(r: f64, g: f64, b: f64) -> [u8; 3] {
    let byte = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    [
        byte(0.299 * r + 0.587 * g + 0.114 * b),
        byte(128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b),
        byte(128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b),
    ]
}

/// A JPEG whose coefficients read, its blocks under `rects` made flat blocks
/// of the paint and written back, every other block keeping its bits. `None`
/// sends it to [`rewrite`]; a zone the draw's decoder does not read back
/// painted is `Unsupported`, and the caller stops drawing the image.
fn jpeg_in_place(
    file: &PdfFile,
    dict: &PdfDictionary,
    data: &[u8],
    resources: Option<&PdfDictionary>,
    rects: &[ImagePixelRect],
    grid: (u32, u32),
    paint: Paint,
) -> PdfResult<Option<Rewritten>> {
    let Some((image, parms, value)) = flat_jpeg(file, dict, data, resources, rects, grid, paint)
    else {
        return Ok(None);
    };
    match dct(&image.data, &parms).is_ok_and(|samples| painted(&samples, rects, &value)) {
        true => Ok(Some(image)),
        false => Err(PdfError::Unsupported(
            "JPEG zone does not read back painted".into(),
        )),
    }
}

/// Whether every sample under `rects` lies within 8 levels of `value`.
fn painted(samples: &Samples, rects: &[ImagePixelRect], value: &[u16]) -> bool {
    let (n, stride) = (samples.components, samples.stride());
    samples.bits == 8
        && n == value.len()
        && rects.iter().all(|rect| {
            (rect.y..rect.y + rect.h).all(|y| {
                (rect.x..rect.x + rect.w).all(|x| {
                    let at = y as usize * stride + x as usize * n;
                    value.iter().enumerate().all(|(c, &v)| {
                        samples
                            .data
                            .get(at + c)
                            .is_some_and(|&s| u16::from(s).abs_diff(v) <= 8)
                    })
                })
            })
        })
}

/// The JPEG with its blocks under `rects` flat, the parameters its decoder
/// takes and the samples the paint gives.
fn flat_jpeg(
    file: &PdfFile,
    dict: &PdfDictionary,
    data: &[u8],
    resources: Option<&PdfDictionary>,
    rects: &[ImagePixelRect],
    grid: (u32, u32),
    paint: Paint,
) -> Option<(Rewritten, PdfDictionary, Vec<u16>)> {
    let filter = dict.get("Filter").and_then(|f| file.resolve(f).ok());
    let last = match filter {
        Some(PdfValue::Array(items)) => items.last().and_then(|f| file.resolve(f).ok()),
        other => other,
    };
    if !matches!(last.and_then(PdfValue::as_name), Some("DCTDecode" | "DCT"))
        || is_stencil(file, dict)
    {
        return None;
    }
    let stream = encoded(file, dict, data).ok()?;
    let Some((Codec::Dct, parms)) = stream.codec else {
        return None;
    };
    let mut jpeg = lj_image::jpeg::read(&stream.bytes)?;
    // A JPEG of another size than the dictionary says is padded or cut by
    // each reader its own way.
    if (jpeg.width as u32, jpeg.height as u32) != grid {
        return None;
    }
    let model = stored(&jpeg, &parms)?;
    let named = dict
        .get("ColorSpace")
        .map(|space| ColorSpace::parse(file, space, resources))
        .transpose()
        .ok()?;
    let (space, decode) = space_and_decode(file, dict, named, jpeg.components.len(), 8).ok()?;
    let target = sample_value(&space, &decode, 8, paint);
    let value: Vec<f64> = target.iter().map(|&v| f64::from(v)).collect();
    let samples: Vec<u8> = match (model, value.as_slice()) {
        (Stored::Direct, _) => target.iter().map(|&v| v as u8).collect(),
        (Stored::YCbCr, &[r, g, b]) => ycbcr(r, g, b).to_vec(),
        (Stored::Ycck, &[c, m, y, k]) => {
            let [y, cb, cr] = ycbcr(255.0 - c, 255.0 - m, 255.0 - y);
            vec![y, cb, cr, k as u8]
        }
        _ => return None,
    };
    if samples.len() != jpeg.components.len() {
        return None;
    }
    for rect in rects {
        let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
        jpeg.flatten((x as usize, y as usize, w as usize, h as usize), &samples);
    }
    let mut new_dict = dict.clone();
    for key in ["Length", "Filter", "DecodeParms", "Alternates", "OPI"] {
        new_dict.remove(key);
    }
    new_dict.insert("Filter".into(), PdfValue::Name("DCTDecode".into()));
    if !parms.is_empty() {
        new_dict.insert("DecodeParms".into(), PdfValue::Dictionary(parms.clone()));
    }
    let image = Rewritten {
        dict: new_dict,
        data: jpeg.write()?,
        alpha: None,
    };
    Some((image, parms, target))
}

fn pixel_template(components: u8, fill_color: Color) -> [u8; 4] {
    let r = fill_color.r;
    let g = fill_color.g;
    let b = fill_color.b;
    match components {
        1 => {
            // ITU-R BT.601 luminance.
            let y =
                (0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b)).round() as u8;
            [y, 0, 0, 0]
        }
        3 => [r, g, b, 0],
        4 => {
            // Naive RGB → CMYK (no ICC). For default black fill (0,0,0)
            // this gives C=M=Y=0, K=255, which renders as ink black.
            let rf = f64::from(r) / 255.0;
            let gf = f64::from(g) / 255.0;
            let bf = f64::from(b) / 255.0;
            let k = 1.0 - rf.max(gf).max(bf);
            let denom = (1.0 - k).max(1e-9);
            let c = (1.0 - rf - k) / denom;
            let m = (1.0 - gf - k) / denom;
            let yy = (1.0 - bf - k) / denom;
            [
                (c.clamp(0.0, 1.0) * 255.0).round() as u8,
                (m.clamp(0.0, 1.0) * 255.0).round() as u8,
                (yy.clamp(0.0, 1.0) * 255.0).round() as u8,
                (k.clamp(0.0, 1.0) * 255.0).round() as u8,
            ]
        }
        _ => [0, 0, 0, 0],
    }
}
