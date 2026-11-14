// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use nexo_core::Report;
use serde::{Deserialize, Serialize};

/// What a number of syncs did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Counts {
    #[serde(default)]
    pub syncs: u64,
    #[serde(default)]
    pub received: u64,
    #[serde(default)]
    pub sent: u64,
    #[serde(default)]
    pub problems: u64,
}

impl Counts {
    fn add(&mut self, report: &Report) {
        self.syncs += 1;
        self.received += report.received as u64;
        self.sent += report.sent as u64;
        self.problems += report.problems.len() as u64;
    }

    pub fn values(&self) -> [u64; 4] {
        [self.syncs, self.received, self.sent, self.problems]
    }
}

/// The counts as the file keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    since: DateTime<Utc>,
    #[serde(flatten)]
    counts: Counts,
}

/// The counts of the syncs: of this run of nexo, and of all runs, which are in a file.
#[derive(Debug)]
pub struct Usage {
    path: PathBuf,
    since: DateTime<Utc>,
    pub total: Counts,
    pub session: Counts,
}

impl Usage {
    /// Read the file. A file that is not there, or that nexo cannot read, starts the counts
    /// again: the counts are not worth a stop of the app.
    pub fn load(path: &Path) -> Self {
        let saved = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Saved>(&text).ok());
        let (since, total) = match saved {
            Some(saved) => (saved.since, saved.counts),
            None => (Utc::now(), Counts::default()),
        };
        Self {
            path: path.to_owned(),
            since,
            total,
            session: Counts::default(),
        }
    }

    /// Count one sync, and save the counts of all runs.
    pub fn record(&mut self, report: &Report) -> Result<()> {
        self.session.add(report);
        self.total.add(report);
        let saved = Saved {
            since: self.since,
            counts: self.total,
        };
        nexo_core::write_atomically(&self.path, serde_json::to_string_pretty(&saved)?)
            .with_context(|| format!("saving {}", self.path.display()))
    }

    /// The day of the first count, such as `2026-11-14`.
    pub fn since(&self) -> String {
        self.since
            .with_timezone(&Local)
            .format("%Y-%m-%d")
            .to_string()
    }
}

/// The size of all files in a directory and the directories in it, in bytes. A file that
/// nexo cannot read counts as empty.
pub fn folder_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => folder_bytes(&entry.path()),
            Ok(meta) if meta.is_file() => meta.len(),
            _ => 0,
        })
        .sum()
}

/// A size for the user, such as `12.3 MB`.
pub fn size_words(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(received: usize, sent: usize, problems: &[&str]) -> Report {
        Report {
            received,
            sent,
            problems: problems.iter().map(|&p| p.to_owned()).collect(),
        }
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nexo-usage-{name}-{}", std::process::id()))
    }

    #[test]
    fn the_counts_of_all_runs_are_in_the_file() {
        let path = temp("counts.json");
        let _ = std::fs::remove_file(&path);
        let mut usage = Usage::load(&path);
        assert_eq!(usage.total, Counts::default());
        usage.record(&report(3, 1, &["chat: offline"])).unwrap();
        usage.record(&report(2, 0, &[])).unwrap();
        assert_eq!(usage.session.values(), [2, 5, 1, 1]);

        let mut next = Usage::load(&path);
        assert_eq!(next.total.values(), [2, 5, 1, 1]);
        assert_eq!(next.session, Counts::default());
        assert_eq!(next.since(), usage.since());
        next.record(&report(1, 0, &[])).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(next.total.values(), [3, 6, 1, 1]);
        assert_eq!(next.session.values(), [1, 1, 0, 0]);
    }

    #[test]
    fn a_file_that_is_not_json_starts_the_counts_again() {
        let path = temp("broken.json");
        std::fs::write(&path, "not json").unwrap();
        let usage = Usage::load(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(usage.total, Counts::default());
    }

    #[test]
    fn the_size_of_a_folder_counts_the_files_in_all_its_directories() {
        let dir = temp("folder");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("chat").join("jane")).unwrap();
        std::fs::write(dir.join("a.txt"), "12345").unwrap();
        std::fs::write(dir.join("chat").join("jane").join("b.md"), "123").unwrap();
        let bytes = folder_bytes(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(bytes, 8);
        assert_eq!(folder_bytes(&dir), 0);
    }

    #[test]
    fn a_size_has_the_unit_that_fits() {
        assert_eq!(size_words(0), "0 bytes");
        assert_eq!(size_words(1023), "1023 bytes");
        assert_eq!(size_words(1024), "1.0 KB");
        assert_eq!(size_words(12_900_000), "12.3 MB");
        assert_eq!(size_words(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
