#![forbid(unsafe_code)]
//! Pack writing one object at a time (T-7522): gix's own entry iterator encodes a chunk of
//! objects per step, so its memory follows the objects in the chunk, whatever their size.

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
        largest = largest.max(repo.find_header(*id).map_err(|e| failed(&e))?.size());
    }
    admit(job_bytes(largest, ids.len() as u64))?;

    let mut odb = repo.objects.clone().into_inner();
    // Critical: prevent pack unload while pack locations are held in `Count` (Contract S3, T-6639)
    odb.prevent_pack_unload();
    let mut buf = Vec::new();
    let mut counts = Vec::with_capacity(ids.len());
    for id in ids {
        let at = odb.location_by_oid(id, &mut buf).map_err(|e| failed(&e))?;
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
        step.map_err(|e| failed(&e))?;
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
