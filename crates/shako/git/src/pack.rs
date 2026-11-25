#![forbid(unsafe_code)]
//! Pack writing one object at a time (T-7522): gix's own entry iterator encodes a chunk of
//! objects per step, so its memory follows the objects in the chunk, whatever their size.

use crate::git_err;
use crate::pktline::write_sideband_chunks;
use gix_hash::ObjectId;
use gix_pack::Find as _;
use gix_pack::data::output;
use gix_packetline::Channel;
use shako_core::{ShakoError, ShakoResult};
use std::cell::Cell;
use std::io::Write;

/// Asked with the bytes a job holds at its largest, before the job allocates them; an error
/// ends the job.
pub type Admit<'a> = &'a mut dyn FnMut(u64) -> ShakoResult<()>;

/// What an id costs while a pack is counted and written: the id, its location and offsets.
pub const BYTES_PER_OBJECT: u64 = 128;

/// What a job over `count` objects holds when the largest inflates to `largest` bytes: that
/// object, its compressed form, and the ids.
#[must_use]
pub const fn job_bytes(largest: u64, count: u64) -> u64 {
    largest
        .saturating_mul(2)
        .saturating_add(count.saturating_mul(BYTES_PER_OBJECT))
}

/// Writes a self-contained pack of `ids` to `out`, copying stored entries where their base is
/// in the pack too. An object that cannot be read fails the pack.
pub fn write_pack(
    repo: &gix::Repository,
    ids: &[ObjectId],
    out: impl Write,
    admit: Admit<'_>,
) -> ShakoResult<()> {
    let failed = |e: &dyn std::fmt::Display| ShakoError::Git(format!("pack writing failed: {e}"));
    let num_entries = u32::try_from(ids.len())
        .map_err(|_| ShakoError::BadRequest("pack object count exceeds u32".into()))?;
    let mut largest = 0;
    for id in ids {
        largest = largest.max(
            repo.find_header(*id)
                .map_err(git_err("pack writing failed"))?
                .size(),
        );
    }
    admit(job_bytes(largest, ids.len() as u64))?;

    let mut odb = repo.objects.clone().into_inner();
    // Critical: prevent pack unload while pack locations are held in `Count` (Contract S3, T-6639)
    odb.prevent_pack_unload();
    let mut buf = Vec::new();
    let mut counts = Vec::with_capacity(ids.len());
    for id in ids {
        let at = odb
            .location_by_oid(id, &mut buf)
            .map_err(git_err("pack writing failed"))?;
        counts.push(output::Count::from_data(*id, at));
    }
    // Loose objects first, then each pack's in its own order, so a base precedes its deltas.
    counts.sort_by_key(|c| {
        c.entry_pack_location
            .as_ref()
            .map(|at| (at.pack_id, at.pack_offset))
    });

    let missing = Cell::new(false);
    let entries = counts.iter().map(|count| {
        if let Some(at) = count.entry_pack_location.as_ref()
            && let Some(stored) = odb.entry_by_location(at)
        {
            let pack_of = |c: &output::Count| c.entry_pack_location.as_ref().map(|l| l.pack_id);
            let from = counts.partition_point(|c| pack_of(c) < Some(at.pack_id));
            let to = counts.partition_point(|c| pack_of(c) <= Some(at.pack_id));
            if let Some(entry) = output::Entry::from_pack_entry(
                stored,
                count,
                &counts[from..to],
                from,
                None::<fn(u32, u64) -> gix::Result<Option<ObjectId>>>,
                gix_pack::data::Version::V2,
            ) {
                return Ok(vec![entry?]);
            }
        }
        let Some((object, _)) = odb.try_find(&count.id, &mut buf)? else {
            missing.set(true);
            return Ok(vec![output::Entry::invalid()]);
        };
        let level = gix::zlib::Compression::default();
        Ok(vec![output::Entry::from_data(count, &object, level)?])
    });
    let written = output::bytes::FromEntriesIter::new(
        entries,
        out,
        num_entries,
        gix_pack::data::Version::V2,
        gix_hash::Kind::Sha1,
    );
    for step in written {
        step.map_err(git_err("pack writing failed"))?;
        if missing.get() {
            return Err(failed(&"an object left the repository"));
        }
    }
    Ok(())
}

/// Frames what is written into side-band data packets of at most `max` bytes.
pub(crate) struct Sideband<W: Write> {
    out: W,
    max: usize,
    held: Vec<u8>,
}

impl<W: Write> Sideband<W> {
    pub(crate) fn new(out: W, max: usize) -> Self {
        Self {
            out,
            max,
            held: Vec::with_capacity(max),
        }
    }

    fn send(&mut self) -> std::io::Result<()> {
        write_sideband_chunks(&mut self.out, Channel::Data, &self.held, self.max)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.held.clear();
        Ok(())
    }
}

impl<W: Write> Write for Sideband<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let take = data.len().min(self.max - self.held.len());
        self.held.extend_from_slice(&data[..take]);
        if self.held.len() == self.max {
            self.send()?;
        }
        Ok(take)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.held.is_empty() {
            self.send()?;
        }
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Refusing, pack_count, put, repo};

    #[test]
    fn the_job_size_counts_two_of_the_largest_object_and_saturates() {
        assert_eq!(job_bytes(0, 0), 0);
        assert_eq!(job_bytes(10, 2), 20 + 2 * BYTES_PER_OBJECT);
        assert_eq!(job_bytes(u64::MAX, 1), u64::MAX);
        assert_eq!(job_bytes(1, u64::MAX), u64::MAX);
    }

    #[test]
    fn a_pack_holds_every_asked_object_once_admitted() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = repo(tmp.path(), 1, true);
        let tip = put(&path, "main", "a.txt", b"hello\n");
        let handle = crate::repo::open_isolated(&path).expect("open");
        let tree = handle
            .find_commit(tip)
            .expect("commit")
            .tree_id()
            .expect("tree")
            .detach();
        let mut asked = 0;
        let mut out = Vec::new();
        write_pack(&handle, &[tip, tree], &mut out, &mut |bytes| {
            asked = bytes;
            Ok(())
        })
        .expect("pack");
        assert_eq!(pack_count(&out), Some(2));
        assert!(asked >= 2 * BYTES_PER_OBJECT, "{asked}");

        // A loose object (a tag written outside any pack) is deflated afresh.
        let tag = crate::testutil::annotated_tag(&path, "v1", tip, gix_object::Kind::Commit);
        let handle = crate::repo::open_isolated(&path).expect("reopen");
        let mut loose = Vec::new();
        write_pack(
            &handle,
            &[tag, tip],
            &mut loose,
            &mut crate::testutil::admit_all,
        )
        .expect("pack");
        assert_eq!(pack_count(&loose), Some(2));

        // A refusal stops it before a byte is written.
        let mut none = Vec::new();
        let err = write_pack(&handle, &[tip], &mut none, &mut |_| {
            Err(ShakoError::Overloaded("busy".into()))
        })
        .expect_err("refused");
        assert!(matches!(err, ShakoError::Overloaded(_)), "{err}");
        assert!(none.is_empty());

        // An id the repository does not hold, and a writer that fails, are Git errors.
        let missing =
            gix_hash::ObjectId::from_hex(b"1111111111111111111111111111111111111111").expect("id");
        let err = write_pack(
            &handle,
            &[missing],
            Vec::new(),
            &mut crate::testutil::admit_all,
        )
        .expect_err("missing object");
        assert!(matches!(err, ShakoError::Git(_)), "{err}");
        let err = write_pack(&handle, &[tip], Refusing, &mut crate::testutil::admit_all)
            .expect_err("writer");
        assert!(
            matches!(&err, ShakoError::Git(m) if m.contains("pack writing failed")),
            "{err}"
        );
    }

    #[test]
    fn sideband_frames_hold_at_most_the_frame_size_and_flush_the_rest() {
        let mut out = Vec::new();
        {
            let mut band = Sideband::new(&mut out, 4);
            assert_eq!(band.write(b"abcdef").expect("write"), 4);
            assert_eq!(band.write(b"ef").expect("write"), 2);
            band.flush().expect("flush");
            // Nothing held: a second flush writes nothing more.
            band.flush().expect("flush again");
        }
        let lines = crate::pktline::parse_all_pkt_lines(&out).expect("parse");
        assert_eq!(
            lines,
            [
                crate::pktline::PktLine::Data(b"\x01abcd".to_vec()),
                crate::pktline::PktLine::Data(b"\x01ef".to_vec()),
            ]
        );
        let mut failing = Sideband::new(Refusing, 2);
        assert!(
            failing.write(b"ab").is_err(),
            "a full frame goes out at once"
        );
        let mut held = Sideband::new(Refusing, 8);
        assert_eq!(held.write(b"ab").expect("held"), 2);
        assert!(held.flush().is_err());
    }
}
