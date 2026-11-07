// SPDX-License-Identifier: EUPL-1.2
//! A ZIP bundle of files (T-4477, EP-41, EP-43): one per collection and a README with the
//! provenance, so a colleague can open the download without the platform. The members are
//! counted against the ceiling as they go in, before compression and with their headers, so
//! the archive can only come out smaller than what was allowed, never larger.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::Error;

/// The media type of a bundle.
pub const MEDIA_TYPE: &str = "application/zip";

/// A bundle being written, one member at a time.
pub struct Bundle {
    archive: ZipWriter<Cursor<Vec<u8>>>,
    written: usize,
    max: usize,
}

impl Bundle {
    /// An empty bundle whose members may hold at most `max_bytes` together.
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self {
            archive: ZipWriter::new(Cursor::new(Vec::new())),
            written: 0,
            max: max_bytes,
        }
    }

    /// One member at `path`, deflated.
    ///
    /// # Errors
    /// [`Error::TooLarge`] when the members pass the ceiling, [`Error::Write`] when the archive
    /// refuses the member (a path it already holds).
    pub fn add(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        self.written = self
            .written
            .saturating_add(bytes.len())
            .saturating_add(crate::PART_OVERHEAD);
        if self.written > self.max {
            return Err(Error::TooLarge);
        }
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        self.archive.start_file(path, options)?;
        self.archive.write_all(bytes)?;
        Ok(())
    }

    /// The archive, its central directory written.
    ///
    /// # Errors
    /// [`Error::Write`] when the archive does not close.
    pub fn finish(self) -> Result<Vec<u8>, Error> {
        Ok(self.archive.finish()?.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn every_member_reads_back() -> Result<(), Box<dyn std::error::Error>> {
        let mut bundle = Bundle::new(1 << 20);
        bundle.add("Car.csv", b"id\r\nurn:a\r\n")?;
        bundle.add("README.md", "Exported from view city.\n".as_bytes())?;
        let bytes = bundle.finish()?;
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        assert_eq!(archive.len(), 2);
        let mut csv = String::new();
        archive.by_name("Car.csv")?.read_to_string(&mut csv)?;
        assert_eq!(csv, "id\r\nurn:a\r\n");
        Ok(())
    }

    #[test]
    fn past_the_ceiling_or_twice_one_path_no_member_goes_in() -> Result<(), Error> {
        let room = 2 * crate::PART_OVERHEAD + 10;
        let mut bundle = Bundle::new(room);
        bundle.add("a.csv", b"12345")?;
        assert_eq!(bundle.add("b.csv", b"123456"), Err(Error::TooLarge));
        let mut fits = Bundle::new(room);
        fits.add("a.csv", b"12345")?;
        fits.add("b.csv", b"12345")?;
        assert!(
            fits.finish()?.len() <= room,
            "the archive stays under what was allowed"
        );
        let mut twice = Bundle::new(1 << 20);
        twice.add("a.csv", b"1")?;
        assert!(matches!(twice.add("a.csv", b"2"), Err(Error::Write(_))));
        Ok(())
    }
}
