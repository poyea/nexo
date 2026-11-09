// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Write the file in full or not at all: write a temporary file next to it, then replace the
/// file with it. Thus a crash or a full disk cannot leave half a file.
pub fn write_atomically(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let tmp = temporary_path(path);
    std::fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".tmp");
    PathBuf::from(name)
}

/// A directory name made from `title`: lowercase letters and digits, with `-` between words.
pub fn slug(title: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.chars().take(max_chars).collect();
    out.trim_end_matches('-').to_owned()
}

/// FNV-1a. It gives the same value on all machines and in all versions, as a directory name
/// must.
pub fn stable_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_replaces_the_file_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("nexo-files-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        write_atomically(&path, "old").unwrap();
        write_atomically(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(!temporary_path(&path).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn slug_keeps_words_and_drops_the_rest() {
        assert_eq!(slug("Jane Doe, John Roe", 40), "jane-doe-john-roe");
        assert_eq!(slug("  ¡Hola!  ", 40), "hola");
        assert_eq!(slug("陳大文", 40), "陳大文");
        assert_eq!(slug("abc def", 4), "abc");
        assert_eq!(slug("...", 40), "");
    }

    #[test]
    fn hash_is_fnv1a() {
        assert_eq!(stable_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
