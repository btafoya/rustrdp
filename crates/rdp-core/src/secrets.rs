use keyring::Entry;
use uuid::Uuid;

const SERVICE: &str = "rustrdp";

fn entry(id: Uuid) -> keyring::Result<Entry> {
    Entry::new(SERVICE, &id.to_string())
}

pub fn set_password(id: Uuid, password: &str) -> keyring::Result<()> {
    entry(id)?.set_password(password)
}

/// `Ok(None)` when nothing is stored.
pub fn get_password(id: Uuid) -> keyring::Result<Option<String>> {
    match entry(id)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn delete_password(id: Uuid) -> keyring::Result<()> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e),
    }
}
