//! The generation journal of one world and dimension.
//!
//! Every time a chunk is generated, or saved with changed blocks, it gets the
//! next sequence number of its dimension. Ironore's client keeps the highest
//! sequence it has ingested, so on rejoin the server sends only the chunks
//! whose sequence is above it.
//!
//! The journal also serves as the index of which chunks exist: a position is
//! in it exactly when a finished chunk was generated or found there.
//!
//! On disk it is an append-only file of fixed-size records beside the region
//! folder. A reservation record raises the ceiling that sequences may reach
//! before it is written, so a restart never hands out a sequence a client may
//! already have seen. An entry record maps a chunk to its sequence and is
//! appended only once the chunk itself has been written.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use rustc_hash::FxHashMap;
use tracing::{error, warn};

use crate::chunk_system::ChunkPos;

const MAGIC: [u8; 8] = *b"IOREJRN1";
const RECORD_LEN: usize = 17;
const TAG_RESERVE: u8 = b'R';
const TAG_ENTRY: u8 = b'E';
/// How many sequences one reservation record covers.
const RESERVATION: i64 = 4096;

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("could not {action} the generation journal {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("the generation journal {} is corrupt at byte {offset}: {reason}", path.display())]
    Corrupt {
        path: PathBuf,
        offset: usize,
        reason: String,
    },
}

/// The sequence numbers of one dimension's chunks, shared by the chunk
/// scheduler, the chunk writer and the LOD sender.
pub struct GenerationJournal {
    path: PathBuf,
    inner: Mutex<Inner>,
    generated: AtomicU64,
    failure: OnceLock<String>,
}

struct Inner {
    file: BufWriter<File>,
    sequences: FxHashMap<ChunkPos, i64>,
    /// Sequences handed out for chunks that have not been written yet.
    unpersisted: FxHashMap<ChunkPos, i64>,
    next: i64,
    reserved: i64,
}

impl GenerationJournal {
    /// Opens the journal at `path`, creating it when it does not exist, and
    /// rewrites it with one entry per chunk.
    ///
    /// A record cut short at the end of the file, as a crash mid-write leaves
    /// it, is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or written, or if it is not
    /// a generation journal.
    pub fn open(path: PathBuf) -> Result<Self, JournalError> {
        let (sequences, reserved) = match fs::read(&path) {
            Ok(bytes) => parse(&path, &bytes)?,
            Err(source) if source.kind() == io::ErrorKind::NotFound => (FxHashMap::default(), 0),
            Err(source) => {
                return Err(JournalError::Io {
                    action: "read",
                    path,
                    source,
                });
            }
        };
        let file = compact(&path, &sequences, reserved)?;
        Ok(Self {
            path,
            inner: Mutex::new(Inner {
                file,
                sequences,
                unpersisted: FxHashMap::default(),
                // Sequences below the reservation may have reached a client.
                next: reserved,
                reserved,
            }),
            generated: AtomicU64::new(0),
            failure: OnceLock::new(),
        })
    }

    /// Gives `pos` the next sequence because it was just generated. The entry
    /// reaches disk with [`Self::persist`], once the chunk has been written.
    pub fn record_generated(&self, pos: ChunkPos) -> i64 {
        self.generated.fetch_add(1, Ordering::Relaxed);
        self.assign(pos)
    }

    /// Gives `pos` the next sequence because its blocks changed. The entry
    /// reaches disk with [`Self::persist`].
    pub fn record_changed(&self, pos: ChunkPos) -> i64 {
        self.assign(pos)
    }

    /// Records a chunk that is already on disk but missing from the journal
    /// and writes its entry at once.
    pub fn record_existing(&self, pos: ChunkPos) -> i64 {
        let sequence = self.assign(pos);
        self.persist(&[pos]);
        sequence
    }

    /// Writes the entries of the chunks at `positions`, which have just been
    /// written to their region files.
    pub fn persist(&self, positions: &[ChunkPos]) {
        let mut inner = self.lock();
        let mut wrote = false;
        for pos in positions {
            if let Some(sequence) = inner.unpersisted.remove(pos) {
                let result = write_entry(&mut inner.file, *pos, sequence);
                self.check(result, "append to");
                wrote = true;
            }
        }
        if wrote {
            let result = inner.file.flush();
            self.check(result, "flush");
        }
    }

    /// Returns the sequence of the chunk at `pos`, or `None` when it has not
    /// been generated.
    #[must_use]
    pub fn sequence(&self, pos: ChunkPos) -> Option<i64> {
        self.lock().sequences.get(&pos).copied()
    }

    /// Counts the generated chunks closer than `radius` chunks to `centre`.
    #[must_use]
    pub fn count_within(&self, centre: ChunkPos, radius: i32) -> usize {
        let radius_sq = i64::from(radius) * i64::from(radius);
        self.lock()
            .sequences
            .keys()
            .filter(|pos| {
                let dx = i64::from(pos.x - centre.x);
                let dz = i64::from(pos.y - centre.y);
                dx * dx + dz * dz < radius_sq
            })
            .count()
    }

    /// Calls `visit` with each generated chunk closer than `radius` chunks to
    /// `centre` and its sequence, and returns the newest sequence handed out,
    /// or -1. Both come from one consistent view of the journal.
    pub fn for_each_within(
        &self,
        centre: ChunkPos,
        radius: i32,
        mut visit: impl FnMut(ChunkPos, i64),
    ) -> i64 {
        let radius_sq = i64::from(radius) * i64::from(radius);
        let inner = self.lock();
        for (&pos, &sequence) in &inner.sequences {
            let dx = i64::from(pos.x - centre.x);
            let dz = i64::from(pos.y - centre.y);
            if dx * dx + dz * dz < radius_sq {
                visit(pos, sequence);
            }
        }
        inner.next - 1
    }

    /// Returns how many chunks this process has generated in this dimension.
    #[must_use]
    pub fn generated_count(&self) -> u64 {
        self.generated.load(Ordering::Relaxed)
    }

    /// Returns the first write failure, after which the journal on disk no
    /// longer matches the world.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.get().map(String::as_str)
    }

    fn assign(&self, pos: ChunkPos) -> i64 {
        let mut inner = self.lock();
        if inner.next >= inner.reserved {
            let reserved = inner.next + RESERVATION;
            let result = write_record(&mut inner.file, TAG_RESERVE, &reserved.to_be_bytes())
                .and_then(|()| inner.file.flush());
            self.check(result, "reserve sequences in");
            inner.reserved = reserved;
        }
        let sequence = inner.next;
        inner.next += 1;
        inner.sequences.insert(pos, sequence);
        inner.unpersisted.insert(pos, sequence);
        sequence
    }

    fn check(&self, result: io::Result<()>, action: &str) {
        if let Err(source) = result {
            let message = format!(
                "could not {action} the generation journal {}: {source}",
                self.path.display()
            );
            error!("{message}");
            let _ = self.failure.set(message);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn parse(path: &Path, bytes: &[u8]) -> Result<(FxHashMap<ChunkPos, i64>, i64), JournalError> {
    let corrupt = |offset: usize, reason: &str| JournalError::Corrupt {
        path: path.to_path_buf(),
        offset,
        reason: reason.to_owned(),
    };
    if bytes.len() < MAGIC.len() || bytes[..MAGIC.len()] != MAGIC {
        return Err(corrupt(0, "it does not start with the journal header"));
    }
    let records = &bytes[MAGIC.len()..];
    let whole = records.len() - records.len() % RECORD_LEN;
    if whole != records.len() {
        warn!(
            "Dropping a generation journal record cut short at the end of {}",
            path.display()
        );
    }
    let mut sequences = FxHashMap::default();
    let mut reserved = 0;
    for (index, record) in records[..whole]
        .as_chunks::<RECORD_LEN>()
        .0
        .iter()
        .enumerate()
    {
        let offset = MAGIC.len() + index * RECORD_LEN;
        match record[0] {
            TAG_RESERVE => reserved = reserved.max(read_i64(record, 1)),
            TAG_ENTRY => {
                let x = read_i32(record, 1);
                let z = read_i32(record, 5);
                let sequence = read_i64(record, 9);
                if sequence >= reserved {
                    return Err(corrupt(offset, "an entry is above its reservation"));
                }
                sequences.insert(ChunkPos::new(x, z), sequence);
            }
            _ => return Err(corrupt(offset, "unknown record tag")),
        }
    }
    Ok((sequences, reserved))
}

/// Rewrites the journal with one record per chunk and returns it open for
/// appending.
fn compact(
    path: &Path,
    sequences: &FxHashMap<ChunkPos, i64>,
    reserved: i64,
) -> Result<BufWriter<File>, JournalError> {
    let io_error = |action: &'static str, source: io::Error| JournalError::Io {
        action,
        path: path.to_path_buf(),
        source,
    };
    let temporary = path.with_extension("journal.tmp");
    let mut writer = BufWriter::new(File::create(&temporary).map_err(|e| io_error("create", e))?);
    writer
        .write_all(&MAGIC)
        .and_then(|()| write_record(&mut writer, TAG_RESERVE, &reserved.to_be_bytes()))
        .and_then(|()| {
            sequences
                .iter()
                .try_for_each(|(pos, sequence)| write_entry(&mut writer, *pos, *sequence))
        })
        .and_then(|()| writer.flush())
        .and_then(|()| writer.get_ref().sync_all())
        .map_err(|e| io_error("write", e))?;
    drop(writer);
    fs::rename(&temporary, path).map_err(|e| io_error("replace", e))?;
    let file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| io_error("open", e))?;
    Ok(BufWriter::new(file))
}

fn write_entry(file: &mut impl Write, pos: ChunkPos, sequence: i64) -> io::Result<()> {
    let mut payload = [0; 16];
    payload[..4].copy_from_slice(&pos.x.to_be_bytes());
    payload[4..8].copy_from_slice(&pos.y.to_be_bytes());
    payload[8..].copy_from_slice(&sequence.to_be_bytes());
    write_record(file, TAG_ENTRY, &payload)
}

/// Writes one record: its tag, then `payload` padded with zeros to
/// [`RECORD_LEN`].
fn write_record(file: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    let mut record = [0; RECORD_LEN];
    record[0] = tag;
    record[1..=payload.len()].copy_from_slice(payload);
    file.write_all(&record)
}

fn read_i32(record: &[u8; RECORD_LEN], start: usize) -> i32 {
    i32::from_be_bytes(std::array::from_fn(|i| record[start + i]))
}

fn read_i64(record: &[u8; RECORD_LEN], start: usize) -> i64 {
    i64::from_be_bytes(std::array::from_fn(|i| record[start + i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("ironore_generation.journal")
    }

    #[test]
    fn sequences_increase_and_a_change_gets_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
        let a = journal.record_generated(ChunkPos::new(0, 0));
        let b = journal.record_generated(ChunkPos::new(1, 0));
        let a_changed = journal.record_changed(ChunkPos::new(0, 0));
        assert!(a < b && b < a_changed);
        assert_eq!(journal.sequence(ChunkPos::new(0, 0)), Some(a_changed));
        assert_eq!(journal.sequence(ChunkPos::new(5, 5)), None);
        assert_eq!(journal.generated_count(), 2);
    }

    #[test]
    fn only_persisted_entries_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let written = ChunkPos::new(3, -4);
        let unwritten = ChunkPos::new(7, 7);
        let written_sequence = {
            let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
            let sequence = journal.record_generated(written);
            journal.record_generated(unwritten);
            journal.persist(&[written]);
            sequence
        };
        let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
        assert_eq!(journal.sequence(written), Some(written_sequence));
        assert_eq!(journal.sequence(unwritten), None);
    }

    #[test]
    fn a_restart_never_reuses_a_sequence_handed_out_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let before = {
            let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
            // Never persisted, as when the server dies before the chunk is written.
            journal.record_generated(ChunkPos::new(0, 0))
        };
        let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
        let after = journal.record_generated(ChunkPos::new(0, 0));
        assert!(after > before);
    }

    #[test]
    fn existing_chunks_are_written_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let pos = ChunkPos::new(-20, 11);
        let sequence = {
            let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
            journal.record_existing(pos)
        };
        let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
        assert_eq!(journal.sequence(pos), Some(sequence));
        assert_eq!(journal.generated_count(), 0);
    }

    #[test]
    fn a_record_cut_short_at_the_end_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = journal_path(&dir);
        let kept = ChunkPos::new(1, 2);
        let journal = GenerationJournal::open(path.clone()).unwrap();
        journal.record_generated(kept);
        journal.persist(&[kept]);
        drop(journal);
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(&[TAG_ENTRY, 0, 0, 0]);
        fs::write(&path, bytes).unwrap();
        let journal = GenerationJournal::open(path).unwrap();
        assert!(journal.sequence(kept).is_some());
    }

    #[test]
    fn a_foreign_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = journal_path(&dir);
        fs::write(&path, b"not a journal at all").unwrap();
        assert!(matches!(
            GenerationJournal::open(path),
            Err(JournalError::Corrupt { offset: 0, .. })
        ));
    }

    #[test]
    fn visits_the_disc_and_reports_the_newest_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
        assert_eq!(journal.for_each_within(ChunkPos::new(0, 0), 5, |_, _| {}), -1);
        journal.record_generated(ChunkPos::new(1, 1));
        let newest = journal.record_generated(ChunkPos::new(40, 0));
        let mut visited = Vec::new();
        let last = journal.for_each_within(ChunkPos::new(0, 0), 5, |pos, sequence| {
            visited.push((pos, sequence));
        });
        assert_eq!(visited, vec![(ChunkPos::new(1, 1), 0)]);
        assert_eq!(last, newest);
    }

    #[test]
    fn counts_only_chunks_inside_the_disc() {
        let dir = tempfile::tempdir().unwrap();
        let journal = GenerationJournal::open(journal_path(&dir)).unwrap();
        for pos in [(0, 0), (3, 3), (3, 4), (-6, 0)] {
            journal.record_generated(ChunkPos::new(pos.0, pos.1));
        }
        // (3, 4) is exactly 5 away, on the edge, which the disc leaves out.
        assert_eq!(journal.count_within(ChunkPos::new(0, 0), 5), 2);
    }
}
