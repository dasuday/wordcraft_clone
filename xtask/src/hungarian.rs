//! Explicit, pinned updates from LibreOffice. Downloads and validates before changing assets.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SOURCE: &str = "https://github.com/LibreOffice/dictionaries";
const FILES: [&str; 3] = ["hu_HU.aff", "hu_HU.dic", "README_hu_HU.txt"];

fn valid_revision(revision: &str) -> bool {
    revision.len() == 40 && revision.bytes().all(|c| c.is_ascii_hexdigit())
}

fn license_supported(readme: &[u8]) -> bool {
    // Fail closed if upstream changes its licensing statement. A maintainer must review it.
    std::str::from_utf8(readme).is_ok_and(|s| s.contains("MPLv2 vagy LesserGPLv3+") && s.contains("see COPYING.MPL"))
}

/// Download, validate and publish one explicitly selected upstream revision.
pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let [revision] = args else { return Err("usage: cargo xtask update-hungarian <40-digit LibreOffice commit>".into()) };
    if !valid_revision(revision) {
        return Err("expected a full 40-digit hexadecimal commit, not a branch or URL".into());
    }
    let work = root.join(".work");
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    // An OS lock is released even if the process is killed. Never unlink the lock file:
    // another process may already have opened it and be waiting on that inode.
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(work.join("hungarian-update.lock")).map_err(|e| e.to_string())?;
    lock.try_lock().map_err(|e| format!("another Hungarian update is running: {e}"))?;
    recover(root)?;
    let stage = Stage::new(&work)?;
    let mut downloaded = Vec::new();
    for name in FILES {
        let url = format!("https://raw.githubusercontent.com/LibreOffice/dictionaries/{revision}/hu_HU/{name}");
        let output = Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--max-time",
                "90",
                "--max-filesize",
                "16777216",
                &url,
            ])
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if !output.status.success() {
            return Err(format!("download {name}: {}", String::from_utf8_lossy(&output.stderr)));
        }
        downloaded.push((name, output.stdout));
    }
    let get = |name: &str| -> Result<&[u8], String> {
        downloaded.iter().find(|(n, _)| *n == name).map(|(_, b)| b.as_slice()).ok_or_else(|| format!("missing {name}"))
    };
    if !license_supported(get("README_hu_HU.txt")?) {
        return Err("upstream license statement changed; review licensing before importing".into());
    }
    let dictionary = wordcraft_proof::hu::parse_dictionary(get("hu_HU.aff")?, get("hu_HU.dic")?)?;
    for word in ["magyar", "helyesírás", "árvíztűrő", "tükörfúrógép", "házainkban", "könyvespolc"] {
        if !dictionary.check(word) {
            return Err(format!("candidate dictionary rejects regression word: {word}"));
        }
    }
    for word in ["hejesírás", "muszály", "árvíztürő"] {
        if dictionary.check(word) {
            return Err(format!("candidate dictionary accepts regression misspelling: {word}"));
        }
    }
    let hashes: serde_json::Map<String, serde_json::Value> = downloaded
        .iter()
        .map(|(n, b)| ((*n).to_string(), Sha256::digest(b).iter().map(|byte| format!("{byte:02x}")).collect::<String>().into()))
        .collect();
    let metadata =
        serde_json::json!({"repository": SOURCE, "revision": revision, "path": "hu_HU", "license_used": "MPL-2.0", "files_sha256": hashes});
    for (name, bytes) in &downloaded {
        fs::write(stage.0.join(name), bytes).map_err(|e| e.to_string())?;
    }
    let metadata = serde_json::to_string_pretty(&metadata).map_err(|e| e.to_string())? + "\n";
    fs::write(stage.0.join("SOURCE.json"), metadata).map_err(|e| e.to_string())?;
    fs::copy(root.join("assets/spelling/hu/COPYING.MPL"), stage.0.join("COPYING.MPL")).map_err(|e| e.to_string())?;
    publish(root, &stage.0)?;
    println!("Hungarian dictionary updated from {SOURCE}/tree/{revision}/hu_HU (MPL-2.0). Run proofing tests and review the diff before committing.");
    Ok(())
}

/// Private staging prevents parallel invocations from sharing downloaded bytes.
struct Stage(PathBuf);
impl Stage {
    fn new(work: &Path) -> Result<Self, String> {
        for attempt in 0..1000 {
            let path = work.join(format!("hungarian-update-{}-{attempt}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("too many stale Hungarian update staging directories".into())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Check actual on-disk bytes, not just the buffers received from curl.
fn verify(directory: &Path) -> Result<(), String> {
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("SOURCE.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    for name in FILES {
        let bytes = fs::read(directory.join(name)).map_err(|e| e.to_string())?;
        let actual = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
        if metadata["files_sha256"][name].as_str() != Some(actual.as_str()) {
            return Err(format!("published hash mismatch: {name}"));
        }
    }
    if !directory.join("COPYING.MPL").is_file() {
        return Err("missing MPL license".into());
    }
    Ok(())
}

/// A remaining backup marks an interrupted publication. Restore the whole old set.
/// While the directory is absent, builds fail rather than embedding mixed revisions.
fn recover(root: &Path) -> Result<(), String> {
    let backup = root.join(".work/hungarian-backup");
    if !backup.exists() {
        return Ok(());
    }
    let destination = root.join("assets/spelling/hu");
    if destination.exists() {
        let quarantine = Stage::new(&root.join(".work"))?;
        fs::rename(&destination, quarantine.0.join("interrupted")).map_err(|e| e.to_string())?;
    }
    fs::rename(&backup, &destination).map_err(|e| format!("restore Hungarian backup: {e}"))?;
    Ok(())
}

/// Publish by directory renames, preserving the previous complete set until verification.
/// Caller holds the update lock. No file-by-file copies touch the live directory.
fn publish(root: &Path, stage: &Path) -> Result<(), String> {
    verify(stage)?;
    let destination = root.join("assets/spelling/hu");
    let backup = root.join(".work/hungarian-backup");
    if backup.exists() {
        return Err("recover the interrupted Hungarian update first".into());
    }
    fs::rename(&destination, &backup).map_err(|e| format!("back up Hungarian dictionary: {e}"))?;
    let result = fs::rename(stage, &destination).map_err(|e| e.to_string()).and_then(|()| verify(&destination));
    if let Err(e) = result {
        recover(root).map_err(|recovery| format!("{e}; recovery failed: {recovery}"))?;
        return Err(e);
    }
    // Rename the backup into private staging before deleting it. If deletion is interrupted,
    // recovery must not restore a partly deleted backup on the next invocation.
    let retired = Stage::new(&root.join(".work"))?;
    fs::rename(&backup, retired.0.join("previous")).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(path: &Path, marker: &str) {
        fs::create_dir_all(path).unwrap();
        let mut hashes = serde_json::Map::new();
        for name in FILES {
            fs::write(path.join(name), marker).unwrap();
            hashes.insert(name.into(), serde_json::Value::String(Sha256::digest(marker.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()));
        }
        fs::write(path.join("SOURCE.json"), serde_json::to_vec(&serde_json::json!({"files_sha256": hashes})).unwrap()).unwrap();
        fs::write(path.join("COPYING.MPL"), "test license placeholder").unwrap();
    }

    #[test]
    fn updates_publish_complete_sets_and_recover_interrupted_renames() {
        let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.work");
        fs::create_dir_all(&work).unwrap();
        let root = Stage::new(&work).unwrap();
        fs::create_dir(root.0.join(".work")).unwrap();
        let destination = root.0.join("assets/spelling/hu");
        let backup = root.0.join(".work/hungarian-backup");
        fixture(&destination, "old");
        let candidate = Stage::new(&root.0.join(".work")).unwrap();
        fixture(&candidate.0, "new");
        fs::write(candidate.0.join("hu_HU.dic"), "corrupt").unwrap();
        assert!(publish(&root.0, &candidate.0).is_err());
        assert_eq!(fs::read_to_string(destination.join("hu_HU.dic")).unwrap(), "old");
        fixture(&candidate.0, "new");
        publish(&root.0, &candidate.0).unwrap();
        verify(&destination).unwrap();
        assert_eq!(fs::read_to_string(destination.join("hu_HU.dic")).unwrap(), "new");
        assert!(!backup.exists());
        // Process killed after moving the old directory, before publishing the candidate.
        fs::rename(&destination, &backup).unwrap();
        recover(&root.0).unwrap();
        verify(&destination).unwrap();
        // Process killed after publishing, before verification/retiring the backup.
        fs::rename(&destination, &backup).unwrap();
        fixture(&destination, "interrupted");
        recover(&root.0).unwrap();
        verify(&destination).unwrap();
        assert_eq!(fs::read_to_string(destination.join("hu_HU.dic")).unwrap(), "new");
    }

    #[test]
    fn concurrent_updates_cannot_acquire_the_same_lock() {
        let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.work");
        fs::create_dir_all(&work).unwrap();
        let stage = Stage::new(&work).unwrap();
        let open = || fs::OpenOptions::new().create(true).truncate(false).write(true).open(stage.0.join("lock")).unwrap();
        let first = open();
        first.try_lock().unwrap();
        let second = open();
        assert!(second.try_lock().is_err());
        drop(first);
        second.try_lock().unwrap();
    }

    #[test]
    fn update_requires_immutable_revision_and_known_license() {
        assert!(valid_revision("74b6ae2a2df22fd9d6e253f0391ac3a67cb7b3f8"));
        for rev in ["master", "HEAD", "../hu_HU", "", "gggggggggggggggggggggggggggggggggggggggg"] {
            assert!(!valid_revision(rev));
        }
        assert!(license_supported(include_bytes!("../../assets/spelling/hu/README_hu_HU.txt")));
        assert!(!license_supported(b"All rights reserved"));
    }
}
