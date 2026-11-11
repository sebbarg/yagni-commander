//! Image mode: decoding one picture for the F3 viewer. The format comes from
//! the file's first bytes and must be one of four; the size is checked from
//! the header, before any pixels are allocated. Runs on the viewer's own
//! thread, since a large picture takes seconds.

use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use image::{DynamicImage, Frame, ImageDecoder, ImageFormat, ImageReader, Limits};

use super::source::Source;

/// Larger pictures are refused: decoding needs 4 bytes per pixel in memory.
pub const PICTURE_PIXELS: u64 = 200_000_000;
/// A decoded picture's longest side is cut to this. A GPU's largest texture
/// is at least this big, and gpui draws nothing for an image that doesn't
/// fit one.
pub const PICTURE_SIDE: u32 = 4096;
/// What the decoder may allocate: a 16-bit picture takes twice the 4 bytes.
const MAX_ALLOC: u64 = 1 << 30;

const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
const NOT_A_PICTURE: &str = "Not a PNG, JPEG, GIF or WebP image.";

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
}

/// Decodes the picture in `source`, its longest side cut to `max_side`. An
/// animation shows its first frame. The error is a sentence for a box.
pub fn read_picture<S: Source>(source: &S, max_side: u32) -> Result<Picture, String> {
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
    })
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
        read_picture(&bytes, PICTURE_SIDE).err().expect("an error")
    }

    #[test]
    fn image_names_open_in_image_mode() {
        for name in ["a.png", "a.PNG", "b.jpg", "b.jpeg", "c.Gif", "d.webp"] {
            assert!(is_picture_name(Path::new(name)), "{name}");
        }
        for name in ["png", ".png", "a.bmp", "a.svg", "a.png.txt", "a"] {
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
            let picture = read_picture(&encoded(format, 3, 2), PICTURE_SIDE).unwrap();
            assert_eq!((picture.width, picture.height), (3, 2), "{name}");
            assert_eq!(picture.format, name);
            assert_eq!(picture.frame.buffer().dimensions(), (3, 2));
        }
    }

    #[test]
    fn the_format_comes_from_the_bytes() {
        let picture = read_picture(&encoded(ImageFormat::Png, 1, 1), 16).unwrap();
        assert_eq!(picture.format, "PNG");
        assert_eq!(error(b"just text, named a.png".to_vec()), NOT_A_PICTURE);
        assert_eq!(error(Vec::new()), NOT_A_PICTURE);
        let mut bmp = b"BM".to_vec();
        bmp.resize(64, 0);
        assert_eq!(error(bmp), NOT_A_PICTURE, "a format we don't decode");
    }

    #[test]
    fn pixels_are_bgra() {
        let picture = read_picture(&encoded(ImageFormat::Png, 1, 1), 16).unwrap();
        assert_eq!(
            picture.frame.buffer().as_raw().as_slice(),
            [30, 20, 10, 255]
        );
    }

    #[test]
    fn a_large_picture_is_cut_and_keeps_its_own_size() {
        let picture = read_picture(&encoded(ImageFormat::Png, 400, 100), 200).unwrap();
        assert_eq!((picture.width, picture.height), (400, 100));
        assert_eq!(picture.frame.buffer().dimensions(), (200, 50));
    }

    #[test]
    fn a_picture_at_the_limit_is_not_cut() {
        let picture = read_picture(&encoded(ImageFormat::Png, 200, 100), 200).unwrap();
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
        let picture = read_picture(&rotated, PICTURE_SIDE).unwrap();
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
        let message = read_picture(&Broken, 16).err().unwrap();
        assert_eq!(message, "Cannot read the image: the disk is gone");
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
