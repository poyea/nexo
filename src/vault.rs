// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! The secrets of the channels, in the Windows Credential Manager. They are not in a file,
//! because the output folder can be in a cloud drive and `config.toml` is plain text.

use anyhow::{Context, Result};
use keyring_core::{Entry, Error};
use serde::Serialize;
use serde::de::DeserializeOwned;

const SERVICE: &str = "nexo";

/// Make the Credential Manager ready. Call it one time, before the other functions.
pub fn open() -> Result<()> {
    let store = windows_native_keyring_store::Store::new()
        .context("opening the Windows Credential Manager")?;
    keyring_core::set_default_store(store);
    Ok(())
}

fn entry(name: &str) -> Result<Entry> {
    Entry::new(SERVICE, name).with_context(|| format!("finding the secret of {name}"))
}

/// The secret with this name, such as the name of a channel. `None` if there is none.
pub fn load<T: DeserializeOwned>(name: &str) -> Result<Option<T>> {
    match entry(name)?.get_password() {
        Ok(json) => serde_json::from_str(&json)
            .map(Some)
            .with_context(|| format!("reading the secret of {name}")),
        Err(Error::NoEntry) => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading the secret of {name}")),
    }
}

pub fn save<T: Serialize>(name: &str, secret: &T) -> Result<()> {
    entry(name)?
        .set_password(&serde_json::to_string(secret)?)
        .with_context(|| format!("saving the secret of {name}"))
}

/// Remove the secret. It is not an error if there is none.
pub fn remove(name: &str) -> Result<()> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing the secret of {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_goes_in_and_out() {
        open().unwrap();
        let name = format!("test-{}", std::process::id());
        assert_eq!(load::<Vec<String>>(&name).unwrap(), None);
        save(&name, &vec!["a".to_owned(), "b".to_owned()]).unwrap();
        assert_eq!(
            load::<Vec<String>>(&name).unwrap(),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        remove(&name).unwrap();
        remove(&name).unwrap();
        assert_eq!(load::<Vec<String>>(&name).unwrap(), None);
    }
}
