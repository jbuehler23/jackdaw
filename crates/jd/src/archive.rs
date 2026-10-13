//! Unpacking editor bundles: `.tar.zst` on Linux, `.zip` on macOS and
//! Windows, and `.tar.gz`.

use std::fs::File;
use std::path::{Path, PathBuf};

/// A bundle's compression, from the name it was published under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    TarZst,
    TarGz,
    Zip,
}

impl Format {
    /// The format a file name or URL ends with.
    pub fn of(name: &str) -> Option<Self> {
        let name = name.split(['?', '#']).next().unwrap_or(name);
        if name.ends_with(".tar.zst") {
            Some(Self::TarZst)
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if name.ends_with(".zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }
}

/// Unpack `archive` into `dest`, which must exist.
///
/// Entries that would land outside `dest` are refused by both readers.
pub fn unpack(archive: &Path, format: Format, dest: &Path) -> Result<(), String> {
    let describe = |error: &dyn std::fmt::Display| format!("{}: {error}", archive.display());
    let file = File::open(archive).map_err(|error| describe(&error))?;
    match format {
        Format::TarZst => {
            let decoder = ruzstd::decoding::StreamingDecoder::new(std::io::BufReader::new(file))
                .map_err(|error| describe(&error))?;
            tar::Archive::new(decoder)
                .unpack(dest)
                .map_err(|error| describe(&error))
        }
        Format::TarGz => tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(dest)
            .map_err(|error| describe(&error)),
        Format::Zip => zip::ZipArchive::new(file)
            .and_then(|mut zip| zip.extract(dest))
            .map_err(|error| describe(&error)),
    }
}

/// The directory a bundle's files are in: bundles hold a single top-level
/// folder, which is used when present.
pub fn bundle_root(unpacked: &Path) -> Result<PathBuf, String> {
    let entries: Vec<_> = std::fs::read_dir(unpacked)
        .map_err(|error| format!("{}: {error}", unpacked.display()))?
        .flatten()
        .collect();
    match entries.as_slice() {
        [only] if only.path().is_dir() => Ok(only.path()),
        _ => Ok(unpacked.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_follow_the_published_extension() {
        assert_eq!(
            Format::of("jackdaw-x86_64-unknown-linux-gnu.tar.zst"),
            Some(Format::TarZst)
        );
        assert_eq!(
            Format::of("https://example.com/jackdaw-aarch64-apple-darwin.zip?download=1"),
            Some(Format::Zip)
        );
        assert_eq!(Format::of("bundle.tgz"), Some(Format::TarGz));
        assert_eq!(Format::of("bundle.rar"), None);
    }

    #[test]
    fn a_gzipped_tarball_unpacks_under_its_single_folder() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bundle.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "jackdaw-test/jackdaw", &b"hello"[..])
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let dest = dir.path().join("out");
        std::fs::create_dir(&dest).unwrap();
        unpack(&archive, Format::TarGz, &dest).unwrap();
        let root = bundle_root(&dest).unwrap();
        assert_eq!(root, dest.join("jackdaw-test"));
        assert_eq!(std::fs::read(root.join("jackdaw")).unwrap(), b"hello");
    }

    #[test]
    fn a_zip_unpacks_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bundle.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer
            .start_file(
                "jackdaw-test/jd",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        std::io::Write::write_all(&mut writer, b"jd bytes").unwrap();
        writer.finish().unwrap();

        let dest = dir.path().join("out");
        std::fs::create_dir(&dest).unwrap();
        unpack(&archive, Format::Zip, &dest).unwrap();
        assert_eq!(
            std::fs::read(dest.join("jackdaw-test/jd")).unwrap(),
            b"jd bytes"
        );
    }
}
