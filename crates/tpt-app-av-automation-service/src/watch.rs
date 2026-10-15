//! Watched-folder polling for `file_arrived` triggers (spec §7.5).
//!
//! The poller is deliberately plain `std::fs` so the service does not depend on a platform file
//! notification backend. Two rules keep it honest:
//!
//! * the first scan of a folder only records what is already there, so starting the service never
//!   fires for old files;
//! * a new file is reported only once its size has been the same on two consecutive scans, so a
//!   file still being copied into the folder is not handed to the rule half-written.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

/// What one folder looked like on the previous scan.
#[derive(Debug, Default)]
struct FolderState {
    /// Whether the baseline scan has happened.
    primed: bool,
    /// Files already reported, or present at the baseline.
    seen: HashSet<PathBuf>,
    /// New files seen once, with the size they had then.
    candidates: HashMap<PathBuf, u64>,
}

/// Remembers which files each watched folder already contained.
#[derive(Debug, Default)]
pub(crate) struct FolderWatch {
    folders: BTreeMap<String, FolderState>,
}

/// A file that has arrived and stopped growing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Arrival {
    /// The folder as the rule names it.
    pub folder: String,
    /// Full path of the file.
    pub path: String,
}

impl FolderWatch {
    /// Scans every folder in `folders` and returns the files that have newly arrived.
    ///
    /// Folders no longer named by any rule are forgotten, so a later rule naming them starts from a
    /// fresh baseline. A folder that cannot be read is skipped for this scan.
    pub(crate) fn poll(&mut self, folders: &BTreeSet<String>) -> Vec<Arrival> {
        self.folders.retain(|name, _| folders.contains(name));
        let mut arrivals = Vec::new();
        for folder in folders {
            let state = self.folders.entry(folder.clone()).or_default();
            let Ok(entries) = std::fs::read_dir(folder) else {
                continue;
            };
            let mut present = HashSet::new();
            for entry in entries.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if !metadata.is_file() {
                    continue;
                }
                let path = entry.path();
                present.insert(path.clone());
                if !state.primed || state.seen.contains(&path) {
                    if !state.primed {
                        state.seen.insert(path);
                    }
                    continue;
                }
                let size = metadata.len();
                if state.candidates.get(&path) == Some(&size) {
                    state.candidates.remove(&path);
                    state.seen.insert(path.clone());
                    arrivals.push(Arrival {
                        folder: folder.clone(),
                        path: path.to_string_lossy().into_owned(),
                    });
                } else {
                    state.candidates.insert(path, size);
                }
            }
            state.primed = true;
            state.candidates.retain(|path, _| present.contains(path));
            state.seen.retain(|path| present.contains(path));
        }
        arrivals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders(name: &str) -> BTreeSet<String> {
        std::iter::once(name.to_owned()).collect()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tpt-watch-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn files_present_at_the_baseline_never_arrive() {
        let dir = scratch("baseline");
        std::fs::write(dir.join("old.mov"), b"old").unwrap();
        let name = dir.to_string_lossy().into_owned();
        let mut watch = FolderWatch::default();
        assert!(watch.poll(&folders(&name)).is_empty());
        assert!(watch.poll(&folders(&name)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_file_arrives_once_its_size_is_stable() {
        let dir = scratch("stable");
        let name = dir.to_string_lossy().into_owned();
        let mut watch = FolderWatch::default();
        watch.poll(&folders(&name));

        std::fs::write(dir.join("clip.mov"), b"partial").unwrap();
        // First sighting: the size is recorded but not yet trusted.
        assert!(watch.poll(&folders(&name)).is_empty());
        // Still growing: the size changed, so it is still not trusted.
        std::fs::write(dir.join("clip.mov"), b"partial and more").unwrap();
        assert!(watch.poll(&folders(&name)).is_empty());
        // Stable across two scans: it arrives, exactly once.
        let arrivals = watch.poll(&folders(&name));
        assert_eq!(arrivals.len(), 1);
        assert_eq!(arrivals[0].folder, name);
        assert!(arrivals[0].path.ends_with("clip.mov"));
        assert!(watch.poll(&folders(&name)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_folder_is_skipped_quietly() {
        let mut watch = FolderWatch::default();
        assert!(watch
            .poll(&folders("/definitely/not/a/real/folder/xyz"))
            .is_empty());
    }
}
