// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const MIN_SYNC_MINUTES: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where nexo keeps the conversations. A folder of Google Drive for desktop also works.
    pub output: PathBuf,
    #[serde(default = "default_sync_minutes")]
    pub sync_minutes: u64,
}

fn default_sync_minutes() -> u64 {
    5
}

impl Config {
    pub fn sync_every(&self) -> Duration {
        Duration::from_secs(self.sync_minutes * 60)
    }

    fn with_defaults() -> Result<Self> {
        let home = std::env::var_os("USERPROFILE").context("USERPROFILE is not set")?;
        Ok(Self {
            output: PathBuf::from(home).join("nexo"),
            sync_minutes: default_sync_minutes(),
        })
    }

    fn check(self, path: &Path) -> Result<Self> {
        if self.sync_minutes < MIN_SYNC_MINUTES {
            bail!(
                "{}: sync_minutes must be {MIN_SYNC_MINUTES} or more",
                path.display()
            );
        }
        if !self.output.is_absolute() {
            bail!("{}: output must be a full path", path.display());
        }
        Ok(self)
    }
}

/// Read the config, or write one with the defaults if it is not there.
pub fn load_or_create(path: &Path) -> Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<Config>(&text)
            .with_context(|| format!("reading {}", path.display()))?
            .check(path),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let config = Config::with_defaults()?;
            nexo_core::write_atomically(path, toml::to_string(&config)?)?;
            Ok(config)
        }
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_minutes_has_a_default() {
        let config: Config = toml::from_str("output = 'C:\\\\nexo'").unwrap();
        assert_eq!(config.sync_every(), Duration::from_secs(300));
    }

    #[test]
    fn bad_values_are_errors() {
        let path = Path::new("config.toml");
        let zero: Config = toml::from_str("output = 'C:\\\\nexo'\nsync_minutes = 0").unwrap();
        assert!(zero.check(path).is_err());
        let relative: Config = toml::from_str("output = 'nexo'").unwrap();
        assert!(relative.check(path).is_err());
        assert!(toml::from_str::<Config>("output = 'C:\\\\nexo'\ntypo = 1").is_err());
    }

    #[test]
    fn defaults_go_through_toml_unchanged() {
        let config = Config::with_defaults().unwrap();
        let text = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }
}
