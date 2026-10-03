//! Archives built for tests: zips and tars with any names, including
//! malicious ones.

use std::fs::File;
use std::path::Path;

use crate::file_ops::Format;
/// A zip at `path`: names ending in "/" are folders, the rest files with
/// the given contents.
pub(crate) fn make_zip(path: &Path, entries: &[(&str, &str)]) {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for (name, contents) in entries {
        if name.ends_with('/') {
            zip.add_directory(*name, options).unwrap();
        } else {
            zip.start_file(*name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
    }
    zip.finish().unwrap();
}

/// One entry for [`make_tar`].
pub(crate) enum T<'a> {
    File(&'a str, &'a str, u32),
    Dir(&'a str),
    Link(&'a str, &'a str),
    Hard(&'a str, &'a [u8]),
    Fifo(&'a str),
    /// A file with a raw name (bypasses the tar crate's `..` check).
    Raw(&'a [u8]),
}

/// Writes raw name and link-name bytes into a header.
fn raw_names(header: &mut tar::Header, name: &[u8], link: &[u8]) {
    let old = header.as_old_mut();
    old.name = [0; 100];
    old.name[..name.len()].copy_from_slice(name);
    old.linkname = [0; 100];
    old.linkname[..link.len()].copy_from_slice(link);
}

/// A tar of `entries` at `path`, compressed as `format`.
pub(crate) fn make_tar(path: &Path, format: Format, entries: &[T]) {
    use std::io::Write;
    let file = File::create(path).unwrap();
    let writer: Box<dyn Write> = match format {
        Format::Tar => Box::new(file),
        Format::TarGz => Box::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::default(),
        )),
        Format::TarBz2 => Box::new(bzip2::write::BzEncoder::new(
            file,
            bzip2::Compression::default(),
        )),
        Format::TarXz => Box::new(liblzma::write::XzEncoder::new(file, 6)),
        Format::TarZst => Box::new(
            zstd::stream::write::Encoder::new(file, 0)
                .unwrap()
                .auto_finish(),
        ),
        Format::Zip => unreachable!(),
    };
    let mut builder = tar::Builder::new(writer);
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mtime(1_000_000_000);
        header.set_username("me").unwrap();
        header.set_groupname("staff").unwrap();
        header.set_mode(0o644);
        header.set_size(0);
        let data: &[u8] = match entry {
            T::File(name, contents, mode) => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(*mode);
                header.set_size(contents.len() as u64);
                raw_names(&mut header, name.as_bytes(), b"");
                contents.as_bytes()
            }
            T::Dir(name) => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                raw_names(&mut header, name.as_bytes(), b"");
                b""
            }
            T::Link(name, target) => {
                header.set_entry_type(tar::EntryType::Symlink);
                raw_names(&mut header, name.as_bytes(), target.as_bytes());
                b""
            }
            T::Hard(name, target) => {
                header.set_entry_type(tar::EntryType::Link);
                raw_names(&mut header, name.as_bytes(), target);
                b""
            }
            T::Fifo(name) => {
                header.set_entry_type(tar::EntryType::Fifo);
                raw_names(&mut header, name.as_bytes(), b"");
                b""
            }
            T::Raw(name) => {
                header.set_entry_type(tar::EntryType::Regular);
                raw_names(&mut header, name, b"");
                b""
            }
        };
        header.set_cksum();
        builder.append(&header, data).unwrap();
    }
    builder.into_inner().unwrap().flush().unwrap();
}
