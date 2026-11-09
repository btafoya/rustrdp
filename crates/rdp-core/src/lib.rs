#![forbid(unsafe_code)]
//! Profiles, storage, .rdp import/export, reachability probe, keyring. No GTK, no FFI.

pub mod model;
pub mod probe;
pub mod rdpfile;
pub mod secrets;
pub mod store;

pub use model::{split_user, Display, DisplayMode, Gateway, Group, Profile, Redirect, SharedFolder, ViewMode};
pub use probe::{probe, Reach};
pub use store::{Filter, Store};
