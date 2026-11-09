use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Gateway {
    pub host: String,
    pub username: String,
    pub domain: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum DisplayMode {
    /// Follow the window size and resize dynamically.
    #[default]
    Dynamic,
    Fixed,
    Fullscreen,
}

/// How the client shows a remote desktop of a fixed size; ignored in `DisplayMode::Dynamic`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum ViewMode {
    /// 1:1, scrollbars if the window is smaller.
    #[default]
    Actual,
    /// Resize the window to wrap the desktop 1:1 (falls back to `Scale` if the screen is too small).
    FitWindow,
    /// Scale the desktop to the window, aspect ratio kept.
    Scale,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Display {
    pub mode: DisplayMode,
    pub width: u32,
    pub height: u32,
    /// Desktop scale factor in percent (100..=500).
    pub scale: u32,
    #[serde(default)]
    pub view: ViewMode,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Dynamic,
            width: 1920,
            height: 1080,
            scale: 100,
            view: ViewMode::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
/// FreeRDP 3.33's drive channel has no read-only flag, so there is deliberately no such field:
/// a shared folder is read/write for the remote side.
pub struct SharedFolder {
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Redirect {
    pub clipboard: bool,
    pub audio: bool,
    pub printers: bool,
    /// Drive redirection is opt-in: nothing is shared unless listed here.
    pub folders: Vec<SharedFolder>,
}

impl Default for Redirect {
    fn default() -> Self {
        Self {
            // Off by default: with it on, every local clipboard change (a copied password) goes to the server.
            clipboard: false,
            audio: true,
            printers: false,
            folders: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub group: Option<Uuid>,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub gateway: Option<Gateway>,
    pub display: Display,
    pub redirect: Redirect,
    /// Raw FreeRDP settings by `FreeRDP_*` key name, applied last ("Advanced" page).
    pub overrides: BTreeMap<String, String>,
    /// RemoteApp program alias or path (e.g. `||excel`); `None` = full desktop.
    #[serde(default)]
    pub remote_app: Option<String>,
    pub last_connected: Option<u64>,
}

impl Profile {
    pub fn new(name: &str, host: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            group: None,
            tags: Vec::new(),
            favorite: false,
            host: host.into(),
            port: 3389,
            username: String::new(),
            domain: String::new(),
            gateway: None,
            display: Display::default(),
            redirect: Redirect::default(),
            overrides: BTreeMap::new(),
            remote_app: None,
            last_connected: None,
        }
    }

    /// Case-insensitive substring match over name, host, username and tags.
    pub fn matches(&self, q: &str) -> bool {
        let q = q.to_lowercase();
        q.is_empty()
            || [&self.name, &self.host, &self.username]
                .iter()
                .any(|s| s.to_lowercase().contains(&q))
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
    }
}

/// `DOMAIN\user` typed into the username field becomes (user, DOMAIN), but only when no domain is set.
/// FreeRDP's NLA sends the username verbatim, so an unsplit `DOMAIN\user` fails with a logon error.
/// `user@domain` (UPN) is left alone: servers accept it as is.
pub fn split_user(username: &str, domain: &str) -> (String, String) {
    match username.split_once('\\') {
        Some((d, u)) if domain.is_empty() && !u.is_empty() => (u.into(), d.into()),
        _ => (username.into(), domain.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_user_moves_backslash_domain_only_when_domain_is_empty() {
        assert_eq!(split_user("CORP\\bob", ""), ("bob".into(), "CORP".into()));
        assert_eq!(split_user(".\\bob", ""), ("bob".into(), ".".into()));
        assert_eq!(split_user("CORP\\bob", "OTHER"), ("CORP\\bob".into(), "OTHER".into()));
        assert_eq!(split_user("bob@example.com", ""), ("bob@example.com".into(), "".into()));
        assert_eq!(split_user("bob", "CORP"), ("bob".into(), "CORP".into()));
    }

    #[test]
    fn new_profiles_share_nothing_but_audio_by_default() {
        let r = Redirect::default();
        assert!(!r.clipboard && !r.printers && r.folders.is_empty());
    }

    #[test]
    fn display_without_view_field_loads_as_actual() {
        let d: Display = serde_json::from_str(r#"{"mode":"Fixed","width":800,"height":600,"scale":100}"#).unwrap();
        assert_eq!(d.view, ViewMode::Actual);
    }
}
