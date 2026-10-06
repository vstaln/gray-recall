//! Bring the index up to date with `$GRAY_HOME/sessions`: only changed
//! files, and only their new bytes when a file just grew.

use crate::extract::extract;
use crate::index::{self, Row, SessionState};
use crate::project::Resolver;
use crate::redact::redact_secrets;
use crate::session;
use crate::text::fnv1a;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs::{File, TryLockError};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Window hashed at the start of a file to notice rewrites.
pub const HEAD_BYTES: u64 = 4096;
/// Window hashed just before `tail_offset` to notice edits near the tail.
pub const TAIL_BYTES: u64 = 256;
const MAX_THREADS: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Another process holds the writer lock; nothing was written.
    pub busy: bool,
    /// Sessions (re)indexed.
    pub changed: usize,
    /// Sessions whose file is gone.
    pub removed: usize,
    /// Files that could not be read or written; retried next run.
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Reread the whole file and replace all of the session's turns.
    Full,
    /// Parse from `offset` (the last indexed turn's prompt line).
    Tail {
        offset: u64,
        entry: i64,
        cwd: String,
        skipped: u64,
        old_size: u64,
    },
}

pub fn sessions_dir(home: &Path) -> PathBuf {
    home.join("sessions")
}

/// The writer lock, or `None` when another process holds it.
pub fn writer_lock(home: &Path) -> Result<Option<File>> {
    let dir = index::recall_dir(home);
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("index.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

pub fn catch_up(conn: &mut Connection, home: &Path) -> Result<Report> {
    match writer_lock(home)? {
        Some(_lock) => catch_up_locked(conn, home),
        None => Ok(Report {
            busy: true,
            ..Report::default()
        }),
    }
}

pub fn catch_up_locked(conn: &mut Connection, home: &Path) -> Result<Report> {
    let mut report = Report::default();
    let files = list_sessions(&sessions_dir(home))?;
    let mut jobs = Vec::new();
    for (id, path) in &files {
        let state = index::get_session(conn, id)?;
        match stat(path).and_then(|(size, mtime_ns)| {
            Ok((
                size,
                mtime_ns,
                detect(state.as_ref(), path, size, mtime_ns)?,
            ))
        }) {
            Ok((size, mtime_ns, Some((mode, head_hash)))) => jobs.push(Work {
                id: id.clone(),
                path: path.clone(),
                size,
                mtime_ns,
                head_hash,
                mode,
            }),
            Ok((_, _, None)) => {}
            Err(_) => report.failed += 1,
        }
    }
    let present: HashSet<&str> = files.iter().map(|(id, _)| id.as_str()).collect();
    let gone: Vec<String> = index::session_ids(conn)?
        .into_iter()
        .filter(|id| !present.contains(id.as_str()))
        .collect();
    if !gone.is_empty() {
        let tx = conn.transaction()?;
        for id in &gone {
            index::delete_session(&tx, id)?;
        }
        tx.commit()?;
        report.removed = gone.len();
    }
    jobs.sort_by_key(|w| std::cmp::Reverse(w.size));
    parse_parallel(&jobs, |result| {
        match result.and_then(|fresh| write(conn, fresh)) {
            Ok(()) => report.changed += 1,
            Err(_) => report.failed += 1,
        }
        Ok(())
    })?;
    if report.changed + report.removed > 0 {
        index::rebuild_projects(conn)?;
    }
    index::meta_set(conn, index::LAST_CATCH_UP, &now_ms().to_string())?;
    Ok(report)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn list_sessions(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let id = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".jsonl"));
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            out.push((id.to_string(), path.clone()));
        }
    }
    out.sort();
    Ok(out)
}

/// Size and mtime (unix ns) of `path`.
pub fn stat(path: &Path) -> Result<(u64, i64)> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64);
    Ok((meta.len(), mtime))
}

/// How to bring `path` up to date given its stored state, or `None` when it
/// is unchanged. Also returns the hash of the file's current head window.
pub fn detect(
    state: Option<&SessionState>,
    path: &Path,
    size: u64,
    mtime_ns: i64,
) -> Result<Option<(Mode, u64)>> {
    if let Some(s) = state
        && s.size == size
        && s.mtime_ns == mtime_ns
    {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    let head = read_at(&mut file, 0, size.min(HEAD_BYTES))?;
    let head_hash = fnv1a(&head);
    let mut mode = Mode::Full;
    if let Some(s) = state
        && let Some(entry) = s.tail_entry
        && size > s.size
        && head.get(..s.size.min(HEAD_BYTES) as usize).map(fnv1a) == Some(s.head_hash)
        && window_hash(&mut file, s.tail_offset)? == s.tail_hash
    {
        mode = Mode::Tail {
            offset: s.tail_offset,
            entry,
            cwd: s.cwd.clone(),
            skipped: s.skipped,
            old_size: s.size,
        };
    }
    Ok(Some((mode, head_hash)))
}

fn read_at(file: &mut File, from: u64, len: u64) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

fn window_hash(file: &mut File, end: u64) -> Result<u64> {
    let start = end.saturating_sub(TAIL_BYTES);
    Ok(fnv1a(&read_at(file, start, end - start)?))
}

#[derive(Debug, Clone)]
struct Work {
    id: String,
    path: PathBuf,
    size: u64,
    mtime_ns: i64,
    head_hash: u64,
    mode: Mode,
}

/// A parsed session, ready to write.
struct Fresh {
    full: bool,
    state: SessionState,
    rows: Vec<Row>,
}

fn parse_parallel(jobs: &[Work], mut sink: impl FnMut(Result<Fresh>) -> Result<()>) -> Result<()> {
    if jobs.is_empty() {
        return Ok(());
    }
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(MAX_THREADS)
        .min(jobs.len());
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let next = &next;
            s.spawn(move || {
                let mut resolver = Resolver::default();
                while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if tx.send(process(job, &mut resolver)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        rx.into_iter().try_for_each(&mut sink)
    })
}

fn process(work: &Work, resolver: &mut Resolver) -> Result<Fresh> {
    let start = match &work.mode {
        Mode::Full => 0,
        Mode::Tail { offset, .. } => *offset,
    };
    let from = start.saturating_sub(TAIL_BYTES);
    let buf = read_at(
        &mut File::open(&work.path)?,
        from,
        work.size.saturating_sub(from),
    )?;
    let parsed = session::parse(
        buf.get((start - from) as usize..).unwrap_or_default(),
        start,
    );
    let (cwd, skipped) = match &work.mode {
        Mode::Full => (
            parsed.cwd.clone().unwrap_or_default(),
            parsed.skipped_offsets.len() as u64,
        ),
        Mode::Tail {
            entry,
            cwd,
            skipped,
            old_size,
            ..
        } => {
            if parsed.root != Some(*entry)
                || parsed.turns.first().map(|t| t.entry_id) != Some(*entry)
            {
                return process(
                    &Work {
                        mode: Mode::Full,
                        ..work.clone()
                    },
                    resolver,
                );
            }
            let new = parsed
                .skipped_offsets
                .iter()
                .filter(|&&o| o >= *old_size)
                .count() as u64;
            (cwd.clone(), skipped + new)
        }
    };
    let project = resolver.project(&cwd);
    let rows = parsed
        .turns
        .iter()
        .map(|t| {
            let e = extract(t, Path::new(&cwd), Path::new(&project.key));
            Row {
                rowid: 0,
                session: work.id.clone(),
                entry_id: t.entry_id,
                project_key: project.key.clone(),
                project_name: project.name.clone(),
                cwd: cwd.clone(),
                date: t.date,
                prompt: redact_secrets(&t.prompt),
                reply: redact_secrets(&e.reply),
                gist: redact_secrets(&e.gist),
                commands: redact_secrets(&e.commands),
                files: e.files.iter().map(|f| redact_secrets(f)).collect(),
                commits: e.commits,
                failed: e.failed,
                has_reply: e.has_reply,
            }
        })
        .collect();
    let last = parsed.turns.last();
    let tail_offset = last.map_or(0, |t| t.offset);
    let window =
        (tail_offset.saturating_sub(TAIL_BYTES) - from) as usize..(tail_offset - from) as usize;
    Ok(Fresh {
        full: work.mode == Mode::Full,
        state: SessionState {
            session: work.id.clone(),
            path: work.path.to_string_lossy().into_owned(),
            cwd,
            project_key: project.key,
            size: work.size,
            mtime_ns: work.mtime_ns,
            head_hash: work.head_hash,
            tail_offset,
            tail_hash: fnv1a(buf.get(window).unwrap_or_default()),
            tail_entry: last.map(|t| t.entry_id),
            skipped,
        },
        rows,
    })
}

fn write(conn: &mut Connection, fresh: Fresh) -> Result<()> {
    let tx = conn.transaction()?;
    if fresh.full {
        index::delete_session_turns(&tx, &fresh.state.session)?;
    }
    for row in &fresh.rows {
        index::insert_turn(&tx, row)?;
    }
    index::put_session(&tx, &fresh.state)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "catchup_tests.rs"]
mod catchup_tests;
