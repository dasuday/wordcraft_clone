use super::{RecoveryGap, RecoveryInfo};
use crate::Session;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak, mpsc};
use std::time::{Duration, Instant};
use wordcraft_doc::Document;

const INTERVAL: Duration = Duration::from_secs(10);
/// A snapshot takes at most this share of the worker's time: after one that took `t`, the next
/// waits at least `t * DUTY`. A two-second snapshot of a large document every ten seconds kept a
/// fifth of a core busy while typing.
const DUTY: u32 = 20;
/// The longest wait between snapshots of a changed document, however slow they are.
const MAX_INTERVAL: Duration = Duration::from_secs(300);
/// The largest generation file: the text and structure. Pictures and parts are kept beside it
/// ([`PARTS`]) and count toward [`RECORD_BUDGET`] and [`DISK_BUDGET`] instead.
const MAX_BYTES: u64 = 256 << 20;
/// A record's whole size: its generation files and the parts they use.
const RECORD_BUDGET: u64 = 1 << 30;
const MAX_RECORDS: usize = 128;
const MAX_FILES: usize = 16;
/// The most part files one record may hold (its generations share them).
const MAX_PARTS: usize = 65_536;
/// The whole recovery folder: every record's generations and parts.
const DISK_BUDGET: u64 = 2 << 30;
/// Times a record lock held for a moment (by a process being started) is tried again.
const LOCK_RETRIES: u32 = 10;
/// The generation format. Version 2 keeps each picture and kept part once per record, in a file
/// named by its SHA-256 under [`PARTS`]; the JSON names it ([`PartRef`]). Version 1 (still read)
/// kept them inside the document JSON, as base64 or, before that, arrays of numbers.
const VERSION: u32 = 2;
/// A record's folder of part files, `parts/<sha256>`.
const PARTS: &str = "parts";

/// Size limits of the recovery store. Tests use small ones to reach them without writing
/// gigabytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    /// One generation file (the JSON alone).
    pub main: u64,
    /// One record: its generations and part files together.
    pub record: u64,
    /// The whole recovery folder.
    pub disk: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self { main: MAX_BYTES, record: RECORD_BUDGET, disk: DISK_BUDGET }
    }
}

/// A picture or kept part in the record's `parts/` folder: its file is named by the SHA-256 of
/// its bytes, so an unchanged picture is written once and shared by later generations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PartRef {
    /// SHA-256 of the bytes, 64 lowercase hex digits: the file's name. Checked on restore.
    part: String,
    len: u64,
}
/// Where a version 2 snapshot's `Document::media` and `Document::passthrough` went.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Parts {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    media: BTreeMap<String, PartRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    passthrough: BTreeMap<String, PartRef>,
}
impl Parts {
    fn is_empty(&self) -> bool {
        self.media.is_empty() && self.passthrough.is_empty()
    }
    /// The part files these name.
    fn files(&self) -> BTreeSet<String> {
        self.media.values().chain(self.passthrough.values()).map(|r| r.part.clone()).collect()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    version: u32,
    pub info: RecoveryInfo,
    generation: u64,
    /// In version 2, without `media` and `passthrough`: those are in [`Self::parts`].
    pub document: Document,
    #[serde(default, skip_serializing_if = "Parts::is_empty")]
    parts: Parts,
}
impl Snapshot {
    pub fn new(info: RecoveryInfo, document: Document) -> Self {
        Self { version: VERSION, info, generation: 0, document, parts: Parts::default() }
    }
}
struct Entry {
    info: RecoveryInfo,
    path: PathBuf,
    /// The part files this generation uses.
    parts: BTreeSet<String>,
}
/// Why a snapshot failed. `until_changed`: the snapshot itself can't be written (over a size
/// limit or budget, or a value the format can't keep), so it isn't tried again until the
/// document changes (#2625). Other failures (a lock held for a moment, an I/O error) are retried
/// at the next interval.
#[derive(Debug)]
struct Failure {
    message: String,
    until_changed: bool,
}
impl Failure {
    fn until_changed(message: impl Into<String>) -> Self {
        Self { message: message.into(), until_changed: true }
    }
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { message, until_changed: false }
    }
}
impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}
struct Store {
    root: PathBuf,
    entries: BTreeMap<String, Entry>,
    // Advisory locks prevent another running instance from restoring/discarding a live copy.
    locks: BTreeMap<String, File>,
    /// SHA-256 of pictures already hashed, by allocation, so an unchanged picture isn't hashed
    /// again at every snapshot. A changed picture is a new allocation (`Arc::make_mut` clones a
    /// shared one and disowns weak references), which the `Weak` check tells apart.
    hashes: HashMap<usize, (Weak<Vec<u8>>, [u8; 32])>,
    /// Bytes written to files in the recovery folder so far (tests and measurements).
    written: u64,
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
/// Make an existing recovery folder private too: `private_dir`'s mode applies only to folders it
/// creates, and copies of unsaved documents must not be readable by other users.
fn tighten(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(path)?;
        if !meta.is_dir() {
            return Err(io::Error::other("the recovery folder is not a folder"));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(meta.permissions().mode() & 0o700))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
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
    Ok(hex(&bytes))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn generation(path: &Path) -> Option<u64> {
    path.file_name()?.to_str()?.strip_prefix("snapshot-")?.strip_suffix(".json")?.parse().ok()
}
/// A part file's name: 64 lowercase hex digits. Anything else a copy names (a path, `..`) is
/// refused, so a hostile copy can't point outside its record.
fn part_ok(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn pending_ok(name: &str) -> bool {
    name.strip_prefix("pending-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(id_ok)
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
    let known = s.version == VERSION || (s.version == 1 && s.parts.is_empty());
    if !known || !id_ok(&s.info.id) {
        return Err("unsupported recovery file".into());
    }
    if !s.parts.files().iter().all(|p| part_ok(p)) {
        return Err("recovery file names an invalid part".into());
    }
    Ok(s)
}
/// A part's bytes from the record folder `dir`, checked against the length and SHA-256 the
/// snapshot recorded: a missing, truncated or altered file is an error.
fn read_part(dir: &Path, part: &PartRef) -> Result<Vec<u8>, String> {
    if !part_ok(&part.part) {
        return Err("invalid part name".into());
    }
    let path = dir.join(PARTS).join(&part.part);
    let meta = fs::symlink_metadata(&path).map_err(|e| format!("missing: {e}"))?;
    if !meta.is_file() || meta.len() != part.len || part.len > RECORD_BUDGET {
        return Err("not a regular file of the recorded length".into());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(part.len).unwrap_or(0));
    File::open(&path).map_err(|e| e.to_string())?.take(part.len.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 != part.len || hex(&sha256(&bytes)) != part.part {
        return Err("its bytes don't match the copy's record of them".into());
    }
    Ok(bytes)
}
/// A picture or part a snapshot may keep in a part file.
struct Candidate {
    media: bool,
    name: String,
    bytes: Arc<Vec<u8>>,
    part: PartRef,
}
impl Store {
    fn new(root: PathBuf) -> Result<Self, String> {
        private_dir(&root).map_err(|e| e.to_string())?;
        tighten(&root).map_err(|e| e.to_string())?;
        Ok(Self { root, entries: BTreeMap::new(), locks: BTreeMap::new(), hashes: HashMap::new(), written: 0 })
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
    /// A record's part files and their sizes (none before its first part).
    fn parts(dir: &Path) -> Result<BTreeMap<String, u64>, String> {
        let mut parts = BTreeMap::new();
        let entries = match fs::read_dir(dir.join(PARTS)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(parts),
            Err(e) => return Err(e.to_string()),
        };
        for e in entries {
            let e = e.map_err(|e| e.to_string())?;
            let Some(name) = e.file_name().to_str().filter(|n| part_ok(n)).map(str::to_string) else { continue };
            let meta = e.metadata().map_err(|e| e.to_string())?;
            if meta.is_file() {
                if parts.len() >= MAX_PARTS {
                    return Err("recovery record has too many parts".into());
                }
                parts.insert(name, meta.len());
            }
        }
        Ok(parts)
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
            // One record that can't be read (say, too many generations) is skipped and reported;
            // it must not hide the others.
            match self.scan_record(&dir, id, &mut entries) {
                Ok(complete) => unreadable |= !complete,
                Err(e) => {
                    unreadable = true;
                    log::warn!("recovery: skipping record {}: {e}", dir.display());
                }
            }
        }
        self.entries = entries;
        if unreadable { Err("Some recovery files could not be read. Earlier valid copies are listed when available.".into()) } else { Ok(()) }
    }
    /// List record `id`'s newest valid generation in `entries`. `Ok(false)` when a newer one
    /// couldn't be read.
    fn scan_record(&mut self, dir: &Path, id: &str, entries: &mut BTreeMap<String, Entry>) -> Result<bool, String> {
        let guard = if self.locks.contains_key(id) {
            None
        } else {
            match self.lock(id) {
                Ok(f) => Some(f),
                Err(e) => {
                    log::debug!("recovery: {e}");
                    return Ok(true);
                }
            }
        };
        let mut complete = true;
        // A crashed writer can leave an incomplete temporary file. Only clean records
        // we have locked, so another instance's write is never interrupted.
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_file() && entry.file_name().to_str().is_some_and(pending_ok) {
                fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
        for path in Self::files(dir)? {
            match read_snapshot(&path) {
                Ok(s) if s.info.id == id && Some(s.generation) == generation(&path) => {
                    entries.insert(id.to_string(), Entry { parts: s.parts.files(), info: s.info, path });
                    break;
                }
                Ok(_) => {
                    complete = false;
                    log::warn!("recovery: mismatched record {}", path.display());
                }
                Err(e) => {
                    complete = false;
                    log::warn!("recovery: unreadable {}: {e}", path.display());
                }
            }
        }
        drop(guard);
        // A crash before the first commit leaves no document to recover. Reclaim
        // that slot only under a fresh lock and the discard path's entry checks.
        if Self::files(dir)?.is_empty()
            && let Err(e) = self.discard(id)
        {
            log::warn!("recovery: cannot remove empty record: {e}");
        }
        Ok(complete)
    }
    fn inventory(&self) -> Vec<RecoveryInfo> {
        let mut v: Vec<_> = self.entries.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(a.id.cmp(&b.id)));
        v
    }
    /// Bytes in the recovery folder's records (generations and part files), leaving out the
    /// record folder `except`.
    fn disk_used(&self, except: &Path) -> Result<u64, String> {
        let mut used = 0u64;
        for dir in self.directories()? {
            if dir == except {
                continue;
            }
            for p in Self::files(&dir)? {
                used = used.saturating_add(fs::metadata(p).map_err(|e| e.to_string())?.len());
            }
            used = used.saturating_add(Self::parts(&dir)?.values().sum());
        }
        Ok(used)
    }
    /// SHA-256 of a picture, remembered while that allocation lives.
    fn hash(&mut self, bytes: &Arc<Vec<u8>>) -> [u8; 32] {
        let key = Arc::as_ptr(bytes) as usize;
        if let Some((weak, hash)) = self.hashes.get(&key)
            && weak.upgrade().is_some_and(|a| Arc::ptr_eq(&a, bytes))
        {
            return *hash;
        }
        let hash = sha256(bytes);
        self.hashes.insert(key, (Arc::downgrade(bytes), hash));
        hash
    }
    /// Write `data` to `target`: exclusively to a temporary file in the record folder `dir`,
    /// synchronized, then renamed into place.
    fn write_part(&mut self, dir: &Path, target: &Path, data: &[u8]) -> Result<(), String> {
        let tmp = dir.join(format!("pending-{}.tmp", new_id()?));
        let result = (|| {
            let mut file = private_file(&tmp, true).map_err(|e| e.to_string())?;
            file.write_all(data).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            self.written = self.written.saturating_add(data.len() as u64);
            fs::rename(&tmp, target).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
    /// Save a generation of `s` within `limits`. `Ok` names the pictures and parts it had no
    /// room for: the text and structure are always kept when anything is.
    fn save(&mut self, s: Snapshot, limits: Limits) -> Result<Vec<String>, Failure> {
        let id = s.info.id.clone();
        let result = self.write_snapshot(s, limits);
        // Include failures before temp-file creation (quota checks, locking, etc.).
        // A record containing any committed generation is never removed here.
        if result.is_err()
            && Self::files(&self.root.join(&id)).is_ok_and(|files| files.is_empty())
            && let Err(e) = self.discard(&id)
        {
            log::warn!("recovery: cannot remove failed empty record: {e}");
        }
        result
    }
    fn write_snapshot(&mut self, mut s: Snapshot, limits: Limits) -> Result<Vec<String>, Failure> {
        let id = s.info.id.clone();
        let dir = self.root.join(&id);
        if !dir.exists() {
            if self.directories()?.len() >= MAX_RECORDS {
                return Err(Failure::until_changed("recovery folder is full; discard an older copy"));
            }
            private_dir(&dir).map_err(|e| e.to_string())?;
        }
        tighten(&dir).map_err(|e| e.to_string())?;
        self.acquire(&id)?;
        let files = Self::files(&dir)?;
        if files.len() >= MAX_FILES {
            return Err("older recovery generations could not be pruned; keeping the last valid copy".into());
        }
        s.version = VERSION;
        s.generation = files.first().and_then(|p| generation(p)).unwrap_or(0).checked_add(1).ok_or("recovery generation overflow")?;
        let previous = self.entries.get(&id).map(|e| (e.path.clone(), e.parts.clone()));
        let existing = Self::parts(&dir)?;
        // The parts the retained generation uses stay until its successor is committed. With no
        // listed generation, every part file found stays until the next one.
        let kept_before: BTreeSet<String> = match &previous {
            Some((_, parts)) => parts.clone(),
            None => existing.keys().cloned().collect(),
        };
        let previous_main = previous.as_ref().and_then(|(p, _)| fs::metadata(p).ok()).map_or(0, |m| m.len());

        // Pictures and parts leave the JSON for part files named by their content.
        let media = std::mem::take(&mut s.document.media);
        let passthrough = std::mem::take(&mut s.document.passthrough);
        let mut candidates = Vec::new();
        for (media, map) in [(true, media), (false, passthrough)] {
            for (name, bytes) in map {
                let hash = self.hash(&bytes);
                let part = PartRef { part: hex(&hash), len: bytes.len() as u64 };
                candidates.push(Candidate { media, name, bytes, part });
            }
        }
        self.hashes.retain(|_, (w, _)| w.strong_count() > 0);
        // Smallest first: when the budget runs out, the most pictures are kept.
        candidates.sort_by(|a, b| a.part.len.cmp(&b.part.len).then_with(|| a.name.cmp(&b.name)));

        // Plan within the budgets before writing anything: the JSON first, measured with every
        // part named (an upper bound), then the part files that fit.
        for c in &candidates {
            let map = if c.media { &mut s.parts.media } else { &mut s.parts.passthrough };
            map.insert(c.name.clone(), c.part.clone());
        }
        let mut counter = Limited { inner: io::sink(), remaining: limits.main };
        serde_json::to_writer(&mut counter, &s).map_err(|e| {
            Failure::until_changed(if e.is_io() {
                "the document's text and structure are too large for a recovery copy".into()
            } else {
                e.to_string()
            })
        })?;
        let main = limits.main.saturating_sub(counter.remaining);
        let limit = limits.record.min(limits.disk.saturating_sub(self.disk_used(&dir)?));
        let mut total = previous_main.saturating_add(main).saturating_add(kept_before.iter().filter_map(|f| existing.get(f)).sum());
        if total > limit {
            return Err(Failure::until_changed("there is no room for a recovery copy of the text: discard older recovery copies"));
        }
        let mut planned = BTreeSet::new();
        let mut omitted = Vec::new();
        let mut plan = Vec::new();
        for c in candidates {
            let on_disk = existing.get(&c.part.part).copied();
            let cost = if planned.contains(&c.part.part) || (kept_before.contains(&c.part.part) && on_disk == Some(c.part.len)) {
                0
            } else {
                // A file already there that the retained generation doesn't use would be pruned:
                // keeping it costs its size. One of the wrong length is written again.
                c.part.len.max(on_disk.unwrap_or(0))
            };
            if total.saturating_add(cost) > limit {
                omitted.push(c.name);
                continue;
            }
            total = total.saturating_add(cost);
            planned.insert(c.part.part.clone());
            plan.push(c);
        }

        // New part files only: an unchanged picture's file is already there.
        s.parts = Parts::default();
        let parts_dir = dir.join(PARTS);
        let mut written = BTreeSet::new();
        for c in plan {
            let present = written.contains(&c.part.part) || existing.get(&c.part.part) == Some(&c.part.len);
            if !present {
                if written.is_empty() {
                    private_dir(&parts_dir).map_err(|e| e.to_string())?;
                    tighten(&parts_dir).map_err(|e| e.to_string())?;
                }
                self.write_part(&dir, &parts_dir.join(&c.part.part), &c.bytes)?;
                written.insert(c.part.part.clone());
            }
            let map = if c.media { &mut s.parts.media } else { &mut s.parts.passthrough };
            map.insert(c.name, c.part);
        }
        if !written.is_empty() {
            sync_dir(&parts_dir).map_err(|e| e.to_string())?;
        }
        omitted.sort();
        s.info.omitted = omitted.clone();

        // The generation itself.
        let tmp = dir.join(format!("pending-{}.tmp", new_id()?));
        let final_path = dir.join(format!("snapshot-{:020}.json", s.generation));
        let result = (|| {
            let file = private_file(&tmp, true).map_err(|e| e.to_string())?;
            let mut writer = Limited { inner: BufWriter::new(file), remaining: limits.main };
            serde_json::to_writer(&mut writer, &s).map_err(|e| e.to_string())?;
            writer.flush().map_err(|e| e.to_string())?;
            writer.inner.get_ref().sync_all().map_err(|e| e.to_string())?;
            self.written = self.written.saturating_add(limits.main.saturating_sub(writer.remaining));
            drop(writer);
            // The format must give back exactly this document: a value JSON can't keep (a NaN,
            // say) fails here, and would again until the document changes.
            let check = read_snapshot(&tmp).map_err(Failure::until_changed)?;
            if check.document != s.document || check.parts != s.parts {
                return Err(Failure::until_changed("recovery format did not preserve the full document"));
            }
            // Every part it names is there, at its length. The hash is checked on restore, so
            // this doesn't read the pictures back.
            let on_disk = Self::parts(&dir)?;
            if !s.parts.media.values().chain(s.parts.passthrough.values()).all(|r| on_disk.get(&r.part) == Some(&r.len)) {
                return Err("a part of the recovery copy is missing".into());
            }
            // A new generation is never written over the last valid file.
            fs::rename(&tmp, &final_path).map_err(|e| e.to_string())?;
            sync_dir(&dir).map_err(|e| e.to_string())?;
            let used = s.parts.files();
            self.entries.insert(s.info.id.clone(), Entry { info: s.info, path: final_path.clone(), parts: used.clone() });
            let previous_path = previous.map(|(p, _)| p);
            for old in Self::files(&dir)?.into_iter().filter(|p| p != &final_path && Some(p) != previous_path.as_ref()) {
                if let Err(e) = fs::remove_file(old) {
                    log::warn!("recovery: cannot prune old generation: {e}");
                }
            }
            // Part files neither this generation nor the retained one uses.
            for name in Self::parts(&dir)?.into_keys().filter(|f| !used.contains(f) && !kept_before.contains(f)) {
                if let Err(e) = fs::remove_file(parts_dir.join(name)) {
                    log::warn!("recovery: cannot remove an unused part: {e}");
                }
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map(|()| omitted)
    }
    /// The copy `id`, with its pictures and parts. A part file that is missing or damaged
    /// (wrong length or hash) leaves out that picture, named in `info.omitted`, never the text.
    fn load(&mut self, id: &str) -> Result<Snapshot, String> {
        if !self.entries.contains_key(id) {
            return Err("no such recovery copy".into());
        }
        self.acquire(id)?;
        let mut s = read_snapshot(&self.entries.get(id).ok_or("no such recovery copy")?.path)?;
        let parts = std::mem::take(&mut s.parts);
        let dir = self.root.join(id);
        for (media, map) in [(true, parts.media), (false, parts.passthrough)] {
            for (name, part) in map {
                match read_part(&dir, &part) {
                    Ok(bytes) => {
                        let store = if media { &mut s.document.media } else { &mut s.document.passthrough };
                        store.insert(name, Arc::new(bytes));
                    }
                    Err(e) => {
                        log::warn!("recovery: the part {name} of {id} could not be restored: {e}");
                        s.info.omitted.push(name);
                    }
                }
            }
        }
        s.info.omitted.sort();
        s.info.omitted.dedup();
        Ok(s)
    }
    fn discard(&mut self, id: &str) -> Result<(), String> {
        if !id_ok(id) {
            return Err("invalid recovery id".into());
        }
        let dir = self.root.join(id);
        if !dir.exists() {
            self.entries.remove(id);
            return Ok(());
        }
        self.acquire(id)?;
        let parts_dir = dir.join(PARTS);
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let known_file = name == "owner.lock" || generation(&entry.path()).is_some() || name.to_str().is_some_and(pending_ok);
            if !(kind.is_file() && known_file || kind.is_dir() && name == PARTS) {
                return Err("recovery folder contains an unexpected entry".into());
            }
        }
        if parts_dir.is_dir() {
            for entry in fs::read_dir(&parts_dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if !entry.file_type().map_err(|e| e.to_string())?.is_file() || !entry.file_name().to_str().is_some_and(part_ok) {
                    return Err("recovery folder contains an unexpected entry".into());
                }
            }
        }
        for p in Self::files(&dir)? {
            fs::remove_file(p).map_err(|e| e.to_string())?;
        }
        for e in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            if e.file_name().to_str().is_some_and(pending_ok) {
                fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            }
        }
        if parts_dir.is_dir() {
            for e in fs::read_dir(&parts_dir).map_err(|e| e.to_string())? {
                fs::remove_file(e.map_err(|e| e.to_string())?.path()).map_err(|e| e.to_string())?;
            }
            fs::remove_dir(&parts_dir).map_err(|e| e.to_string())?;
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
    remaining: u64,
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("recovery snapshot or folder size limit exceeded"));
        }
        let n = self.inner.write(bytes)?;
        self.remaining -= n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
enum Job {
    Save(Box<Snapshot>, Limits),
    Scan,
    Load(String, (u64, u64)),
    Discard(String),
}
enum Outcome {
    Done,
    /// A copy was written; the pictures and parts it had no room for.
    Saved(Vec<String>),
    Discarded(String),
    Loaded(Box<Snapshot>, (u64, u64)),
}
struct Event {
    documents: Vec<RecoveryInfo>,
    outcome: Result<Outcome, String>,
    /// How long a snapshot took to write (also when it failed), to pace the next ([`DUTY`]).
    saved_in: Option<Duration>,
    /// The snapshot failed in a way only a change to the document can clear ([`Failure`]).
    until_changed: bool,
}
pub(crate) struct Recovery {
    /// `None` once dropped, which ends the worker ([`Drop`]).
    tx: Option<mpsc::SyncSender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
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
    /// Copies of documents that were saved and then replaced before a tick discarded them:
    /// removed when the worker is free, so they aren't offered as unsaved work at the next start.
    stale: Vec<String>,
    pub(crate) limits: Limits,
    /// What the current document's last snapshot couldn't cover (`None`: all of it).
    gap: Option<RecoveryGap>,
    /// The document whose snapshot is being written, so its result isn't taken for another's.
    saving_for: Option<u64>,
}
impl Recovery {
    pub fn start(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (events, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("wordcraft-recovery".into())
            .spawn(move || {
                let mut store = match Store::new(root) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = events.send(Event { documents: Vec::new(), outcome: Err(e), saved_in: None, until_changed: false });
                        wake();
                        return;
                    }
                };
                let initial = store.scan().map(|_| Outcome::Done);
                if events.send(Event { documents: store.inventory(), outcome: initial, saved_in: None, until_changed: false }).is_err() {
                    return;
                }
                wake();
                while let Ok(job) = jobs.recv() {
                    let (mut saved_in, mut until_changed) = (None, false);
                    let result = match job {
                        Job::Save(s, limits) => {
                            let started = Instant::now();
                            let result = store.save(*s, limits).map(Outcome::Saved).map_err(|f| {
                                until_changed = f.until_changed;
                                f.message
                            });
                            saved_in = Some(started.elapsed());
                            result
                        }
                        Job::Scan => store.scan().map(|_| Outcome::Done),
                        Job::Load(id, expected) => store.load(&id).map(|s| Outcome::Loaded(Box::new(s), expected)),
                        Job::Discard(id) => store.discard(&id).map(|_| Outcome::Discarded(id)),
                    };
                    if events.send(Event { documents: store.inventory(), outcome: result, saved_in, until_changed }).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
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
            stale: Vec::new(),
            limits: Limits::default(),
            gap: None,
            saving_for: None,
        })
    }
    fn queue(&mut self, job: Job) -> Result<(), String> {
        if !self.alive {
            return Err("recovery worker is unavailable".into());
        }
        if self.busy {
            return Err("recovery is busy; try again when it finishes".into());
        }
        self.tx.as_ref().ok_or("recovery worker is unavailable")?.try_send(job).map_err(|e| e.to_string())?;
        self.busy = true;
        self.error = None;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<(Snapshot, (u64, u64))> {
        match self.rx.try_recv() {
            Ok(e) => {
                self.busy = false;
                self.ready = true;
                self.documents = e.documents;
                // What a snapshot of the current document covers (not one of a document replaced
                // while it was being written).
                if e.saved_in.is_some() && self.saving_for.take() == self.document {
                    self.gap = match &e.outcome {
                        Ok(Outcome::Saved(omitted)) if !omitted.is_empty() => Some(RecoveryGap::Omitted(omitted.clone())),
                        Err(error) => Some(RecoveryGap::Failed(error.clone())),
                        _ => None,
                    };
                }
                let loaded = match e.outcome {
                    Ok(Outcome::Done | Outcome::Saved(_)) => None,
                    Ok(Outcome::Discarded(id)) => {
                        if self.id.as_deref() == Some(&id) {
                            self.id = None;
                        }
                        None
                    }
                    Ok(Outcome::Loaded(s, expected)) => Some((*s, expected)),
                    Err(error) => {
                        self.error = Some(error);
                        // A snapshot over a size limit would fail the same way, writing as much
                        // again: keep its revision so it is tried only after the next change
                        // (#2625). Other failures may clear by themselves and are retried.
                        if !e.until_changed {
                            self.revision = None;
                        }
                        self.due = Instant::now() + INTERVAL;
                        None
                    }
                };
                // A slow snapshot spaces out the next, so recovery stays a small share of a core.
                if let Some(took) = e.saved_in {
                    self.due = self.due.max(Instant::now() + took.saturating_mul(DUTY).min(MAX_INTERVAL));
                }
                loaded
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
        self.id = Some(id);
        self.document = Some(document);
        self.revision = Some(revision);
        self.gap = None;
        self.due = Instant::now() + INTERVAL;
    }
    pub fn tick(&mut self, s: &Session, now: Instant) -> Result<(), String> {
        if self.document != Some(s.document_id()) {
            self.document = Some(s.document_id());
            self.id = None;
            self.revision = None;
            self.gap = None;
        }
        if self.busy || !self.ready || !self.alive {
            return Ok(());
        }
        if let Some(id) = self.stale.pop() {
            return self.queue(Job::Discard(id));
        }
        if !s.dirty || s.password.is_some() {
            self.gap = None;
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
        let info = RecoveryInfo { id: id.clone(), name, saved_at: crate::cmd::now_iso(), omitted: Vec::new() };
        self.queue(Job::Save(Box::new(Snapshot::new(info, s.doc.clone())), self.limits))?;
        self.saving_for = self.document;
        self.id = Some(id);
        self.revision = Some(s.rev());
        self.due = now + INTERVAL;
        Ok(())
    }
    /// What the current document's last snapshot couldn't cover.
    pub fn gap(&self) -> Option<&RecoveryGap> {
        self.gap.as_ref()
    }
    pub fn alive(&self) -> bool {
        self.alive
    }
    pub fn due_now(&mut self) {
        self.due = Instant::now();
    }
    pub fn document_is(&self, document: u64) -> bool {
        self.document == Some(document)
    }
    /// The document `document` is being replaced while saved: its copy, if a tick hasn't
    /// discarded it yet, is out of date.
    pub fn replaced_clean(&mut self, document: u64) {
        if self.document == Some(document)
            && let Some(id) = self.id.take()
        {
            self.stale.push(id);
            self.revision = None;
        }
    }
    pub fn has_stale(&self) -> bool {
        !self.stale.is_empty()
    }
    pub fn wait(&self, document: u64, revision: u64) -> Option<Duration> {
        if self.alive && self.ready && !self.busy && !self.stale.is_empty() {
            return Some(Duration::from_millis(100));
        }
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
    /// Remove the current document's copy now, waiting for the worker.
    pub fn purge(&mut self) -> Result<(), String> {
        let Some(id) = self.id.clone() else { return Ok(()) };
        self.discard_now(id)?;
        self.id = None;
        self.revision = None;
        self.gap = None;
        Ok(())
    }
    /// Remove the copies of replaced saved documents ([`Self::replaced_clean`]) now, before the
    /// app exits. A copy that can't be removed stays (it is never lost).
    pub fn purge_stale(&mut self) {
        while let Some(id) = self.stale.pop() {
            if let Err(e) = self.discard_now(id) {
                log::warn!("recovery: cannot discard an out-of-date copy: {e}");
            }
        }
    }
    /// Discard the copy `id`, waiting for the job under way and then for the discard.
    fn discard_now(&mut self, id: String) -> Result<(), String> {
        if self.busy {
            let e = self.rx.recv().map_err(|e| e.to_string())?;
            self.documents = e.documents;
            self.busy = false;
            self.ready = true;
            self.saving_for = None;
            if matches!(e.outcome, Ok(Outcome::Loaded(..))) {
                return Err("finish recovery before protecting this document".into());
            }
        }
        self.queue(Job::Discard(id))?;
        let e = self.rx.recv().map_err(|e| e.to_string())?;
        self.busy = false;
        self.documents = e.documents;
        e.outcome.map(|_| ())
    }
}

/// Let the worker finish the job under way and release its record locks before the session is
/// gone: a snapshot being written at exit completes rather than being cut off, and a session
/// started next (the next launch, or a test) can lock and list that copy.
impl Drop for Recovery {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            log::warn!("recovery: the worker stopped abnormally");
        }
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
        Snapshot::new(
            RecoveryInfo { id: id.into(), name: "Synthetic draft".into(), saved_at: crate::cmd::now_iso(), omitted: Vec::new() },
            Document::from_text(text),
        )
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
        store.save(snapshot(&id, "First valid copy"), Limits::default()).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        // Simulate a final rename failure without relying on permissions (root can bypass them).
        let obstruction = t.0.join(&id).join("snapshot-00000000000000000002.json");
        fs::create_dir(&obstruction).unwrap();
        assert!(store.save(snapshot(&id, "Failed copy"), Limits::default()).is_err());
        assert_eq!(fs::read(&first).unwrap(), bytes);
        fs::remove_dir(obstruction).unwrap();
        store.save(snapshot(&id, "Second valid copy"), Limits::default()).unwrap();
        let second = store.entries[&id].path.clone();
        fs::write(second, b"{truncated").unwrap();
        drop(store);
        let mut reopened = reopen_reporting_unreadable(&t.0);
        assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "First valid copy");
        // Never reuse the corrupt generation's name or prune the valid fallback.
        reopened.save(snapshot(&id, "Third valid copy"), Limits::default()).unwrap();
        assert_eq!(fs::read(first).unwrap(), bytes);
        assert_eq!(Store::files(&t.0.join(&id)).unwrap().len(), 2);
    }

    #[test]
    fn size_limit_and_lossy_values_fail_without_committing() {
        let mut out = Limited { inner: Vec::new(), remaining: 3 };
        assert!(out.write_all(b"four").is_err());
        assert!(out.inner.is_empty());
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "valid"), Limits::default()).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        let mut invalid = snapshot(&id, "invalid");
        invalid.document.last_section.page_w = f32::NAN;
        assert!(store.save(invalid, Limits::default()).is_err());
        assert_eq!(fs::read(first).unwrap(), bytes);
        assert_eq!(Store::files(&t.0.join(id)).unwrap().len(), 1);
    }

    /// Version 1 copies kept pictures inside the JSON: as base64 (#2101) or, before that, as
    /// arrays of numbers. Both are still listed and restored, bytes intact, and the next copy of
    /// the restored document moves the picture to a part file.
    #[test]
    fn version_1_copies_with_inline_pictures_still_restore() {
        for numbers in [false, true] {
            let t = Temp::new();
            let id = new_id().unwrap();
            let mut s = snapshot(&id, "Synthetic draft with a picture");
            (s.version, s.generation) = (1, 1);
            s.document.media.insert("media/image1.png".into(), Arc::new(vec![0, 1, 127, 128, 254, 255]));
            s.document.passthrough.insert("word/embeddings/oleObject1.bin".into(), Arc::new(vec![9; 40]));
            let mut value = serde_json::to_value(&s).unwrap();
            assert_eq!(value["version"], 1);
            assert_eq!(value["document"]["media"]["media/image1.png"], "AAF/gP7/");
            assert!(value.get("parts").is_none());
            if numbers {
                value["document"]["media"]["media/image1.png"] = json!([0, 1, 127, 128, 254, 255]);
            }
            let dir = t.0.join(&id);
            private_dir(&dir).unwrap();
            fs::write(dir.join("snapshot-00000000000000000001.json"), serde_json::to_vec(&value).unwrap()).unwrap();
            let mut store = Store::new(t.0.clone()).unwrap();
            store.scan().unwrap();
            assert_eq!(store.inventory().len(), 1);
            let loaded = store.load(&id).unwrap();
            assert_eq!(loaded.document, s.document, "numbers: {numbers}");
            store.save(loaded, Limits::default()).unwrap();
            assert_eq!(part_files(&dir).len(), 2);
            let newest = read_snapshot(&Store::files(&dir).unwrap()[0]).unwrap();
            assert_eq!(newest.version, VERSION);
            assert!(newest.document.media.is_empty() && newest.document.passthrough.is_empty());
            assert_eq!(store.load(&id).unwrap().document, s.document);
            drop(store);
        }
    }

    #[test]
    fn failed_pruning_cannot_push_a_record_beyond_the_discovery_limit() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "Last valid copy"), Limits::default()).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        // A failed cleanup must not allow the next write to exceed the reader's bound.
        for n in 2..=MAX_FILES {
            fs::write(t.0.join(&id).join(format!("snapshot-{n:020}.json")), b"{truncated").unwrap();
        }
        assert!(store.save(snapshot(&id, "New copy"), Limits::default()).is_err());
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
        assert!(store.save(invalid, Limits::default()).is_err());
        assert!(store.directories().unwrap().is_empty());
    }

    #[test]
    fn discovery_reclaims_a_crashed_first_write_but_keeps_valid_records() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let empty = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "Valid synthetic draft"), Limits::default()).unwrap();
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
        owner.save(snapshot(&id, "live"), Limits::default()).unwrap();
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

    /// `private_dir` sets the mode only on folders it creates: an existing recovery folder (and
    /// record folder) that others could read is made private before a copy is written into it.
    #[cfg(unix)]
    #[test]
    fn an_existing_recovery_folder_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let t = Temp::new();
        let root = t.0.join("recovery");
        let id = new_id().unwrap();
        fs::create_dir_all(root.join(&id)).unwrap();
        for dir in [&root, &root.join(&id)] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut store = Store::new(root.clone()).unwrap();
        assert_eq!(mode(&root), 0o700);
        store.save(snapshot(&id, "Private synthetic draft"), Limits::default()).unwrap();
        assert_eq!(mode(&root.join(&id)), 0o700);
        // Every file in a record, the lock included, is the owner's only.
        let files: Vec<PathBuf> = fs::read_dir(root.join(&id)).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 2, "{files:?}");
        for f in files {
            assert_eq!(mode(&f), 0o600, "{}", f.display());
        }
    }

    /// One record failing a per-record check (here, more generations than discovery reads) is
    /// skipped and reported; it used to end the scan, so no copy at all was offered.
    #[test]
    fn one_unreadable_record_does_not_hide_the_others() {
        let t = Temp::new();
        let (good, bad) = (new_id().unwrap(), new_id().unwrap());
        // The same store scans, holding the good record's lock: no other process can be holding
        // it for a moment (#1849).
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&good, "Valid synthetic draft"), Limits::default()).unwrap();
        private_dir(&t.0.join(&bad)).unwrap();
        for n in 1..=MAX_FILES + 1 {
            fs::write(t.0.join(&bad).join(format!("snapshot-{n:020}.json")), b"{truncated").unwrap();
        }
        store.entries.clear();
        assert!(store.scan().is_err(), "the bad record is reported");
        let listed: Vec<String> = store.inventory().into_iter().map(|i| i.id).collect();
        assert_eq!(listed, vec![good.clone()]);
        assert_eq!(store.load(&good).unwrap().document.plain_text(StoryRef::Body), "Valid synthetic draft");
        assert!(t.0.join(&bad).exists(), "the bad record is left alone");
    }

    /// A document saved and then replaced before a tick saw it saved (Save, then New or Open at
    /// once) left its copy behind, offered as unsaved work at every later start; so did Don't Save.
    #[test]
    fn copies_of_saved_or_abandoned_documents_do_not_outlive_them() {
        let t = Temp::new();
        let root = t.0.join("profile");
        let mut s = session(&root);
        s.run("text.insert", &json!({"text": "Saved synthetic draft"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
        s.run("file.save", &json!({"path": t.0.join("saved.docx")})).unwrap();
        s.run("file.new", &json!({})).unwrap();
        assert!(s.recovery_wait().is_some(), "the out-of-date copy schedules its own removal");
        save_now(&mut s);
        settle(&mut s);
        assert!(s.recovery_documents().is_empty());
        assert!(fs::read_dir(&root).unwrap().next().is_none(), "the out-of-date copy is gone from disk");

        // Saved, then the app closes before a tick: the copy goes before exit.
        s.run("text.insert", &json!({"text": "Saved before closing"})).unwrap();
        save_now(&mut s);
        s.run("file.save", &json!({"path": t.0.join("closing.docx")})).unwrap();
        s.discard_recovery();
        assert!(fs::read_dir(&root).unwrap().next().is_none());

        // Don't Save: the abandoned document's copy goes at once, without waiting for a tick.
        s.run("file.new", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "Abandoned synthetic draft"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
        s.discard_recovery();
        assert!(fs::read_dir(&root).unwrap().next().is_none());

        // An unsaved document replaced without a choice (an agent opening a file) keeps its copy.
        s.run("text.insert", &json!({"text": "Only copy of this work"})).unwrap();
        save_now(&mut s);
        s.run("file.new", &json!({})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
        drop(s);
        let s = session(&root);
        assert_eq!(s.recovery_documents().len(), 1, "offered at the next start");
    }

    /// A slow snapshot spaces out the next, at most [`MAX_INTERVAL`] later, so recovery stays a
    /// small share of a core on a large document; a quick one keeps the ten-second interval.
    #[test]
    fn slow_snapshots_space_out_the_next() {
        let t = Temp::new();
        let mut s = session(&t.0);
        let r = s.recovery.as_mut().unwrap();
        // Stand in for the worker's report.
        let (events, rx) = mpsc::channel();
        let worker_events = std::mem::replace(&mut r.rx, rx);
        for (took, least, most) in [
            (Duration::from_millis(20), INTERVAL - Duration::from_secs(1), INTERVAL),
            (Duration::from_secs(3), Duration::from_secs(59), Duration::from_secs(60)),
            (Duration::from_secs(100), MAX_INTERVAL - Duration::from_secs(1), MAX_INTERVAL),
        ] {
            r.due = Instant::now() + INTERVAL;
            r.busy = true;
            events
                .send(Event { documents: Vec::new(), outcome: Ok(Outcome::Saved(Vec::new())), saved_in: Some(took), until_changed: false })
                .unwrap();
            assert!(r.poll().is_none());
            let wait = r.due.saturating_duration_since(Instant::now());
            assert!(wait > least && wait <= most, "{took:?}: next in {wait:?}");
        }
        drop(worker_events);
    }

    /// Ending a session lets the worker finish the snapshot under way and release its lock, so a
    /// session started next lists the copy. It used to be cut off or still locked, and missed.
    #[test]
    fn ending_a_session_finishes_the_snapshot_under_way() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Written as the session ends"})).unwrap();
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        assert!(s.recovery_busy());
        drop(s);
        let records: Vec<PathBuf> = fs::read_dir(&t.0).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(records.len(), 1);
        assert_eq!(Store::files(&records[0]).unwrap().len(), 1, "the snapshot was finished");
        let s = session(&t.0);
        assert_eq!(s.recovery_documents().len(), 1);
    }

    /// Measures what recovery costs on a large synthetic document: copy size, `.json` size, the
    /// worker's time per snapshot and its share of a core while typing. Run on demand:
    /// `cargo test --release -p wordcraft-engine recovery_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement"]
    fn recovery_cost_on_a_large_document() {
        let para = "Synthetic recovery measurement text with several ordinary words in it. ".repeat(8);
        let text: Vec<String> = (0..6000).map(|i| format!("{i} {para}")).collect();
        let mut doc = Document::from_text(&text.join("\n"));
        // Incompressible bytes, as a photograph's are.
        let mut noise = vec![0u8; 27 << 20];
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for b in noise.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        doc.media.insert("media/image1.png".into(), Arc::new(noise));
        let t0 = Instant::now();
        let saved = crate::io::save_bytes("doc.json", &doc).unwrap();
        println!(".json file: {} MB, written in {:?}", saved.len() >> 20, t0.elapsed());
        let t0 = Instant::now();
        let reopened = crate::io::open_bytes("doc.json", &saved).unwrap();
        println!(".json file: opened in {:?}", t0.elapsed());
        assert_eq!(reopened.media, doc.media);
        drop((saved, reopened));
        let t = Temp::new();
        let mut s = session(&t.0);
        s.set_document(doc);
        s.run("text.insert", &json!({"text": "edit"})).unwrap();
        let t0 = Instant::now();
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        println!("UI thread: recovery_tick that queues a snapshot {:?}", t0.elapsed());
        settle(&mut s);
        println!("worker: first snapshot written in {:?} (error: {:?})", t0.elapsed(), s.recovery_error());
        let size: u64 = Store::files(&t.0.join(&s.recovery_documents()[0].id)).unwrap().iter().map(|p| fs::metadata(p).unwrap().len()).sum();
        println!("recovery copy: {} MB", size >> 20);
        // Two minutes of typing, a character every 100 ms, ticking as the app's frames do.
        let (window, mut busy_since, mut busy, mut snapshots) = (Duration::from_secs(120), None::<Instant>, Duration::ZERO, 0);
        let (start, mut typed) = (Instant::now(), Instant::now());
        while start.elapsed() < window {
            if typed.elapsed() >= Duration::from_millis(100) {
                s.run("text.insert", &json!({"text": "x"})).unwrap();
                typed = Instant::now();
            }
            s.recovery_tick();
            match (busy_since, s.recovery_busy()) {
                (None, true) => {
                    busy_since = Some(Instant::now());
                    snapshots += 1;
                }
                (Some(since), false) => {
                    busy += since.elapsed();
                    busy_since = None;
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if let Some(since) = busy_since {
            busy += since.elapsed();
        }
        let share = busy.as_secs_f64() * 100.0 / start.elapsed().as_secs_f64();
        println!("typing for {window:?}: {snapshots} snapshots, worker busy {busy:?} = {share:.1}% of a core");
        settle(&mut s);
        drop(s);
        let t0 = Instant::now();
        let s = session(&t.0);
        println!("start-up scan listing {} copies: {:?}", s.recovery_documents().len(), t0.elapsed());
    }

    // ---- Parts stored once per record, as files (#2624), and failures not retried (#2625) ----

    /// Incompressible bytes, as a photograph's are, cheap to make: one xorshift step per 8 bytes.
    fn picture(seed: u64, len: usize) -> Arc<Vec<u8>> {
        let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut bytes = vec![0u8; len];
        for chunk in bytes.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes()[..chunk.len()]);
        }
        Arc::new(bytes)
    }
    fn with_pictures(text: &str, pictures: &[(&str, Arc<Vec<u8>>)]) -> Snapshot {
        let mut s = snapshot(&new_id().unwrap(), text);
        for (name, bytes) in pictures {
            s.document.media.insert((*name).into(), bytes.clone());
        }
        s
    }
    fn part_files(dir: &Path) -> Vec<String> {
        Store::parts(dir).unwrap().into_keys().collect()
    }
    /// The only record folder under `root`.
    fn record(root: &Path) -> PathBuf {
        let records: Vec<PathBuf> = fs::read_dir(root).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(records.len(), 1, "{records:?}");
        records[0].clone()
    }
    /// Bytes of the files under `dir` written at or after `since`: a file written again (a part
    /// rewritten through a temporary file and a rename, too) has a new modification time.
    fn bytes_written_since(dir: &Path, since: std::time::SystemTime) -> u64 {
        let mut total = 0;
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                total += bytes_written_since(&e.path(), since);
            } else if meta.modified().unwrap() >= since {
                total += meta.len();
            }
        }
        total
    }
    /// A moment from which later writes are told apart from earlier ones: file times come from a
    /// coarse clock (a few milliseconds behind), so wait that out on both sides.
    fn mark() -> std::time::SystemTime {
        std::thread::sleep(Duration::from_millis(30));
        let now = std::time::SystemTime::now();
        std::thread::sleep(Duration::from_millis(30));
        now
    }

    /// Each picture and kept part is a file named by its SHA-256, written once and shared by
    /// later generations; the JSON names it by hash and length. A text edit writes only the JSON,
    /// and a part no kept generation uses is removed once the next generation is committed.
    #[test]
    fn parts_are_written_once_and_pruned_after_commit() {
        let t = Temp::new();
        let mut store = Store::new(t.0.clone()).unwrap();
        let big = picture(1, 300_000);
        let small = picture(2, 1000);
        let ole = picture(9, 500);
        let make = |text: &str, id: &str| {
            let mut s = snapshot(id, text);
            for (name, bytes) in [("media/big.png", &big), ("media/small.png", &small), ("media/copy.png", &small)] {
                s.document.media.insert(name.into(), bytes.clone());
            }
            s.document.passthrough.insert("word/embeddings/oleObject1.bin".into(), ole.clone());
            s
        };
        let id = new_id().unwrap();
        let w0 = store.written;
        assert_eq!(store.save(make("First synthetic text", &id), Limits::default()).unwrap(), Vec::<String>::new());
        let first = store.written - w0;
        let dir = t.0.join(&id);
        let mut expected = vec![hex(&sha256(&big)), hex(&sha256(&small)), hex(&sha256(&ole))];
        expected.sort();
        assert_eq!(part_files(&dir), expected, "the same bytes twice are one file, named by their SHA-256");
        assert!(first >= 301_500, "{first}");
        let main = Store::files(&dir).unwrap()[0].clone();
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&main).unwrap()).unwrap();
        assert_eq!(json["version"], 2);
        assert_eq!(json["parts"]["media"]["media/big.png"], json!({"part": hex(&sha256(&big)), "len": 300_000}));
        assert!(json["document"]["media"].as_object().is_none_or(|m| m.is_empty()), "the JSON doesn't hold the pictures");
        // The pictures alone would be 400 KB of base64.
        let main_len = fs::metadata(&main).unwrap().len();
        assert!(main_len < 100_000, "{main_len}");

        // Text edited, pictures unchanged: only the JSON is written.
        let w1 = store.written;
        store.save(make("Second synthetic text", &id), Limits::default()).unwrap();
        let second = store.written - w1;
        assert!(second < 100_000, "{second} bytes written for a text edit");
        assert_eq!(second, fs::metadata(&Store::files(&dir).unwrap()[0]).unwrap().len(), "nothing but the new generation");
        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.document, make("Second synthetic text", &id).document);

        // A picture removed: its file stays while the retained generation uses it, then goes.
        let only_big = |text: &str| {
            let mut s = snapshot(&id, text);
            s.document.media.insert("media/big.png".into(), big.clone());
            s
        };
        store.save(only_big("Third"), Limits::default()).unwrap();
        assert_eq!(part_files(&dir).len(), 3, "the retained second generation still uses them");
        store.save(only_big("Fourth"), Limits::default()).unwrap();
        assert_eq!(part_files(&dir), vec![hex(&sha256(&big))]);
        assert_eq!(Store::files(&dir).unwrap().len(), 2);
        // A record's parts go with it.
        store.discard(&id).unwrap();
        assert!(!dir.exists());
    }

    /// A document whose pictures are far over the JSON limit gets a copy (with pictures as
    /// arrays of numbers, about 70 MB of them was enough to get none, #2624). Its second snapshot,
    /// after a text edit, writes no picture bytes. Here the limit is lowered so that a few
    /// megabytes show it; `three_hundred_megabytes_of_pictures_get_a_copy` uses the real ones.
    #[test]
    fn a_document_with_pictures_over_the_json_limit_gets_a_copy() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.recovery.as_mut().unwrap().limits.main = 256 << 10;
        let pictures: Vec<Arc<Vec<u8>>> = (0..8).map(|i| picture(i, 512 << 10)).collect();
        for (i, p) in pictures.iter().enumerate() {
            s.doc.media.insert(format!("media/image{i}.png"), p.clone());
        }
        s.run("text.insert", &json!({"text": "Synthetic text with four megabytes of pictures. "})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_error(), None);
        assert_eq!(s.recovery_gap(), None);
        let dir = record(&t.0);
        assert_eq!(part_files(&dir).len(), 8);
        let main = Store::files(&dir).unwrap()[0].clone();
        assert!(fs::metadata(&main).unwrap().len() < 256 << 10);

        let since = mark();
        s.run("text.insert", &json!({"text": "A text edit. "})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_error(), None);
        let written = bytes_written_since(&dir, since);
        let newest = Store::files(&dir).unwrap()[0].clone();
        assert_ne!(newest, main);
        assert_eq!(written, fs::metadata(&newest).unwrap().len(), "only the new generation was written");
        assert!(written < 64 << 10, "{written} bytes written for a text edit");

        let expected = s.doc.clone();
        drop(s);
        let mut s = session(&t.0);
        let id = s.recovery_documents()[0].id.clone();
        s.run("file.recover", &json!({"restore": id})).unwrap();
        settle(&mut s);
        assert_eq!(s.recovery_error(), None);
        assert_eq!(s.doc, expected);
    }

    /// The case from #2456: 300 MB of pictures (ten of 30 MB) with the real limits. The first
    /// snapshot makes a copy; the second, after a text edit, writes no picture bytes; the copy
    /// restores every picture intact. Writes 300 MB and holds 300 MB of pictures (twice while
    /// restoring), so it runs on demand:
    /// `cargo test --release -p wordcraft-engine three_hundred_megabytes -- --ignored --nocapture`.
    #[test]
    #[ignore = "writes 300 MB; the default run checks the same with a lowered limit"]
    fn three_hundred_megabytes_of_pictures_get_a_copy() {
        let t = Temp::new();
        let mut s = session(&t.0);
        let mut hashes = BTreeMap::new();
        for i in 0..10 {
            let p = picture(i, 30 << 20);
            hashes.insert(format!("media/image{i}.png"), sha256(&p));
            s.doc.media.insert(format!("media/image{i}.png"), p);
        }
        s.run("text.insert", &json!({"text": "Synthetic text with 300 MB of pictures. "})).unwrap();
        let t0 = Instant::now();
        save_now(&mut s);
        let first = t0.elapsed();
        assert_eq!(s.recovery_error(), None);
        assert_eq!(s.recovery_gap(), None);
        let dir = record(&t.0);
        let on_disk = bytes_written_since(&dir, std::time::UNIX_EPOCH);
        assert!(on_disk > 300 << 20);

        let since = mark();
        s.run("text.insert", &json!({"text": "A text edit. "})).unwrap();
        let t0 = Instant::now();
        save_now(&mut s);
        let second = t0.elapsed();
        assert_eq!(s.recovery_error(), None);
        let written = bytes_written_since(&dir, since);
        assert!(written < 64 << 10, "{written} bytes written for a text edit");
        println!(
            "300 MB of pictures: first snapshot {first:?}, {} MB on disk; second after a text edit {second:?}, {written} bytes written",
            on_disk >> 20
        );

        let text = s.doc.plain_text(wordcraft_doc::StoryRef::Body);
        drop(s);
        let mut s = session(&t.0);
        let id = s.recovery_documents()[0].id.clone();
        let t0 = Instant::now();
        s.run("file.recover", &json!({"restore": id})).unwrap();
        settle(&mut s);
        println!("restored in {:?}", t0.elapsed());
        assert_eq!(s.recovery_error(), None);
        assert_eq!(s.doc.plain_text(wordcraft_doc::StoryRef::Body), text);
        let restored: BTreeMap<String, [u8; 32]> = s.doc.media.iter().map(|(k, v)| (k.clone(), sha256(v))).collect();
        assert_eq!(restored, hashes);
    }

    /// A part file that is missing, truncated, or altered at the same length is caught on
    /// restore (length, then SHA-256): that picture is left out and named, the text and the
    /// other pictures are restored.
    #[test]
    fn a_missing_or_damaged_part_is_detected_on_restore() {
        let t = Temp::new();
        let mut store = Store::new(t.0.clone()).unwrap();
        let pics = [
            ("media/a.png", picture(1, 2000)),
            ("media/b.png", picture(2, 2000)),
            ("media/c.png", picture(3, 2000)),
            ("media/d.png", picture(4, 2000)),
        ];
        let s = with_pictures("Synthetic text", &pics);
        let id = s.info.id.clone();
        store.save(s, Limits::default()).unwrap();
        let parts = t.0.join(&id).join(PARTS);
        let file = |i: usize| parts.join(hex(&sha256(&pics[i].1)));
        let mut altered = fs::read(file(0)).unwrap();
        altered[1000] ^= 1;
        fs::write(file(0), altered).unwrap();
        fs::write(file(1), &pics[1].1[..1999]).unwrap();
        fs::remove_file(file(2)).unwrap();
        drop(store);
        let mut store = Store::new(t.0.clone()).unwrap();
        store.scan().unwrap();
        let loaded = load_retrying(&mut store, &id);
        assert_eq!(loaded.document.plain_text(StoryRef::Body), "Synthetic text");
        assert_eq!(loaded.document.media.keys().collect::<Vec<_>>(), vec!["media/d.png"]);
        assert_eq!(loaded.document.media["media/d.png"], pics[3].1);
        assert_eq!(loaded.info.omitted, vec!["media/a.png", "media/b.png", "media/c.png"]);
        drop(store);

        // Through the session, the restore says what it left out.
        let mut s = session(&t.0);
        s.run("file.recover", &json!({"restore": id})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Synthetic text");
        let error = s.recovery_error().unwrap();
        assert!(error.contains("media/a.png, media/b.png, media/c.png"), "{error}");
    }

    /// Over the budget, the text and structure are still kept, with the pictures that fit
    /// (smallest first); the copy names the ones left out, and nothing too large is written and
    /// then deleted. Parts count toward the folder budget too. Text alone over the budget is
    /// refused before anything is written, and the last copy stays.
    #[test]
    fn over_budget_keeps_the_text_and_names_the_parts_left_out() {
        let t = Temp::new();
        let mut store = Store::new(t.0.clone()).unwrap();
        let limits = Limits { main: 1 << 20, record: 400 << 10, disk: 1 << 30 };
        let s = with_pictures(
            "Synthetic text that must endure",
            &[("media/a.png", picture(1, 50_000)), ("media/b.png", picture(2, 100_000)), ("media/huge.png", picture(3, 4 << 20))],
        );
        let id = s.info.id.clone();
        let omitted = store.save(s, limits).unwrap();
        assert_eq!(omitted, vec!["media/huge.png".to_string()]);
        assert!(store.written < 400 << 10, "{} bytes written: the huge picture was never written", store.written);
        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.document.plain_text(StoryRef::Body), "Synthetic text that must endure");
        assert_eq!(loaded.document.media.len(), 2);
        assert_eq!(loaded.info.omitted, vec!["media/huge.png".to_string()]);
        assert_eq!(store.inventory()[0].omitted, vec!["media/huge.png".to_string()]);

        // The folder budget: another record's parts leave less room for this one.
        let other = with_pictures("Another synthetic draft", &[("media/x.png", picture(7, 300_000))]);
        let folder = Limits { main: 1 << 20, record: 1 << 30, disk: 400 << 10 };
        let omitted = store.save(other, folder).unwrap();
        assert_eq!(omitted, vec!["media/x.png".to_string()], "the first record's 150 KB of parts count");

        // Text alone over the budget: refused before writing anything; the last copy stays.
        let before = fs::read(&store.entries[&id].path).unwrap();
        let w = store.written;
        let tight = Limits { main: 1 << 20, record: 1024, disk: 1 << 30 };
        let failure = store.save(snapshot(&id, &"long text ".repeat(1000)), tight).unwrap_err();
        assert!(failure.until_changed, "{failure:?}");
        assert_eq!(store.written, w);
        assert_eq!(fs::read(&store.entries[&id].path).unwrap(), before);
        // So is JSON over its own limit.
        let failure = store.save(snapshot(&id, &"long text ".repeat(1000)), Limits { main: 1024, ..Limits::default() }).unwrap_err();
        assert!(failure.until_changed, "{failure:?}");
        assert_eq!(store.written, w);
    }

    /// A failure only a change can clear (a size limit, a budget, a value the format can't keep)
    /// is told apart from one that may clear by itself (here, the generation's name taken).
    #[test]
    fn size_failures_are_told_apart_from_passing_ones() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "First valid copy"), Limits::default()).unwrap();
        let mut invalid = snapshot(&id, "invalid");
        invalid.document.last_section.page_w = f32::NAN;
        assert!(store.save(invalid, Limits::default()).unwrap_err().until_changed);
        let obstruction = t.0.join(&id).join("snapshot-00000000000000000002.json");
        fs::create_dir(&obstruction).unwrap();
        assert!(!store.save(snapshot(&id, "Blocked copy"), Limits::default()).unwrap_err().until_changed);
        fs::remove_dir(obstruction).unwrap();
    }

    /// #2625: a snapshot that fails on a size limit isn't tried again at every interval, writing
    /// as much again each time, but only once the document changes, and then still paced (#2102).
    #[test]
    fn a_snapshot_over_a_size_limit_is_not_retried_until_the_document_changes() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.recovery.as_mut().unwrap().limits.main = 512;
        s.run("text.insert", &json!({"text": "Synthetic text too long for a 512-byte recovery copy"})).unwrap();
        save_now(&mut s);
        let error = s.recovery_error().unwrap().to_string();
        assert!(error.contains("too large"), "{error}");
        assert!(matches!(s.recovery_gap(), Some(crate::recovery::RecoveryGap::Failed(_))));
        assert!(fs::read_dir(&t.0).unwrap().next().is_none(), "nothing left behind");
        // Unchanged: no new attempt, however long it waits, and no timer to wake the app for it.
        for _ in 0..5 {
            s.recovery.as_mut().unwrap().due = Instant::now();
            s.recovery_tick();
            assert!(!s.recovery_busy(), "an unchanged document is not tried again");
        }
        assert!(s.recovery_wait().is_none());
        // Changed: tried again at the next interval.
        s.run("text.insert", &json!({"text": " more"})).unwrap();
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        assert!(s.recovery_busy(), "a changed document is tried again");
        settle(&mut s);
        assert!(s.recovery_error().is_some());
        // The failed attempt still paces the next (#2102): a change straight after it waits.
        s.run("text.insert", &json!({"text": " again"})).unwrap();
        s.recovery_tick();
        assert!(!s.recovery_busy(), "still paced after the failure");
        assert!(s.recovery_wait().is_some_and(|w| w > Duration::from_secs(1)));
        // A budget large enough again: the next change gets its copy and the warning goes.
        s.recovery.as_mut().unwrap().limits = Limits::default();
        s.run("text.insert", &json!({"text": " fits now"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_error(), None);
        assert_eq!(s.recovery_gap(), None);
        assert_eq!(s.recovery_documents().len(), 1);
    }

    /// The session reports what its copy doesn't cover, for the status bar, until a copy covers
    /// it all or the document is saved.
    #[test]
    fn the_session_reports_what_recovery_does_not_cover() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.set_recovery_record_budget(200 << 10);
        s.run("text.insert", &json!({"text": "Synthetic"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_gap(), None);
        s.doc.media.insert("media/huge.png".into(), picture(5, 1 << 20));
        s.run("text.insert", &json!({"text": " edit"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_gap(), Some(crate::recovery::RecoveryGap::Omitted(vec!["media/huge.png".into()])));
        let listed = s.run("file.recover", &json!({})).unwrap();
        assert_eq!(listed["gap"]["omitted"][0], "media/huge.png");
        assert_eq!(listed["documents"][0]["omitted"][0], "media/huge.png");
        // Saved: the file keeps it all, nothing to report.
        s.run("file.save", &json!({"path": t.0.join("saved.json")})).unwrap();
        assert_eq!(s.recovery_gap(), None);
    }

    /// A copy naming a part outside its record, or not by a SHA-256, is unreadable, not followed.
    #[test]
    fn hostile_part_names_are_refused() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let dir = t.0.join(&id);
        private_dir(&dir).unwrap();
        for (n, part) in ["../../outside", "zz", "/etc/passwd"].into_iter().enumerate() {
            let value = json!({
                "version": 2, "generation": n + 1,
                "info": {"id": id, "name": "Synthetic draft", "savedAt": "2026-01-01T00:00:00Z"},
                "document": Document::from_text("x"),
                "parts": {"media": {"a.png": {"part": part, "len": 1}}},
            });
            fs::write(dir.join(format!("snapshot-{:020}.json", n + 1)), serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let mut store = Store::new(t.0.clone()).unwrap();
        assert!(store.scan().is_err());
        assert!(store.inventory().is_empty());
        let zeros = "0".repeat(64);
        for bad in ["../x", "", &"0".repeat(63), &"A".repeat(64), &format!("{zeros}.bin")] {
            assert!(!part_ok(bad), "{bad}");
        }
        assert!(part_ok(&zeros));
        // A version this build doesn't know is not read as one it does.
        let value = json!({"version": 3, "generation": 9, "info": {"id": id, "name": "x", "savedAt": "x"}, "document": Document::from_text("x")});
        fs::write(dir.join("snapshot-00000000000000000009.json"), serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(read_snapshot(&dir.join("snapshot-00000000000000000009.json")).is_err());
    }
}
