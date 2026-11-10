// SPDX-License-Identifier: EUPL-1.2
//! A ZIP bundle of files (T-4477, EP-41, EP-43): one per collection and a README with the
//! provenance, so a colleague can open the download without the platform. The members are
//! counted against the ceiling as they go in, before compression and with their headers, so
//! the archive can only come out smaller than what was allowed, never larger. The archive is
//! written as a stream (each member's sizes in a data descriptor after it), so a surface sends
//! it as it grows ([`Bundle::drain`]) and a member may be written a page at a time.

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use zip::write::{SimpleFileOptions, StreamWriter};
use zip::{CompressionMethod, ZipWriter};

use crate::Error;

/// The media type of a bundle.
pub const MEDIA_TYPE: &str = "application/zip";

/// A bundle being written, one member at a time.
pub struct Bundle {
    archive: ZipWriter<StreamWriter<Shared>>,
    out: Shared,
    written: usize,
    max: usize,
}

/// The archive's bytes not drained yet; the writer holds one handle, the bundle the other.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Shared {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut out = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        out.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Bundle {
    /// An empty bundle whose members may hold at most `max_bytes` together.
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        let out = Shared::default();
        Self {
            archive: ZipWriter::new_stream(out.clone()),
            out,
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
        self.start(path)?;
        self.write(bytes)
    }

    /// A member at `path`, deflated, whose bytes [`Bundle::write`] adds; the member before it
    /// ends here.
    ///
    /// # Errors
    /// As [`Bundle::add`].
    pub fn start(&mut self, path: &str) -> Result<(), Error> {
        self.count(crate::PART_OVERHEAD)?;
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        self.archive.start_file(path, options)?;
        Ok(())
    }

    /// More bytes of the member [`Bundle::start`] began.
    ///
    /// # Errors
    /// [`Error::TooLarge`] when the members pass the ceiling.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.count(bytes.len())?;
        self.archive.write_all(bytes)?;
        Ok(())
    }

    fn count(&mut self, n: usize) -> Result<(), Error> {
        self.written = self.written.saturating_add(n);
        if self.written > self.max {
            return Err(Error::TooLarge);
        }
        Ok(())
    }

    /// The archive's bytes written since the last drain.
    #[must_use]
    pub fn drain(&mut self) -> Vec<u8> {
        self.out.take()
    }

    /// The rest of the archive, its central directory written.
    ///
    /// # Errors
    /// [`Error::Write`] when the archive does not close.
    pub fn finish(self) -> Result<Vec<u8>, Error> {
        self.archive.finish()?;
        Ok(self.out.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

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

    #[test]
    fn a_member_written_in_parts_and_drained_reads_back() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut bundle = Bundle::new(1 << 20);
        let mut bytes = Vec::new();
        bundle.start("Car.csv")?;
        bundle.write(b"id\r\n")?;
        bytes.extend(bundle.drain());
        assert!(bytes.starts_with(b"PK\x03\x04"), "the local header leaves first");
        for i in 0..200 {
            bundle.write(format!("urn:ngsi-ld:Car:{i}\r\n").as_bytes())?;
            bytes.extend(bundle.drain());
        }
        bundle.add("README.md", b"two\n")?;
        bytes.extend(bundle.drain());
        bytes.extend(bundle.finish()?);
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        let mut csv = String::new();
        archive.by_name("Car.csv")?.read_to_string(&mut csv)?;
        assert_eq!(csv.lines().count(), 201);
        assert!(csv.ends_with("urn:ngsi-ld:Car:199\r\n"));
        let mut readme = String::new();
        archive.by_name("README.md")?.read_to_string(&mut readme)?;
        assert_eq!(readme, "two\n");
        let mut tight = Bundle::new(crate::PART_OVERHEAD + 4);
        tight.start("a.csv")?;
        tight.write(b"1234")?;
        assert_eq!(tight.write(b"5"), Err(Error::TooLarge), "a part counts as added");
        Ok(())
    }
}
