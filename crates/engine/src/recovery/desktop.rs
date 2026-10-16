use super::RecoveryInfo;
use crate::Session;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wordcraft_doc::Document;

const INTERVAL: Duration = Duration::from_secs(10);
const MAX_BYTES: u64 = 256 << 20;
const MAX_RECORDS: usize = 128;
const MAX_FILES: usize = 16;
const DISK_BUDGET: u64 = 2 << 30;
/// Times a record lock held for a moment (by a process being started) is tried again.
const LOCK_RETRIES: u32 = 10;

/// Size caps on snapshot files, smaller in tests.
#[derive(Clone, Copy)]
struct Limits {
    /// Encoded bytes in one snapshot.
    snapshot: u64,
    /// Snapshot bytes in the whole recovery folder.
    folder: u64,
}
const LIMITS: Limits = Limits { snapshot: MAX_BYTES, folder: DISK_BUDGET };

/// Why a worker job failed.
#[derive(Debug)]
enum Failure {
    /// The snapshot passed `limit` (the folder's remaining room if `folder`) at `at_least` bytes.
    TooLarge {
        at_least: u64,
        limit: u64,
        folder: bool,
    },
    Other(String),
}
impl From<String> for Failure {
    fn from(e: String) -> Self {
        Self::Other(e)
    }
}
impl From<&str> for Failure {
    fn from(e: &str) -> Self {
        Self::Other(e.into())
    }
}
/// The File › Recover message for a snapshot that didn't fit.
fn size_message(folder: bool) -> &'static str {
    if folder { super::FOLDER_FULL_MESSAGE } else { super::TOO_LARGE_MESSAGE }
}
impl From<Failure> for String {
    fn from(f: Failure) -> Self {
        match f {
            Failure::TooLarge { folder, .. } => size_message(folder).into(),
            Failure::Other(e) => e,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    version: u32,
    pub info: RecoveryInfo,
    generation: u64,
    pub document: Document,
}
struct Entry {
    info: RecoveryInfo,
    path: PathBuf,
}
struct Store {
    root: PathBuf,
    entries: BTreeMap<String, Entry>,
    // Advisory locks prevent another running instance from restoring/discarding a live copy.
    locks: BTreeMap<String, File>,
    limits: Limits,
    /// Records whose last snapshot didn't fit: the next is measured before anything is written.
    oversize: BTreeSet<String>,
    /// Temporary files created, so tests can tell an attempt wrote nothing.
    #[cfg(test)]
    temp_files: usize,
}
fn private_dir(path: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
}
fn private_file(path: &Path, exclusive: bool) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true);
    if exclusive {
        o.create_new(true);
    } else {
        o.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}
fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
fn id_ok(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn new_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn generation(path: &Path) -> Option<u64> {
    path.file_name()?.to_str()?.strip_prefix("snapshot-")?.strip_suffix(".json")?.parse().ok()
}
fn read_snapshot(path: &Path) -> Result<Snapshot, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err("not a regular recovery file, or it exceeds 256 MiB".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular recovery file".into());
    }
    let s: Snapshot = serde_json::from_reader(BufReader::new(file.take(MAX_BYTES + 1))).map_err(|e| e.to_string())?;
    if s.version != 1 || !id_ok(&s.info.id) {
        return Err("unsupported recovery file".into());
    }
    Ok(s)
}
impl Store {
    fn new(root: PathBuf) -> Result<Self, String> {
        private_dir(&root).map_err(|e| e.to_string())?;
        Ok(Self {
            root,
            entries: BTreeMap::new(),
            locks: BTreeMap::new(),
            limits: LIMITS,
            oversize: BTreeSet::new(),
            #[cfg(test)]
            temp_files: 0,
        })
    }
    fn directories(&self) -> Result<Vec<PathBuf>, String> {
        let mut dirs = Vec::new();
        for e in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            if e.file_type().map_err(|e| e.to_string())?.is_dir() && e.file_name().to_str().is_some_and(id_ok) {
                if dirs.len() >= MAX_RECORDS {
                    return Err("recovery folder has too many records".into());
                }
                dirs.push(e.path());
            }
        }
        Ok(dirs)
    }
    fn files(dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            if e.file_type().map_err(|e| e.to_string())?.is_file() && generation(&e.path()).is_some() {
                if paths.len() >= MAX_FILES {
                    return Err("recovery record has too many generations".into());
                }
                paths.push(e.path());
            }
        }
        paths.sort_by_key(|p| std::cmp::Reverse(generation(p)));
        Ok(paths)
    }
    /// Locks a record's `owner.lock`, retrying [`LOCK_RETRIES`] times 10 ms apart.
    fn lock(&self, id: &str) -> Result<File, String> {
        if !id_ok(id) {
            return Err("invalid recovery id".into());
        }
        let file = private_file(&self.root.join(id).join("owner.lock"), false).map_err(|e| e.to_string())?;
        // A process another part of the app (or a test) is starting holds the lock for a moment
        // (a forked child shares it until it execs): wait that out, but not a live owner.
        for _ in 0..LOCK_RETRIES {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(format!("recovery copy is in use or cannot be locked: {e}")),
            }
        }
        file.try_lock().map_err(|e| format!("recovery copy is in use or cannot be locked: {e}"))?;
        Ok(file)
    }
    fn acquire(&mut self, id: &str) -> Result<(), String> {
        if !self.locks.contains_key(id) {
            let lock = self.lock(id)?;
            self.locks.insert(id.to_string(), lock);
        }
        Ok(())
    }
    fn scan(&mut self) -> Result<(), String> {
        let mut entries = BTreeMap::new();
        let mut unreadable = false;
        for dir in self.directories()? {
            let Some(id) = dir.file_name().and_then(|s| s.to_str()) else { continue };
            let guard = if self.locks.contains_key(id) {
                None
            } else {
                match self.lock(id) {
                    Ok(f) => Some(f),
                    Err(e) => {
                        log::debug!("recovery: {e}");
                        continue;
                    }
                }
            };
            // A crashed writer can leave an incomplete temporary file. Only clean records
            // we have locked, so another instance's write is never interrupted.
            for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_file()
                    && entry.file_name().to_str().is_some_and(|n| n.strip_prefix("pending-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(id_ok))
                {
                    fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
                }
            }
            for path in Self::files(&dir)? {
                match read_snapshot(&path) {
                    Ok(s) if s.info.id == id && Some(s.generation) == generation(&path) => {
                        entries.insert(id.to_string(), Entry { info: s.info, path });
                        break;
                    }
                    Ok(_) => {
                        unreadable = true;
                        log::warn!("recovery: mismatched record {}", path.display());
                    }
                    Err(e) => {
                        unreadable = true;
                        log::warn!("recovery: unreadable {}: {e}", path.display());
                    }
                }
            }
            drop(guard);
            // A crash before the first commit leaves no document to recover. Reclaim
            // that slot only under a fresh lock and the discard path's entry checks.
            if Self::files(&dir)?.is_empty()
                && let Err(e) = self.discard(id)
            {
                log::warn!("recovery: cannot remove empty record: {e}");
            }
        }
        self.entries = entries;
        if unreadable { Err("Some recovery files could not be read. Earlier valid copies are listed when available.".into()) } else { Ok(()) }
    }
    fn inventory(&self) -> Vec<RecoveryInfo> {
        let mut v: Vec<_> = self.entries.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(a.id.cmp(&b.id)));
        v
    }
    fn disk_used(&self) -> Result<u64, String> {
        let mut used = 0u64;
        for dir in self.directories()? {
            for p in Self::files(&dir)? {
                used = used.saturating_add(fs::metadata(p).map_err(|e| e.to_string())?.len());
            }
        }
        Ok(used)
    }
    fn save(&mut self, s: Snapshot) -> Result<(), Failure> {
        let id = s.info.id.clone();
        let measured = self.oversize.contains(&id);
        let result = self.write_snapshot(s);
        // Include failures before temp-file creation (quota checks, locking, etc.).
        // A record containing any committed generation is never removed here.
        if result.is_err()
            && Self::files(&self.root.join(&id)).is_ok_and(|files| files.is_empty())
            && let Err(e) = self.discard(&id)
        {
            log::warn!("recovery: cannot remove failed empty record: {e}");
        }
        // Other failures say nothing about size, so they keep the measuring.
        if matches!(result, Err(Failure::TooLarge { .. })) || (measured && result.is_err()) {
            self.oversize.insert(id);
        } else {
            self.oversize.remove(&id);
        }
        result
    }
    fn write_snapshot(&mut self, mut s: Snapshot) -> Result<(), Failure> {
        let room = self.limits.folder.saturating_sub(self.disk_used()?);
        let cap = self.limits.snapshot;
        // After a snapshot that didn't fit, measure the next in memory before touching its record.
        if self.oversize.contains(&s.info.id) {
            encode(io::sink(), &s, room, cap)?;
        }
        let dir = self.root.join(&s.info.id);
        if !dir.exists() {
            if self.directories()?.len() >= MAX_RECORDS {
                return Err("recovery folder is full; discard an older copy".into());
            }
            private_dir(&dir).map_err(|e| e.to_string())?;
        }
        self.acquire(&s.info.id)?;
        let files = Self::files(&dir)?;
        if files.len() >= MAX_FILES {
            return Err("older recovery generations could not be pruned; keeping the last valid copy".into());
        }
        s.generation = files.first().and_then(|p| generation(p)).unwrap_or(0).checked_add(1).ok_or("recovery generation overflow")?;
        let previous = self.entries.get(&s.info.id).map(|e| e.path.clone());
        let tmp = dir.join(format!("pending-{}.tmp", new_id()?));
        let final_path = dir.join(format!("snapshot-{:020}.json", s.generation));
        #[cfg(test)]
        {
            self.temp_files += 1;
        }
        let result = (|| -> Result<(), Failure> {
            let file = private_file(&tmp, true).map_err(|e| e.to_string())?;
            let mut out = encode(BufWriter::new(file), &s, room, cap)?;
            out.flush().map_err(|e| e.to_string())?;
            out.get_ref().sync_all().map_err(|e| e.to_string())?;
            drop(out);
            let check = read_snapshot(&tmp)?;
            if check.document != s.document {
                return Err("recovery format did not preserve the full document".into());
            }
            // A new generation is never written over the last valid file.
            fs::rename(&tmp, &final_path).map_err(|e| e.to_string())?;
            sync_dir(&dir).map_err(|e| e.to_string())?;
            self.entries.insert(s.info.id.clone(), Entry { info: s.info, path: final_path.clone() });
            for old in Self::files(&dir)?.into_iter().filter(|p| p != &final_path && Some(p) != previous.as_ref()) {
                if let Err(e) = fs::remove_file(old) {
                    log::warn!("recovery: cannot prune old generation: {e}");
                }
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
    fn load(&mut self, id: &str) -> Result<Snapshot, String> {
        if !self.entries.contains_key(id) {
            return Err("no such recovery copy".into());
        }
        self.acquire(id)?;
        read_snapshot(&self.entries.get(id).ok_or("no such recovery copy")?.path)
    }
    fn discard(&mut self, id: &str) -> Result<(), String> {
        if !id_ok(id) {
            return Err("invalid recovery id".into());
        }
        self.oversize.remove(id);
        let dir = self.root.join(id);
        if !dir.exists() {
            self.entries.remove(id);
            return Ok(());
        }
        self.acquire(id)?;
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let known = name == "owner.lock"
                || generation(&entry.path()).is_some()
                || name.to_str().is_some_and(|n| n.strip_prefix("pending-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(id_ok));
            if !known || !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("recovery folder contains an unexpected entry".into());
            }
        }
        for p in Self::files(&dir)? {
            fs::remove_file(p).map_err(|e| e.to_string())?;
        }
        for e in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            let name = e.file_name();
            if name.to_str().is_some_and(|n| n.strip_prefix("pending-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(id_ok)) {
                fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            }
        }
        sync_dir(&dir).map_err(|e| e.to_string())?;
        self.entries.remove(id);
        self.locks.remove(id);
        fs::remove_file(dir.join("owner.lock")).map_err(|e| e.to_string())?;
        fs::remove_dir(&dir).map_err(|e| e.to_string())?;
        sync_dir(&self.root).map_err(|e| e.to_string())?;
        Ok(())
    }
}
struct Limited<W> {
    inner: W,
    /// Bytes past this are counted, not written.
    limit: u64,
    /// Writing fails once the count passes this.
    cap: u64,
    count: u64,
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let total = self.count.saturating_add(bytes.len() as u64);
        if total > self.cap {
            self.count = total;
            return Err(io::Error::other("recovery snapshot size limit exceeded"));
        }
        let n = if total > self.limit { bytes.len() } else { self.inner.write(bytes)? };
        self.count = self.count.saturating_add(n as u64);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
/// Encode `s` into `out`, writing at most `room` bytes and counting on to `cap` to tell a full
/// folder from a snapshot over the cap.
fn encode<W: Write>(out: W, s: &Snapshot, room: u64, cap: u64) -> Result<W, Failure> {
    let limit = room.min(cap);
    let mut w = Limited { inner: out, limit, cap, count: 0 };
    match serde_json::to_writer(&mut w, s) {
        Ok(()) if w.count <= limit => Ok(w.inner),
        Ok(()) => Err(Failure::TooLarge { at_least: w.count, limit, folder: true }),
        Err(_) if w.count > cap => Err(Failure::TooLarge { at_least: w.count, limit: cap, folder: false }),
        Err(e) => Err(Failure::Other(e.to_string())),
    }
}
enum Job {
    Save(Box<Snapshot>),
    Scan,
    Load(String, (u64, u64)),
    Discard(String),
}
enum Outcome {
    Done,
    Discarded(String),
    Loaded(Box<Snapshot>, (u64, u64)),
}
struct Event {
    documents: Vec<RecoveryInfo>,
    outcome: Result<Outcome, Failure>,
}
pub(crate) struct Recovery {
    tx: mpsc::SyncSender<Job>,
    rx: mpsc::Receiver<Event>,
    pub documents: Vec<RecoveryInfo>,
    pub ready: bool,
    pub busy: bool,
    pub error: Option<String>,
    document: Option<u64>,
    id: Option<String>,
    revision: Option<u64>,
    due: Instant,
    alive: bool,
    /// The document whose too-large snapshot was logged, so the warning appears once per document.
    warned: Option<u64>,
    /// The document of the snapshot under way.
    saving: Option<u64>,
    /// `Some(folder_full)` while the current document's last snapshot doesn't fit.
    too_large: Option<bool>,
}
impl Recovery {
    pub fn start(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        Self::start_with(root, wake, LIMITS)
    }
    fn start_with(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>, limits: Limits) -> Result<Self, String> {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (events, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("wordcraft-recovery".into())
            .spawn(move || {
                let mut store = match Store::new(root) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = events.send(Event { documents: Vec::new(), outcome: Err(e.into()) });
                        wake();
                        return;
                    }
                };
                store.limits = limits;
                let initial = store.scan().map(|_| Outcome::Done).map_err(Failure::Other);
                if events.send(Event { documents: store.inventory(), outcome: initial }).is_err() {
                    return;
                }
                wake();
                while let Ok(job) = jobs.recv() {
                    let result = match job {
                        Job::Save(s) => store.save(*s).map(|_| Outcome::Done),
                        Job::Scan => store.scan().map(|_| Outcome::Done).map_err(Failure::Other),
                        Job::Load(id, expected) => store.load(&id).map(|s| Outcome::Loaded(Box::new(s), expected)).map_err(Failure::Other),
                        Job::Discard(id) => store.discard(&id).map(|_| Outcome::Discarded(id)).map_err(Failure::Other),
                    };
                    if events.send(Event { documents: store.inventory(), outcome: result }).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            rx,
            documents: Vec::new(),
            ready: false,
            busy: true,
            error: None,
            document: None,
            id: None,
            revision: None,
            due: Instant::now() + INTERVAL,
            alive: true,
            warned: None,
            saving: None,
            too_large: None,
        })
    }
    fn queue(&mut self, job: Job) -> Result<(), String> {
        if !self.alive {
            return Err("recovery worker is unavailable".into());
        }
        if self.busy {
            return Err("recovery is busy; try again when it finishes".into());
        }
        self.tx.try_send(job).map_err(|e| e.to_string())?;
        self.busy = true;
        self.error = None;
        Ok(())
    }
    /// The error to show: the last job's, else why the document has no recovery copy.
    pub fn message(&self) -> Option<&str> {
        self.error.as_deref().or(self.too_large.map(size_message))
    }
    pub fn poll(&mut self) -> Option<(Snapshot, (u64, u64))> {
        match self.rx.try_recv() {
            Ok(e) => {
                self.busy = false;
                self.ready = true;
                self.documents = e.documents;
                let saved = self.saving.take();
                if saved.is_some() {
                    self.too_large = None;
                }
                match e.outcome {
                    Ok(Outcome::Done) => {
                        // A rescan may show room freed elsewhere, so a full folder is tried again.
                        if self.too_large == Some(true) {
                            self.revision = None;
                        }
                        None
                    }
                    Ok(Outcome::Discarded(id)) => {
                        if self.id.as_deref() == Some(&id) {
                            self.id = None;
                        } else if self.too_large == Some(true) {
                            // Discarding another copy frees room in a full folder.
                            self.revision = None;
                        }
                        None
                    }
                    Ok(Outcome::Loaded(s, expected)) => Some((*s, expected)),
                    Err(Failure::TooLarge { at_least, limit, folder }) => {
                        if self.warned != saved {
                            self.warned = saved;
                            let cap = if folder { "remaining folder" } else { "snapshot" };
                            log::warn!("recovery: snapshot reached {at_least} bytes, over the {limit}-byte {cap} limit; this revision gets no copy");
                        }
                        // A late result for a document no longer open says nothing about this one.
                        if saved.is_some() && saved == self.document {
                            // Keep the attempted revision: only an edit tries again.
                            self.too_large = Some(folder);
                            self.due = Instant::now() + INTERVAL;
                        }
                        None
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        self.revision = None;
                        self.due = Instant::now() + INTERVAL;
                        None
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                self.alive = false;
                if self.error.is_none() {
                    self.error = Some("recovery worker is unavailable".into());
                }
                None
            }
        }
    }
    pub fn adopt(&mut self, id: String, document: u64, revision: u64) {
        self.too_large = None;
        self.id = Some(id);
        self.document = Some(document);
        self.revision = Some(revision);
        self.due = Instant::now() + INTERVAL;
    }
    pub fn tick(&mut self, s: &Session, now: Instant) -> Result<(), String> {
        if self.document != Some(s.document_id()) {
            self.document = Some(s.document_id());
            self.id = None;
            self.revision = None;
            self.too_large = None;
        }
        if self.busy || !self.ready || !self.alive {
            return Ok(());
        }
        if !s.dirty || s.password.is_some() {
            self.too_large = None;
            if let Some(id) = self.id.clone() {
                self.queue(Job::Discard(id))?;
            }
            return Ok(());
        }
        if self.revision == Some(s.rev()) || now < self.due {
            return Ok(());
        }
        let id = match &self.id {
            Some(id) => id.clone(),
            None => new_id()?,
        };
        let name = s.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled draft".into());
        let info = RecoveryInfo { id: id.clone(), name, saved_at: crate::cmd::now_iso() };
        self.queue(Job::Save(Box::new(Snapshot { version: 1, info, generation: 0, document: s.doc.clone() })))?;
        self.saving = self.document;
        self.id = Some(id);
        self.revision = Some(s.rev());
        self.due = now + INTERVAL;
        Ok(())
    }
    pub fn document_is(&self, document: u64) -> bool {
        self.document == Some(document)
    }
    pub fn wait(&self, document: u64, revision: u64) -> Option<Duration> {
        if !self.alive || !self.ready || self.busy || (self.document == Some(document) && self.revision == Some(revision)) {
            return None;
        }
        Some(self.due.saturating_duration_since(Instant::now()).max(Duration::from_millis(100)))
    }
    pub fn restore(&mut self, id: &str, expected: (u64, u64)) -> Result<(), String> {
        if !self.documents.iter().any(|d| d.id == id) {
            return Err("no such recovery copy".into());
        }
        self.queue(Job::Load(id.into(), expected))
    }
    pub fn discard(&mut self, id: &str, current: (u64, u64)) -> Result<(), String> {
        if !self.documents.iter().any(|d| d.id == id) {
            return Err("no such recovery copy".into());
        }
        self.queue(Job::Discard(id.into()))?;
        if self.id.as_deref() == Some(id) {
            self.revision = Some(current.1);
        }
        Ok(())
    }
    pub fn refresh(&mut self) -> Result<(), String> {
        self.queue(Job::Scan)
    }
    pub fn purge(&mut self) -> Result<(), String> {
        let Some(id) = self.id.clone() else { return Ok(()) };
        if self.busy {
            let e = self.rx.recv().map_err(|e| e.to_string())?;
            self.documents = e.documents;
            self.busy = false;
            self.ready = true;
            self.saving = None;
            if matches!(e.outcome, Ok(Outcome::Loaded(..))) {
                return Err("finish recovery before protecting this document".into());
            }
        }
        self.queue(Job::Discard(id))?;
        let e = self.rx.recv().map_err(|e| e.to_string())?;
        self.busy = false;
        self.documents = e.documents;
        e.outcome?;
        self.id = None;
        self.revision = None;
        self.too_large = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wordcraft_doc::StoryRef;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("wordcraft-recovery-test-{}", new_id().unwrap()));
            private_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    /// A store reopened on `root` whose scan reports an unreadable record. The record's lock can be
    /// briefly held by a process another test is starting (a forked child holds the descriptor
    /// until it execs), and a record that can't be locked is skipped, not reported: so try again
    /// for up to a second rather than read a skipped record as a clean scan.
    fn reopen_reporting_unreadable(root: &std::path::Path) -> Store {
        for _ in 0..50 {
            let mut store = Store::new(root.to_path_buf()).unwrap();
            if store.scan().is_err() {
                return store;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the scan never reported the unreadable record");
    }
    /// `store.load(id)`, tried again for up to a second while the record can't be locked: a process
    /// another test is starting holds the lock for a moment (see [`reopen_reporting_unreadable`]).
    fn load_retrying(store: &mut Store, id: &str) -> Snapshot {
        let mut last = String::new();
        for _ in 0..50 {
            match store.load(id) {
                Ok(snapshot) => return snapshot,
                Err(e) => last = e,
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the record never loaded: {last}");
    }
    fn snapshot(id: &str, text: &str) -> Snapshot {
        Snapshot {
            version: 1,
            info: RecoveryInfo { id: id.into(), name: "Synthetic draft".into(), saved_at: crate::cmd::now_iso() },
            generation: 0,
            document: Document::from_text(text),
        }
    }
    fn settle(s: &mut Session) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            s.recovery_tick();
            if s.recovery_ready() && !s.recovery_busy() {
                break;
            }
            assert!(Instant::now() < end, "recovery did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn session(root: &Path) -> Session {
        let mut s = Session::new(Document::new());
        s.enable_recovery(root.to_owned(), Arc::new(|| {})).unwrap();
        settle(&mut s);
        s
    }
    fn save_now(s: &mut Session) {
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        settle(s);
    }
    /// A session whose recovery worker uses `limits` instead of the real caps.
    fn limited_session(root: &Path, limits: Limits) -> Session {
        let mut s = Session::new(Document::new());
        s.recovery = Some(Recovery::start_with(root.to_owned(), Arc::new(|| {}), limits).unwrap());
        settle(&mut s);
        s
    }
    /// Tick with the interval elapsed and report whether a snapshot was queued.
    fn attempts(s: &mut Session) -> bool {
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        let queued = s.recovery_busy();
        settle(s);
        queued
    }

    #[test]
    fn recovery_commands_only_address_known_profile_records() {
        use crate::file_access::{AutomationRoots, confine};
        let profile = Temp::new();
        let files = Temp::new();
        let original = files.0.join("original.txt");
        fs::write(&original, b"External synthetic original").unwrap();
        let roots = AutomationRoots::new(Some(&files.0), Some(&files.0)).unwrap().unwrap();
        let mut s = session(&profile.0);
        s.run("document.setText", &json!({"text": "Recoverable synthetic draft"})).unwrap();
        save_now(&mut s);
        let id = s.recovery_documents()[0].id.clone();
        confine(Some(&roots), || {
            // A command cannot use a file path or choose a different storage folder.
            for bad in [original.to_string_lossy().into_owned(), "../original.txt".into()] {
                for action in ["restore", "discard"] {
                    let err = s.run("file.recover", &json!({action: bad, "root": files.0})).unwrap_err();
                    assert!(err.to_string().contains("no such recovery copy"), "{err}");
                }
            }
            assert_eq!(s.recovery_documents().len(), 1);
            s.run("file.recover", &json!({"restore": id})).unwrap();
            settle(&mut s);
            assert!(s.path.is_none());
            assert!(s.dirty);
            assert_eq!(s.doc.plain_text(StoryRef::Body), "Recoverable synthetic draft");
            s.run("file.recover", &json!({"discard": id})).unwrap();
            settle(&mut s);
            assert!(s.recovery_documents().is_empty());
        });
        assert_eq!(fs::read(&original).unwrap(), b"External synthetic original");
        assert_eq!(fs::read_dir(&files.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_write_keeps_last_valid_generation_and_discovery_falls_back() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "First valid copy")).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        // Simulate a final rename failure without relying on permissions (root can bypass them).
        let obstruction = t.0.join(&id).join("snapshot-00000000000000000002.json");
        fs::create_dir(&obstruction).unwrap();
        assert!(store.save(snapshot(&id, "Failed copy")).is_err());
        assert_eq!(fs::read(&first).unwrap(), bytes);
        fs::remove_dir(obstruction).unwrap();
        store.save(snapshot(&id, "Second valid copy")).unwrap();
        let second = store.entries[&id].path.clone();
        fs::write(second, b"{truncated").unwrap();
        drop(store);
        let mut reopened = reopen_reporting_unreadable(&t.0);
        assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "First valid copy");
        // Never reuse the corrupt generation's name or prune the valid fallback.
        reopened.save(snapshot(&id, "Third valid copy")).unwrap();
        assert_eq!(fs::read(first).unwrap(), bytes);
        assert_eq!(Store::files(&t.0.join(&id)).unwrap().len(), 2);
    }

    #[test]
    fn size_limit_and_lossy_values_fail_without_committing() {
        let mut out = Limited { inner: Vec::new(), limit: 3, cap: 3, count: 0 };
        assert!(out.write_all(b"four").is_err());
        assert!(out.inner.is_empty());
        assert_eq!(out.count, 4);
        // Past the limit but under the cap, bytes are counted and not written.
        let mut out = Limited { inner: Vec::new(), limit: 2, cap: 8, count: 0 };
        out.write_all(b"four").unwrap();
        assert!(out.inner.is_empty());
        assert_eq!(out.count, 4);
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "valid")).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        let mut invalid = snapshot(&id, "invalid");
        invalid.document.last_section.page_w = f32::NAN;
        assert!(store.save(invalid).is_err());
        assert_eq!(fs::read(first).unwrap(), bytes);
        assert_eq!(Store::files(&t.0.join(id)).unwrap().len(), 1);
    }

    #[test]
    fn a_snapshot_that_did_not_fit_is_measured_before_anything_is_written() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        let small = snapshot(&id, "Small synthetic draft");
        let limit = serde_json::to_vec(&small).unwrap().len() as u64 + 1024;
        store.limits.snapshot = limit;
        store.save(small).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        let large = || snapshot(&id, &"Synthetic words ".repeat(400));
        match store.save(large()) {
            Err(Failure::TooLarge { at_least, limit: l, folder: false }) => assert!(l == limit && at_least > limit, "{at_least} {l}"),
            other => panic!("expected a size failure, got {other:?}"),
        }
        assert_eq!(store.temp_files, 2);
        // Still too large: measured in memory, and no file is written.
        assert!(matches!(store.save(large()), Err(Failure::TooLarge { folder: false, .. })));
        assert_eq!(store.temp_files, 2);
        assert_eq!(fs::read(&first).unwrap(), bytes);
        let names: Vec<_> = fs::read_dir(t.0.join(&id)).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 2, "only the lock and the valid copy: {names:?}");
        // A failure that isn't about size keeps the measuring.
        let mut lossy = snapshot(&id, "Lossy synthetic draft");
        lossy.document.last_section.page_w = f32::NAN;
        assert!(matches!(store.save(lossy), Err(Failure::Other(_))));
        assert_eq!(store.temp_files, 3);
        assert!(matches!(store.save(large()), Err(Failure::TooLarge { .. })));
        assert_eq!(store.temp_files, 3);
        // Once it fits again it is written.
        store.save(snapshot(&id, "Smaller synthetic draft")).unwrap();
        assert_eq!(store.temp_files, 4);
        assert!(store.oversize.is_empty());
        assert_eq!(load_retrying(&mut store, &id).document.plain_text(StoryRef::Body), "Smaller synthetic draft");
    }

    #[test]
    fn a_full_recovery_folder_is_reported_as_full() {
        let t = Temp::new();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&new_id().unwrap(), "First synthetic draft")).unwrap();
        store.limits.folder = store.disk_used().unwrap() + 64;
        let second = new_id().unwrap();
        let small = snapshot(&second, "Second synthetic draft");
        let size = serde_json::to_vec(&small).unwrap().len() as u64;
        let err = store.save(small).unwrap_err();
        // Counted to the end, so the log gives the snapshot's size.
        assert!(matches!(err, Failure::TooLarge { at_least, limit: 64, folder: true } if at_least == size), "{err:?}");
        assert_eq!(String::from(err), crate::recovery::FOLDER_FULL_MESSAGE);
        assert!(!t.0.join(&second).exists(), "a failed first write leaves no record");
        // A snapshot over the cap as well is too large, not a full folder.
        store.limits.snapshot = size;
        let err = store.save(snapshot(&second, &"Synthetic words ".repeat(400))).unwrap_err();
        assert!(matches!(err, Failure::TooLarge { limit, folder: false, .. } if limit == size), "{err:?}");
        assert_eq!(String::from(err), crate::recovery::TOO_LARGE_MESSAGE);
    }

    #[test]
    fn a_too_large_document_is_tried_again_only_after_it_changes() {
        let t = Temp::new();
        let base = serde_json::to_vec(&Session::new(Document::new()).doc).unwrap().len() as u64;
        let mut s = limited_session(&t.0, Limits { snapshot: base + 4096, folder: DISK_BUDGET });
        s.run("text.insert", &json!({"text": "Small synthetic draft"})).unwrap();
        save_now(&mut s);
        assert!(s.recovery_error().is_none());
        let id = s.recovery_documents()[0].id.clone();
        let first = Store::files(&t.0.join(&id)).unwrap()[0].clone();
        let bytes = fs::read(&first).unwrap();
        s.run("text.insert", &json!({"text": " synthetic words".repeat(1000)})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_error(), Some(crate::recovery::TOO_LARGE_MESSAGE));
        assert_eq!(s.recovery.as_ref().unwrap().warned, Some(s.document_id()));
        // The previous valid copy stays listed and unchanged.
        assert_eq!(s.recovery_documents().len(), 1);
        assert_eq!(Store::files(&t.0.join(&id)).unwrap(), vec![first.clone()]);
        assert_eq!(fs::read(&first).unwrap(), bytes);
        assert!(s.recovery_wait().is_none(), "no idle wakeups for an unchanged document");
        // Refreshing the list keeps the message and doesn't retry a document over the cap.
        s.run("file.recover", &json!({"refresh": true})).unwrap();
        settle(&mut s);
        assert_eq!(s.recovery_error(), Some(crate::recovery::TOO_LARGE_MESSAGE));
        assert!(s.recovery_wait().is_none());
        // An edit allows one attempt, at the normal interval.
        s.run("text.insert", &json!({"text": "!"})).unwrap();
        assert!(s.recovery_wait().is_some());
        s.recovery_tick();
        assert!(!s.recovery_busy(), "not before the interval");
        assert!(attempts(&mut s));
        assert_eq!(s.recovery_error(), Some(crate::recovery::TOO_LARGE_MESSAGE));
        // Unchanged: no new attempt, however long it waits.
        for _ in 0..3 {
            assert!(!attempts(&mut s), "an unchanged document is not tried again");
        }
        assert!(s.recovery_wait().is_none());
        s.run("text.insert", &json!({"text": "?"})).unwrap();
        assert!(attempts(&mut s));
        assert!(!attempts(&mut s), "one attempt per change");
        assert_eq!(fs::read(&first).unwrap(), bytes);
    }

    #[test]
    fn a_full_folder_is_tried_again_when_another_copy_is_discarded() {
        let t = Temp::new();
        let ours = serde_json::to_vec(&Session::new(Document::new()).doc).unwrap().len();
        let other = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&other, &"Other synthetic draft ".repeat(ours / 10 + 200))).unwrap();
        let used = store.disk_used().unwrap();
        drop(store);
        let mut s = limited_session(&t.0, Limits { snapshot: MAX_BYTES, folder: used + 64 });
        assert_eq!(s.recovery_documents().len(), 1);
        s.run("text.insert", &json!({"text": "Synthetic draft"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_error(), Some(crate::recovery::FOLDER_FULL_MESSAGE));
        assert!(s.recovery_wait().is_none(), "an unchanged document waits");
        // A refresh keeps the message and allows one retry, which still doesn't fit.
        s.run("file.recover", &json!({"refresh": true})).unwrap();
        settle(&mut s);
        assert_eq!(s.recovery_error(), Some(crate::recovery::FOLDER_FULL_MESSAGE));
        assert!(s.recovery_wait().is_some());
        assert!(attempts(&mut s));
        assert_eq!(s.recovery_error(), Some(crate::recovery::FOLDER_FULL_MESSAGE));
        assert!(s.recovery_wait().is_none());
        // Discarding the other copy makes room: the same revision is tried again and fits.
        s.run("file.recover", &json!({"discard": other})).unwrap();
        settle(&mut s);
        assert_eq!(s.recovery_error(), Some(crate::recovery::FOLDER_FULL_MESSAGE));
        assert!(s.recovery_wait().is_some());
        assert!(attempts(&mut s));
        assert!(s.recovery_error().is_none());
        let documents = s.recovery_documents();
        assert_eq!(documents.len(), 1);
        assert_ne!(documents[0].id, other);
    }

    #[test]
    fn a_late_size_failure_does_not_show_on_the_next_document() {
        let t = Temp::new();
        let base = serde_json::to_vec(&Session::new(Document::new()).doc).unwrap().len() as u64;
        let mut s = limited_session(&t.0, Limits { snapshot: base + 4096, folder: DISK_BUDGET });
        s.run("text.insert", &json!({"text": " synthetic words".repeat(1000)})).unwrap();
        let first = s.document_id();
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        assert!(s.recovery_busy());
        // The host sees the new document, already edited, before the snapshot's result arrives.
        s.run("file.new", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "Second synthetic draft"})).unwrap();
        let mut r = s.recovery.take().unwrap();
        r.tick(&s, Instant::now()).unwrap();
        s.recovery = Some(r);
        settle(&mut s);
        assert!(s.recovery_error().is_none());
        assert_eq!(s.recovery.as_ref().unwrap().warned, Some(first));
    }

    #[test]
    fn other_failures_are_retried_after_the_interval() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Synthetic draft"})).unwrap();
        // A value the format can't keep fails the write's check, not the size limit.
        s.doc.last_section.page_w = f32::NAN;
        save_now(&mut s);
        let error = s.recovery_error().unwrap().to_string();
        assert_ne!(error, crate::recovery::TOO_LARGE_MESSAGE);
        assert!(s.recovery_wait().is_some(), "a failed write schedules another");
        s.recovery_tick();
        assert!(!s.recovery_busy(), "not before the interval");
        assert!(attempts(&mut s), "the same revision is tried again");
        assert!(attempts(&mut s));
        assert!(s.recovery_documents().is_empty());
    }

    #[test]
    fn failed_pruning_cannot_push_a_record_beyond_the_discovery_limit() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "Last valid copy")).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        // A failed cleanup must not allow the next write to exceed the reader's bound.
        for n in 2..=MAX_FILES {
            fs::write(t.0.join(&id).join(format!("snapshot-{n:020}.json")), b"{truncated").unwrap();
        }
        assert!(store.save(snapshot(&id, "New copy")).is_err());
        assert_eq!(fs::read(first).unwrap(), bytes);
        drop(store);
        let mut reopened = reopen_reporting_unreadable(&t.0);
        assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "Last valid copy");
    }

    #[test]
    fn failed_first_write_does_not_consume_a_record_slot() {
        let t = Temp::new();
        let mut store = Store::new(t.0.clone()).unwrap();
        let mut invalid = snapshot(&new_id().unwrap(), "Synthetic draft");
        invalid.document.last_section.page_w = f32::NAN;
        assert!(store.save(invalid).is_err());
        assert!(store.directories().unwrap().is_empty());
    }

    #[test]
    fn discovery_reclaims_a_crashed_first_write_but_keeps_valid_records() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let empty = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "Valid synthetic draft")).unwrap();
        drop(store);
        let dir = t.0.join(&empty);
        private_dir(&dir).unwrap();
        fs::write(dir.join(format!("pending-{}.tmp", new_id().unwrap())), b"{unfinished").unwrap();
        let mut reopened = Store::new(t.0.clone()).unwrap();
        reopened.scan().unwrap();
        assert!(!dir.exists());
        assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "Valid synthetic draft");
    }

    #[test]
    fn live_records_are_locked_and_discard_releases_storage() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut owner = Store::new(t.0.clone()).unwrap();
        owner.save(snapshot(&id, "live")).unwrap();
        let mut other = Store::new(t.0.clone()).unwrap();
        other.scan().unwrap();
        assert!(other.inventory().is_empty());
        assert!(other.discard(&id).is_err());
        assert!(other.discard("../outside").is_err());
        drop(owner);
        other.scan().unwrap();
        assert_eq!(other.inventory().len(), 1);
        other.discard(&id).unwrap();
        assert!(!t.0.join(id).exists(), "discard must release the record slot");
    }

    #[test]
    fn new_draft_restarts_restores_unsaved_and_explicitly_discards() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Unsaved synthetic draft"})).unwrap();
        save_now(&mut s);
        assert!(s.recovery_wait().is_none(), "unchanged documents must not schedule idle work");
        let expected = s.doc.clone();
        drop(s);
        let mut s = session(&t.0);
        let id = s.recovery_documents()[0].id.clone();
        s.run("file.recover", &json!({"restore": id})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc, expected);
        assert!(s.dirty && s.path.is_none());
        s.run("file.recover", &json!({"discard": id})).unwrap();
        settle(&mut s);
        save_now(&mut s);
        assert!(s.recovery_documents().is_empty(), "discard isn't undone by the next timer");
        assert_eq!(s.doc, expected, "discard deletes the copy, not the editing document");
    }

    #[test]
    fn existing_file_is_unchanged_and_successful_save_removes_copy() {
        let t = Temp::new();
        let file = t.0.join("original.docx");
        let root = t.0.join("profile");
        crate::io::save_path(&file, &Document::from_text("Original business note")).unwrap();
        let original = fs::read(&file).unwrap();
        let mut s = session(&root);
        s.run("file.open", &json!({"path": file})).unwrap();
        s.run("text.insert", &json!({"text": "Unsaved edit. "})).unwrap();
        save_now(&mut s);
        let expected = s.doc.clone();
        drop(s);
        let mut s = session(&root);
        let id = s.recovery_documents()[0].id.clone();
        s.run("file.recover", &json!({"restore": id})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc, expected);
        assert!(s.path.is_none() && s.dirty);
        assert_eq!(fs::read(&file).unwrap(), original);
        let saved = t.0.join("recovered.json");
        s.run("file.save", &json!({"path": saved})).unwrap();
        settle(&mut s);
        assert!(s.recovery_documents().is_empty());
        assert_eq!(fs::read(file).unwrap(), original);
    }

    #[test]
    fn password_barrier_clears_in_flight_plaintext_and_blocks_new_copies() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Confidential synthetic draft"})).unwrap();
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        assert!(s.recovery_busy());
        s.run("file.encrypt", &json!({"password": "synthetic-password"})).unwrap();
        assert!(s.recovery_documents().is_empty());
        s.run("text.insert", &json!({"text": "More confidential edits"})).unwrap();
        save_now(&mut s);
        assert!(fs::read_dir(&t.0).unwrap().next().is_none());
        // Removing protection makes future recovery possible again.
        s.run("file.encrypt", &json!({"password": null})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
    }

    #[test]
    fn failed_password_cleanup_rejects_protection_and_preserves_snapshot() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Synthetic text"})).unwrap();
        save_now(&mut s);
        let id = s.recovery_documents()[0].id.clone();
        let dir = t.0.join(id);
        // An unknown file prevents removal: don't report that plaintext was purged.
        let first = Store::files(&dir).unwrap()[0].clone();
        let bytes = fs::read(&first).unwrap();
        fs::write(dir.join("unexpected"), b"unexpected").unwrap();
        assert!(s.run("file.encrypt", &json!({"password": "synthetic-password"})).is_err());
        assert!(s.password.is_none());
        assert_eq!(fs::read(first).unwrap(), bytes);
    }

    #[test]
    fn delayed_restore_cannot_replace_new_edits() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Saved draft"})).unwrap();
        save_now(&mut s);
        drop(s);
        let mut s = session(&t.0);
        let id = s.recovery_documents()[0].id.clone();
        s.run("file.recover", &json!({"restore": id})).unwrap();
        s.run("text.insert", &json!({"text": "New work"})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "New work");
        assert!(s.recovery_error().unwrap().contains("document changed"));
    }

    #[test]
    fn opening_protected_file_and_password_save_never_leave_plaintext_copies() {
        let t = Temp::new();
        let original = t.0.join("protected.docx");
        let root = t.0.join("profile");
        crate::io::save_path_with(
            &original,
            &Document::from_text("Synthetic confidential note"),
            Some("synthetic-password"),
            &crate::io::ExportOptions::default(),
        )
        .unwrap();
        let bytes = fs::read(&original).unwrap();
        let mut s = session(&root);
        s.run("file.open", &json!({"path": original, "password": "synthetic-password"})).unwrap();
        s.run("text.insert", &json!({"text": "Confidential edit"})).unwrap();
        save_now(&mut s);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        assert_eq!(fs::read(&original).unwrap(), bytes);
        s.run("file.new", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "New confidential draft"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
        let saved = t.0.join("saved.docx");
        s.run("file.save", &json!({"path": saved, "password": "synthetic-password"})).unwrap();
        settle(&mut s);
        assert!(fs::read_dir(root).unwrap().next().is_none());
        assert_eq!(fs::read(original).unwrap(), bytes);
    }

    #[test]
    fn protection_after_document_switch_does_not_discard_another_draft() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "First synthetic draft"})).unwrap();
        save_now(&mut s);
        s.run("file.new", &json!({})).unwrap();
        // Protection can be commanded before the host's next recovery tick.
        s.run("file.encrypt", &json!({"password": "synthetic-password"})).unwrap();
        settle(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
    }
}
