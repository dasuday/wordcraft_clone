//! Explicit, pinned updates from LibreOffice. Downloads and validates before changing assets.
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command};

const SOURCE: &str = "https://github.com/LibreOffice/dictionaries";
const FILES: [&str; 3] = ["hu_HU.aff", "hu_HU.dic", "README_hu_HU.txt"];

fn valid_revision(revision: &str) -> bool {
    revision.len() == 40 && revision.bytes().all(|c| c.is_ascii_hexdigit())
}

fn license_supported(readme: &[u8]) -> bool {
    // Fail closed if upstream changes its licensing statement. A maintainer must review it.
    std::str::from_utf8(readme).is_ok_and(|s| s.contains("MPLv2 vagy LesserGPLv3+") && s.contains("see COPYING.MPL"))
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let [revision] = args else { return Err("usage: cargo xtask update-hungarian <40-digit LibreOffice commit>".into()) };
    if !valid_revision(revision) {
        return Err("expected a full 40-digit hexadecimal commit, not a branch or URL".into());
    }
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
    let stage = root.join(".work/hungarian-update");
    let destination = root.join("assets/spelling/hu");
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
    for (name, bytes) in &downloaded {
        std::fs::write(stage.join(name), bytes).map_err(|e| e.to_string())?;
    }
    let metadata = serde_json::to_string_pretty(&metadata).map_err(|e| e.to_string())? + "\n";
    std::fs::write(stage.join("SOURCE.json"), metadata).map_err(|e| e.to_string())?;
    // All network, license, parser and language checks have passed. Preserve the raw source
    // files (including authors and notices); the MPL text already shipped here stays intact.
    for name in FILES.into_iter().chain(["SOURCE.json"]) {
        std::fs::copy(stage.join(name), destination.join(name)).map_err(|e| format!("install {name}: {e}"))?;
    }
    println!("Hungarian dictionary updated from {SOURCE}/tree/{revision}/hu_HU (MPL-2.0). Run proofing tests and review the diff before committing.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
