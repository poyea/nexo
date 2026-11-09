// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Seconds,
    Minutes,
}

impl Unit {
    fn seconds(self) -> u64 {
        match self {
            Unit::Seconds => 1,
            Unit::Minutes => 60,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Unit::Seconds => "seconds",
            Unit::Minutes => "minutes",
        }
    }

    /// The numbers that the unit takes: 5 seconds to one day.
    fn range(self) -> (u64, u64) {
        match self {
            Unit::Seconds => (5, 86_400),
            Unit::Minutes => (1, 1_440),
        }
    }
}

/// The time from the end of one sync to the start of the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub every: u64,
    pub unit: Unit,
}

impl Interval {
    /// The interval that the user typed, or what is wrong with it.
    pub fn parse(text: &str, unit: Unit) -> Result<Self, String> {
        let (min, max) = unit.range();
        match text.trim().parse::<u64>() {
            Ok(every) if (min..=max).contains(&every) => Ok(Self { every, unit }),
            _ => Err(format!("Type a number from {min} to {max}.")),
        }
    }

    pub fn duration(self) -> Duration {
        Duration::from_secs(self.every * self.unit.seconds())
    }

    pub fn describe(self) -> String {
        match (self.every, self.unit) {
            (1, Unit::Minutes) => "Sync every minute".into(),
            (n, unit) => format!("Sync every {n} {}", unit.name()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where nexo keeps the conversations. A folder of Google Drive for desktop also works.
    pub output: PathBuf,
    #[serde(default = "default_sync_every")]
    sync_every: u64,
    #[serde(default = "default_sync_unit")]
    sync_unit: Unit,
}

fn default_sync_every() -> u64 {
    5
}

fn default_sync_unit() -> Unit {
    Unit::Minutes
}

impl Config {
    pub fn interval(&self) -> Interval {
        Interval {
            every: self.sync_every,
            unit: self.sync_unit,
        }
    }

    fn with_defaults() -> Result<Self> {
        let home = std::env::var_os("USERPROFILE").context("USERPROFILE is not set")?;
        Ok(Self {
            output: PathBuf::from(home).join("nexo"),
            sync_every: default_sync_every(),
            sync_unit: default_sync_unit(),
        })
    }

    fn check(self, path: &Path) -> Result<Self> {
        if let Err(why) = Interval::parse(&self.sync_every.to_string(), self.sync_unit) {
            bail!(
                "{}: sync_every = {} is not valid with sync_unit = \"{}\". {why}",
                path.display(),
                self.sync_every,
                self.sync_unit.name()
            );
        }
        if !self.output.is_absolute() {
            bail!("{}: output must be a full path", path.display());
        }
        Ok(self)
    }

    /// Change the interval here and in the file. The rest of the file stays as it is.
    pub fn save_interval(&mut self, path: &Path, interval: Interval) -> Result<()> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        nexo_core::write_atomically(path, with_interval(&text, interval)?)
            .with_context(|| format!("saving {}", path.display()))?;
        self.sync_every = interval.every;
        self.sync_unit = interval.unit;
        Ok(())
    }
}

fn with_interval(text: &str, interval: Interval) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("reading the config")?;
    let every = i64::try_from(interval.every).map_err(|_| anyhow!("the interval is too long"))?;
    doc["sync_every"] = toml_edit::value(every);
    doc["sync_unit"] = toml_edit::value(interval.unit.name());
    Ok(doc.to_string())
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

    const OUTPUT: &str = "output = 'C:\\nexo'\n";

    fn interval(every: u64, unit: Unit) -> Interval {
        Interval { every, unit }
    }

    #[test]
    fn the_default_is_five_minutes() {
        let config: Config = toml::from_str(OUTPUT).unwrap();
        assert_eq!(config.interval(), interval(5, Unit::Minutes));
        assert_eq!(config.interval().duration(), Duration::from_secs(300));
    }

    #[test]
    fn an_interval_can_be_in_seconds() {
        let text = format!("{OUTPUT}sync_every = 30\nsync_unit = \"seconds\"\n");
        let config: Config = toml::from_str(&text).unwrap();
        assert_eq!(config.interval().duration(), Duration::from_secs(30));
    }

    #[test]
    fn parse_takes_only_numbers_in_the_range() {
        assert_eq!(
            Interval::parse(" 30 ", Unit::Seconds),
            Ok(interval(30, Unit::Seconds))
        );
        assert_eq!(
            Interval::parse("1", Unit::Minutes),
            Ok(interval(1, Unit::Minutes))
        );
        assert_eq!(
            Interval::parse("4", Unit::Seconds),
            Err("Type a number from 5 to 86400.".into())
        );
        assert!(Interval::parse("1441", Unit::Minutes).is_err());
        assert!(Interval::parse("", Unit::Minutes).is_err());
        assert!(Interval::parse("-3", Unit::Minutes).is_err());
        assert!(Interval::parse("1.5", Unit::Minutes).is_err());
    }

    #[test]
    fn describe_says_the_number_and_the_unit() {
        assert_eq!(interval(1, Unit::Minutes).describe(), "Sync every minute");
        assert_eq!(
            interval(5, Unit::Minutes).describe(),
            "Sync every 5 minutes"
        );
        assert_eq!(
            interval(30, Unit::Seconds).describe(),
            "Sync every 30 seconds"
        );
    }

    #[test]
    fn bad_values_are_errors() {
        let path = Path::new("config.toml");
        let check = |more: &str| {
            toml::from_str::<Config>(&format!("{OUTPUT}{more}"))
                .unwrap()
                .check(path)
        };
        assert!(check("").is_ok());
        assert!(check("sync_every = 0").is_err());
        assert!(check("sync_every = 2\nsync_unit = \"seconds\"").is_err());
        let relative: Config = toml::from_str("output = 'nexo'").unwrap();
        assert!(relative.check(path).is_err());
        assert!(toml::from_str::<Config>(&format!("{OUTPUT}typo = 1")).is_err());
        assert!(toml::from_str::<Config>(&format!("{OUTPUT}sync_unit = \"hours\"")).is_err());
    }

    #[test]
    fn defaults_go_through_toml_unchanged() {
        let config = Config::with_defaults().unwrap();
        let text = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }

    #[test]
    fn a_new_interval_keeps_the_rest_of_the_file() {
        let text = "# My folder\noutput = 'C:\\nexo'\nsync_every = 5\n";
        let saved = with_interval(text, interval(30, Unit::Seconds)).unwrap();
        assert_eq!(
            saved,
            "# My folder\noutput = 'C:\\nexo'\nsync_every = 30\nsync_unit = \"seconds\"\n"
        );
        let config: Config = toml::from_str(&saved).unwrap();
        assert_eq!(config.interval(), interval(30, Unit::Seconds));
    }
}
