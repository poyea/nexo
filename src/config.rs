// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use glasskit::window::Material;
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

    /// The largest number that the unit takes: one day. The smallest is 1.
    fn max(self) -> u64 {
        match self {
            Unit::Seconds => 86_400,
            Unit::Minutes => 1_440,
        }
    }
}

/// The background of the window: how much of the desktop shows through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Look {
    /// A light tint of the desktop picture.
    #[default]
    Mica,
    /// The blurred desktop behind the window.
    Glass,
    /// One opaque color.
    Solid,
}

impl Look {
    pub const ALL: [Look; 3] = [Look::Mica, Look::Glass, Look::Solid];

    /// The name in the config file.
    fn name(self) -> &'static str {
        match self {
            Look::Mica => "mica",
            Look::Glass => "glass",
            Look::Solid => "solid",
        }
    }

    /// The name that the user sees.
    pub fn label(self) -> &'static str {
        match self {
            Look::Mica => "Mica",
            Look::Glass => "Glass",
            Look::Solid => "Solid",
        }
    }

    pub fn material(self) -> Material {
        match self {
            Look::Mica => Material::Mica,
            Look::Glass => Material::Glass,
            Look::Solid => Material::Solid,
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
        let max = unit.max();
        match text.trim().parse::<u64>() {
            Ok(every) if (1..=max).contains(&every) => Ok(Self { every, unit }),
            _ => Err(format!("Type a number from 1 to {max}.")),
        }
    }

    /// The part of typed text that can be a positive number: the digits, without zeros at
    /// the start. Thus the user cannot type zero, a sign, or a point.
    pub fn digits(text: &str) -> String {
        let digits: String = text.chars().filter(char::is_ascii_digit).collect();
        digits.trim_start_matches('0').to_owned()
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
    #[serde(default)]
    pub look: Look,
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
            look: Look::default(),
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
        let every =
            i64::try_from(interval.every).map_err(|_| anyhow!("the interval is too long"))?;
        save_values(
            path,
            &[
                ("sync_every", toml_edit::value(every)),
                ("sync_unit", toml_edit::value(interval.unit.name())),
            ],
        )?;
        self.sync_every = interval.every;
        self.sync_unit = interval.unit;
        Ok(())
    }

    /// Change the look here and in the file. The rest of the file stays as it is.
    pub fn save_look(&mut self, path: &Path, look: Look) -> Result<()> {
        save_values(path, &[("look", toml_edit::value(look.name()))])?;
        self.look = look;
        Ok(())
    }
}

/// Change these values in the file. The rest of the file stays as it is.
fn save_values(path: &Path, values: &[(&str, toml_edit::Item)]) -> Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    nexo_core::write_atomically(path, with_values(&text, values)?)
        .with_context(|| format!("saving {}", path.display()))
}

fn with_values(text: &str, values: &[(&str, toml_edit::Item)]) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("reading the config")?;
    for (key, value) in values {
        doc[key] = value.clone();
    }
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
            Interval::parse("1", Unit::Seconds),
            Ok(interval(1, Unit::Seconds))
        );
        assert_eq!(
            Interval::parse("0", Unit::Seconds),
            Err("Type a number from 1 to 86400.".into())
        );
        assert!(Interval::parse("86401", Unit::Seconds).is_err());
        assert!(Interval::parse("1441", Unit::Minutes).is_err());
        assert!(Interval::parse("", Unit::Minutes).is_err());
        assert!(Interval::parse("-3", Unit::Minutes).is_err());
        assert!(Interval::parse("1.5", Unit::Minutes).is_err());
    }

    #[test]
    fn digits_drop_all_that_is_not_a_positive_number() {
        assert_eq!(Interval::digits("30"), "30");
        assert_eq!(Interval::digits("0"), "");
        assert_eq!(Interval::digits("007"), "7");
        assert_eq!(Interval::digits("-5"), "5");
        assert_eq!(Interval::digits("1.5x"), "15");
        assert_eq!(Interval::digits("100"), "100");
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
        assert!(toml::from_str::<Config>(&format!("{OUTPUT}sync_every = -5")).is_err());
        assert!(check("sync_every = 86401\nsync_unit = \"seconds\"").is_err());
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
        let values = [
            ("sync_every", toml_edit::value(30)),
            ("sync_unit", toml_edit::value("seconds")),
        ];
        let saved = with_values(text, &values).unwrap();
        assert_eq!(
            saved,
            "# My folder\noutput = 'C:\\nexo'\nsync_every = 30\nsync_unit = \"seconds\"\n"
        );
        let config: Config = toml::from_str(&saved).unwrap();
        assert_eq!(config.interval(), interval(30, Unit::Seconds));
    }

    #[test]
    fn the_look_is_mica_unless_the_file_says_more() {
        let config: Config = toml::from_str(OUTPUT).unwrap();
        assert_eq!(config.look, Look::Mica);
        for look in Look::ALL {
            let saved = with_values(OUTPUT, &[("look", toml_edit::value(look.name()))]).unwrap();
            assert_eq!(toml::from_str::<Config>(&saved).unwrap().look, look);
        }
        assert!(toml::from_str::<Config>(&format!("{OUTPUT}look = \"clear\"")).is_err());
    }
}
