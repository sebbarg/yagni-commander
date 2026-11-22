//! Image mode: decoding one picture for the F3 viewer. The format comes from
//! the file's first bytes and must be one of five; a bitmap's size is checked
//! from the header, before any pixels are allocated, and an SVG is drawn at
//! the size asked for. Runs on the viewer's own thread, since a large picture
//! takes seconds.

use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use image::{DynamicImage, Frame, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage};
use resvg::{tiny_skia, usvg};

use super::source::Source;

/// Larger pictures are refused: decoding needs 4 bytes per pixel in memory.
pub const PICTURE_PIXELS: u64 = 200_000_000;
/// A decoded picture's longest side is cut to this. A GPU's largest texture
/// is at least this big, and gpui draws nothing for an image that doesn't
/// fit one.
pub const PICTURE_SIDE: u32 = 4096;
/// What the decoder may allocate: a 16-bit picture takes twice the 4 bytes.
const MAX_ALLOC: u64 = 1 << 30;

/// Larger SVG files are refused: the whole file is parsed in memory.
pub const SVG_CAP: u64 = 20 << 20;
/// How far into the file an SVG's `<svg` tag is looked for.
const SVG_HEAD: usize = 4096;

const EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "svg"];
const NOT_A_PICTURE: &str = "Not a PNG, JPEG, GIF, WebP or SVG image.";

/// A name F3 opens in image mode, any case.
pub fn is_picture_name(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| EXTENSIONS.iter().any(|known| e.eq_ignore_ascii_case(known)))
}

/// A decoded picture.
pub struct Picture {
    /// The pixels in BGRA order, as gpui draws them. Smaller than `width` by
    /// `height` when the picture was cut to the side asked for.
    pub frame: Frame,
    /// The picture's own size, upright.
    pub width: u32,
    pub height: u32,
    pub format: &'static str,
    /// Drawn from shapes (SVG): it can be drawn again at any size.
    pub vector: bool,
}

/// Decodes the picture in `source`, its longest side cut to `max_side`. An
/// animation shows its first frame. An SVG is drawn at `vector_scale` pixels
/// per unit of its own size (the display's scale factor times the zoom), cut
/// to `max_side` too. The error is a sentence for a box.
pub fn read_picture<S: Source>(
    source: &S,
    max_side: u32,
    vector_scale: f32,
) -> Result<Picture, String> {
    let mut head = vec![0; SVG_HEAD];
    let n = read_full(source, &mut head, 0).map_err(|e| format!("Cannot read the image: {e}"))?;
    if is_svg(&head[..n]) {
        return read_svg(source, max_side, vector_scale);
    }
    let failed = |e: image::ImageError| format!("Cannot read the image: {e}");
    let mut reader = ImageReader::new(BufReader::new(Reader { source, pos: 0 }))
        .with_guessed_format()
        .map_err(|e| format!("Cannot read the image: {e}"))?;
    let format = reader.format().and_then(format_name).ok_or(NOT_A_PICTURE)?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(failed)?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > PICTURE_PIXELS {
        return Err(format!(
            "The image is too large to show ({w}x{h}, at most {} megapixels).",
            PICTURE_PIXELS / 1_000_000
        ));
    }
    let orientation = decoder.orientation().map_err(failed)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(failed)?;
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    if width.max(height) > max_side {
        image = image.thumbnail(max_side, max_side);
    }
    let mut pixels = image.into_rgba8();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(Picture {
        frame: Frame::new(pixels),
        width,
        height,
        format,
        vector: false,
    })
}

/// Whether the file's start is an SVG document: markup (after a byte order
/// mark and white space) with an `<svg` tag in the first `SVG_HEAD` bytes.
fn is_svg(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let start = head.iter().position(|b| !b.is_ascii_whitespace());
    start.is_some_and(|at| head[at] == b'<')
        && head.windows(4).any(|w| w.eq_ignore_ascii_case(b"<svg"))
}

/// Reads until `buf` is full or the file ends.
fn read_full<S: Source>(source: &S, buf: &mut [u8], from: u64) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match source.read_at(&mut buf[n..], from + n as u64)? {
            0 => break,
            read => n += read,
        }
    }
    Ok(n)
}

fn read_svg<S: Source>(source: &S, max_side: u32, scale: f32) -> Result<Picture, String> {
    let len = source.len();
    if len > SVG_CAP {
        return Err(format!(
            "The SVG file is too large to show (at most {} MiB).",
            SVG_CAP >> 20
        ));
    }
    let mut data = vec![0; len as usize];
    let n = read_full(source, &mut data, 0).map_err(|e| format!("Cannot read the image: {e}"))?;
    data.truncate(n);
    let tree = usvg::Tree::from_data(&data, &svg_options())
        .map_err(|e| format!("Cannot read the image: {e}."))?;
    let size = tree.size();
    let (own_w, own_h) = (size.width(), size.height());
    let (width, height) = (own_w.ceil() as u32, own_h.ceil() as u32);
    // Pixels per unit, cut so the longest side fits `max_side`.
    let scale = scale.min(max_side as f32 / own_w.max(own_h));
    let side = |own: f32| ((own * scale).round() as u32).clamp(1, max_side.max(1));
    let (w, h) = (side(own_w), side(own_h));
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("The image could not be drawn.")?;
    let transform = tiny_skia::Transform::from_scale(w as f32 / own_w, h as f32 / own_h);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // tiny-skia's pixels are premultiplied RGBA, gpui's straight BGRA.
    let mut bgra = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.pixels() {
        let c = pixel.demultiply();
        bgra.extend([c.blue(), c.green(), c.red(), c.alpha()]);
    }
    let pixels = RgbaImage::from_raw(w, h, bgra).ok_or("The image could not be drawn.")?;
    Ok(Picture {
        frame: Frame::new(pixels),
        width: width.max(1),
        height: height.max(1),
        format: "SVG",
        vector: true,
    })
}

/// Parsing options: the system's fonts for text, and no files but the SVG
/// itself: an `<image>` shows only when its data is inside (a `data:` URL),
/// never a file it names, so a picture can't pull in other files' contents.
fn svg_options() -> usvg::Options<'static> {
    usvg::Options {
        fontdb: system_fonts(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}

/// The system's fonts, loaded once, by the first SVG (tens of milliseconds).
fn system_fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            installed_generics(&mut db);
            Arc::new(db)
        })
        .clone()
}

/// Installed fonts to try for a generic family, in order, when the one
/// fontdb chose isn't installed.
const SANS: &[&str] = &[
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "Cantarell",
    "Ubuntu",
    "Helvetica",
    "Arial",
];
const SERIF: &[&str] = &[
    "Noto Serif",
    "DejaVu Serif",
    "Liberation Serif",
    "Times",
    "Times New Roman",
];
const MONO: &[&str] = &[
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Ubuntu Mono",
    "Menlo",
    "Courier New",
];

/// Points each generic family (`sans-serif`, ...) at a font that is
/// installed. fontdb takes them from fontconfig's alias files without
/// looking: the last alias's first preferred name, which on Kubuntu is a
/// font it doesn't have, so `font-family="sans-serif"` (and usvg's fallback,
/// serif) matched nothing and the text was left out. Kept when installed;
/// else the first of our list that is, else any installed family.
fn installed_generics(db: &mut usvg::fontdb::Database) {
    use usvg::fontdb::Family;
    let installed = |db: &usvg::fontdb::Database, name: &str| {
        db.faces()
            .any(|face| face.families.iter().any(|(n, _)| n == name))
    };
    let pick = |db: &usvg::fontdb::Database, family: Family, list: &[&str]| {
        let current = db.family_name(&family);
        if installed(db, current) {
            return None;
        }
        list.iter()
            .find(|name| installed(db, name))
            .map(|name| (*name).to_owned())
            .or_else(|| {
                let face = db.faces().next()?;
                Some(face.families.first()?.0.clone())
            })
    };
    if let Some(name) = pick(db, Family::SansSerif, SANS) {
        db.set_sans_serif_family(name);
    }
    if let Some(name) = pick(db, Family::Serif, SERIF) {
        db.set_serif_family(name);
    }
    if let Some(name) = pick(db, Family::Monospace, MONO) {
        db.set_monospace_family(name);
    }
    let sans = db.family_name(&Family::SansSerif).to_owned();
    for (family, set) in [
        (
            Family::Cursive,
            usvg::fontdb::Database::set_cursive_family::<String> as fn(&mut _, String),
        ),
        (
            Family::Fantasy,
            usvg::fontdb::Database::set_fantasy_family::<String>,
        ),
    ] {
        if !installed(db, db.family_name(&family)) {
            set(db, sans.clone());
        }
    }
}

fn format_name(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("PNG"),
        ImageFormat::Jpeg => Some("JPEG"),
        ImageFormat::Gif => Some("GIF"),
        ImageFormat::WebP => Some("WebP"),
        _ => None,
    }
}

/// A `Source` as the stream the decoders read.
struct Reader<'a, S> {
    source: &'a S,
    pos: u64,
}

impl<S: Source> Read for Reader<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.source.read_at(buf, self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<S: Source> Seek for Reader<'_, S> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(n) => self.pos.checked_add_signed(n),
            SeekFrom::End(n) => self.source.len().checked_add_signed(n),
        };
        self.pos = pos.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use std::io::Cursor;

    fn encoded(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        // JPEG has no alpha.
        if format == ImageFormat::Jpeg {
            DynamicImage::ImageRgba8(image)
                .into_rgb8()
                .write_to(&mut out, format)
                .unwrap();
        } else {
            image.write_to(&mut out, format).unwrap();
        }
        out.into_inner()
    }

    fn error(bytes: Vec<u8>) -> String {
        read_picture(&bytes, PICTURE_SIDE, 1.0)
            .err()
            .expect("an error")
    }

    #[test]
    fn image_names_open_in_image_mode() {
        for name in [
            "a.png", "a.PNG", "b.jpg", "b.jpeg", "c.Gif", "d.webp", "e.SVG",
        ] {
            assert!(is_picture_name(Path::new(name)), "{name}");
        }
        for name in ["png", ".png", "a.bmp", "a.svgz", "a.png.txt", "a"] {
            assert!(!is_picture_name(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn the_four_formats_decode() {
        for (format, name) in [
            (ImageFormat::Png, "PNG"),
            (ImageFormat::Jpeg, "JPEG"),
            (ImageFormat::Gif, "GIF"),
            (ImageFormat::WebP, "WebP"),
        ] {
            let picture = read_picture(&encoded(format, 3, 2), PICTURE_SIDE, 1.0).unwrap();
            assert_eq!((picture.width, picture.height), (3, 2), "{name}");
            assert_eq!(picture.format, name);
            assert_eq!(picture.frame.buffer().dimensions(), (3, 2));
        }
    }

    #[test]
    fn the_format_comes_from_the_bytes() {
        let picture = read_picture(&encoded(ImageFormat::Png, 1, 1), 16, 1.0).unwrap();
        assert_eq!(picture.format, "PNG");
        assert_eq!(error(b"just text, named a.png".to_vec()), NOT_A_PICTURE);
        assert_eq!(error(Vec::new()), NOT_A_PICTURE);
        let mut bmp = b"BM".to_vec();
        bmp.resize(64, 0);
        assert_eq!(error(bmp), NOT_A_PICTURE, "a format we don't decode");
    }

    #[test]
    fn pixels_are_bgra() {
        let picture = read_picture(&encoded(ImageFormat::Png, 1, 1), 16, 1.0).unwrap();
        assert_eq!(
            picture.frame.buffer().as_raw().as_slice(),
            [30, 20, 10, 255]
        );
    }

    #[test]
    fn a_large_picture_is_cut_and_keeps_its_own_size() {
        let picture = read_picture(&encoded(ImageFormat::Png, 400, 100), 200, 1.0).unwrap();
        assert_eq!((picture.width, picture.height), (400, 100));
        assert_eq!(picture.frame.buffer().dimensions(), (200, 50));
    }

    #[test]
    fn a_picture_at_the_limit_is_not_cut() {
        let picture = read_picture(&encoded(ImageFormat::Png, 200, 100), 200, 1.0).unwrap();
        assert_eq!(picture.frame.buffer().dimensions(), (200, 100));
    }

    fn png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        png.extend((data.len() as u32).to_be_bytes());
        png.extend(kind);
        png.extend(data);
        png.extend(crc.sum().to_be_bytes());
    }

    /// A PNG that claims this size and holds no pixels.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut header = width.to_be_bytes().to_vec();
        header.extend(height.to_be_bytes());
        header.extend([8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png_chunk(&mut png, b"IHDR", &header);
        png_chunk(&mut png, b"IDAT", &[]);
        png_chunk(&mut png, b"IEND", &[]);
        png
    }

    #[test]
    fn too_many_pixels_are_refused_from_the_header() {
        assert_eq!(
            error(png_header(20_000, 20_000)),
            "The image is too large to show (20000x20000, at most 200 megapixels)."
        );
    }

    #[test]
    fn damaged_data_is_an_error() {
        let message = error(png_header(4, 4));
        assert!(message.starts_with("Cannot read the image: "), "{message}");
        let mut garbled = b"\x89PNG\r\n\x1a\n".to_vec();
        garbled.extend([0xFF; 32]);
        let message = error(garbled);
        assert!(message.starts_with("Cannot read the image: "), "{message}");
    }

    #[test]
    fn a_photo_shows_upright() {
        // A 4x2 JPEG with an EXIF block saying "rotate 90 degrees".
        let jpeg = encoded(ImageFormat::Jpeg, 4, 2);
        let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0".to_vec();
        exif.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0]);
        let mut rotated = jpeg[..2].to_vec();
        rotated.extend([0xFF, 0xE1]);
        rotated.extend(((exif.len() + 2) as u16).to_be_bytes());
        rotated.extend(exif);
        rotated.extend(&jpeg[2..]);
        let picture = read_picture(&rotated, PICTURE_SIDE, 1.0).unwrap();
        assert_eq!((picture.width, picture.height), (2, 4));
        assert_eq!(picture.frame.buffer().dimensions(), (2, 4));
    }

    #[test]
    fn a_read_error_is_a_sentence() {
        struct Broken;
        impl Source for Broken {
            fn len(&self) -> u64 {
                100
            }
            fn current_len(&self) -> io::Result<u64> {
                Ok(100)
            }
            fn read_at(&self, _: &mut [u8], _: u64) -> io::Result<usize> {
                Err(io::Error::other("the disk is gone"))
            }
        }
        let message = read_picture(&Broken, 16, 1.0).err().unwrap();
        assert_eq!(message, "Cannot read the image: the disk is gone");
    }

    const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
        <rect width="20" height="10" fill="#0a141e"/></svg>"##;

    fn svg(text: &str, max_side: u32, scale: f32) -> Result<Picture, String> {
        read_picture(&text.as_bytes().to_vec(), max_side, scale)
    }

    #[test]
    fn an_svg_is_drawn_at_its_own_size_times_the_scale() {
        let picture = svg(SQUARE, PICTURE_SIDE, 1.0).unwrap();
        assert_eq!((picture.width, picture.height), (20, 10));
        assert_eq!((picture.format, picture.vector), ("SVG", true));
        assert_eq!(picture.frame.buffer().dimensions(), (20, 10));
        let sharp = svg(SQUARE, PICTURE_SIDE, 3.0).unwrap();
        assert_eq!((sharp.width, sharp.height), (20, 10), "its own size stays");
        assert_eq!(sharp.frame.buffer().dimensions(), (60, 30));
    }

    #[test]
    fn an_svg_is_cut_to_the_longest_side() {
        let picture = svg(SQUARE, 8, 4.0).unwrap();
        assert_eq!(picture.frame.buffer().dimensions(), (8, 4));
    }

    #[test]
    fn svg_pixels_are_straight_bgra() {
        let half = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
            <rect width="1" height="1" fill="#ff0000" fill-opacity="0.5"/></svg>"##;
        let picture = svg(half, 16, 1.0).unwrap();
        let [b, g, r, a] = picture.frame.buffer().as_raw()[..] else {
            panic!("one pixel")
        };
        assert_eq!((b, g), (0, 0));
        assert!(r >= 254, "not premultiplied: {r}");
        assert!((127..=129).contains(&a), "{a}");
        let picture = svg(SQUARE, 16, 1.0).unwrap();
        assert_eq!(&picture.frame.buffer().as_raw()[..4], [30, 20, 10, 255]);
    }

    #[test]
    fn an_svg_is_found_by_its_content() {
        let declared = format!("\u{feff}<?xml version=\"1.0\"?>\n<!-- a comment -->\n{SQUARE}");
        assert_eq!(svg(&declared, 16, 1.0).unwrap().format, "SVG");
        let spaced = format!("\n  {SQUARE}");
        assert_eq!(svg(&spaced, 16, 1.0).unwrap().format, "SVG");
        assert!(is_svg(b"<SVG"));
        assert!(!is_svg(b"just text with <svg in it"));
        assert!(!is_svg(b"<html><body></body></html>"));
        let late = format!("<?xml version=\"1.0\"?>{}{SQUARE}", " ".repeat(SVG_HEAD));
        assert_eq!(
            svg(&late, 16, 1.0).err().unwrap(),
            NOT_A_PICTURE,
            "too far in"
        );
    }

    #[test]
    fn a_broken_svg_is_an_error() {
        let message = svg("<svg xmlns=\"http://www.w3.org/2000/svg\"><rect", 16, 1.0)
            .err()
            .unwrap();
        assert!(message.starts_with("Cannot read the image: "), "{message}");
        let message = svg("<html><svg/></html>", 16, 1.0).err().unwrap();
        assert!(message.starts_with("Cannot read the image: "), "{message}");
    }

    #[test]
    fn a_large_svg_file_is_refused_before_reading() {
        struct Huge;
        impl Source for Huge {
            fn len(&self) -> u64 {
                SVG_CAP + 1
            }
            fn current_len(&self) -> io::Result<u64> {
                Ok(SVG_CAP + 1)
            }
            fn read_at(&self, buf: &mut [u8], at: u64) -> io::Result<usize> {
                assert_eq!(at, 0, "only the head is read");
                let head = b"<svg";
                let n = head.len().min(buf.len());
                buf[..n].copy_from_slice(&head[..n]);
                buf[n..].fill(b' ');
                Ok(buf.len())
            }
        }
        let message = read_picture(&Huge, 16, 1.0).err().unwrap();
        assert_eq!(
            message,
            "The SVG file is too large to show (at most 20 MiB)."
        );
    }

    #[test]
    fn an_svg_shows_no_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("red.png");
        RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255]))
            .save(&png)
            .unwrap();
        let named = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1" height="1">
            <image width="1" height="1" xlink:href="{}"/></svg>"#,
            png.display()
        );
        let picture = svg(&named, 16, 1.0).unwrap();
        assert_eq!(
            picture.frame.buffer().as_raw()[3],
            0,
            "the file is not drawn"
        );
        let mut encoded = Cursor::new(Vec::new());
        RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255]))
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let inline = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1" height="1">
            <image width="1" height="1" xlink:href="data:image/png;base64,{}"/></svg>"#,
            base64(&encoded.into_inner())
        );
        let picture = svg(&inline, 16, 1.0).unwrap();
        assert_eq!(
            picture.frame.buffer().as_raw()[..],
            [0, 0, 255, 255],
            "data is drawn"
        );
    }

    fn base64(bytes: &[u8]) -> String {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
            for i in 0..4 {
                out.push(if i <= chunk.len() {
                    ABC[(n >> (18 - 6 * i) & 63) as usize] as char
                } else {
                    '='
                });
            }
        }
        out
    }

    #[test]
    fn svg_text_is_drawn_in_a_system_font() {
        if system_fonts().is_empty() {
            return; // a machine without fonts draws no text
        }
        let text = r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="30">
            <text x="2" y="24" font-size="24" font-family="sans-serif">Hi</text></svg>"#;
        let picture = svg(text, 64, 1.0).unwrap();
        let drawn = picture
            .frame
            .buffer()
            .as_raw()
            .chunks(4)
            .filter(|p| p[3] > 0)
            .count();
        assert!(drawn > 20, "{drawn} pixels");
    }

    /// A database holding only the bundled JetBrains Mono.
    fn one_font() -> usvg::fontdb::Database {
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../yagni-commander/assets/fonts/JetBrainsMonoNL-Regular.ttf"
            ))
            .to_vec(),
        );
        db
    }

    #[test]
    fn generic_families_name_installed_fonts() {
        use usvg::fontdb::Family;
        let mut db = one_font();
        // As fontdb leaves them after reading an alias it doesn't have.
        db.set_sans_serif_family("Not Installed Sans");
        db.set_serif_family("Not Installed Serif");
        installed_generics(&mut db);
        for family in [
            Family::SansSerif,
            Family::Serif,
            Family::Monospace,
            Family::Cursive,
            Family::Fantasy,
        ] {
            assert_eq!(db.family_name(&family), "JetBrains Mono NL", "{family:?}");
        }
        let text = r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="30">
            <text x="2" y="24" font-size="24" font-family="sans-serif">Hi</text></svg>"#;
        let options = usvg::Options {
            fontdb: Arc::new(db),
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_str(text, &options).unwrap();
        assert!(tree.root().has_children(), "the text is drawn");
    }

    #[test]
    fn an_installed_generic_family_is_kept() {
        use usvg::fontdb::Family;
        let mut db = one_font();
        db.set_monospace_family("JetBrains Mono NL");
        db.set_sans_serif_family("JetBrains Mono NL");
        installed_generics(&mut db);
        assert_eq!(db.family_name(&Family::Monospace), "JetBrains Mono NL");
        let mut empty = usvg::fontdb::Database::new();
        installed_generics(&mut empty);
        assert_eq!(
            empty.family_name(&Family::SansSerif),
            "Arial",
            "nothing to pick"
        );
    }

    #[test]
    fn the_stream_seeks_from_every_end() {
        let bytes = vec![1, 2, 3, 4, 5];
        let mut r = Reader {
            source: &bytes,
            pos: 0,
        };
        assert_eq!(r.seek(SeekFrom::End(-2)).unwrap(), 3);
        assert_eq!(r.seek(SeekFrom::Current(-1)).unwrap(), 2);
        assert_eq!(r.seek(SeekFrom::Start(1)).unwrap(), 1);
        let mut one = [0];
        assert_eq!(r.read(&mut one).unwrap(), 1);
        assert_eq!((one[0], r.pos), (2, 2));
        assert!(r.seek(SeekFrom::Current(-3)).is_err());
    }
}
