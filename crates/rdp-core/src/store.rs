use crate::model::{Group, Profile};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Default)]
struct Data {
    profiles: Vec<Profile>,
    groups: Vec<Group>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    All,
    Favorites,
    Recent,
    Group(Uuid),
}

/// ponytail: one JSON file, whole-file atomic rewrite. Fine for hundreds of hosts;
/// move to SQLite only if load/save time becomes visible.
pub struct Store {
    path: PathBuf,
    data: Data,
}

impl Store {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("rustrdp").join("profiles.json")
    }

    /// Missing file means an empty store; a corrupt file is an error (never silently wiped).
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let data = match fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Data::default(),
            Err(e) => return Err(e),
        };
        Ok(Self { path, data })
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        fs::rename(tmp, &self.path)
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.data.profiles
    }

    pub fn groups(&self) -> &[Group] {
        &self.data.groups
    }

    pub fn get(&self, id: Uuid) -> Option<&Profile> {
        self.data.profiles.iter().find(|p| p.id == id)
    }

    pub fn upsert(&mut self, p: Profile) {
        match self.data.profiles.iter_mut().find(|x| x.id == p.id) {
            Some(slot) => *slot = p,
            None => self.data.profiles.push(p),
        }
    }

    pub fn remove(&mut self, id: Uuid) -> bool {
        let n = self.data.profiles.len();
        self.data.profiles.retain(|p| p.id != id);
        n != self.data.profiles.len()
    }

    pub fn add_group(&mut self, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.data.groups.push(Group { id, name: name.into() });
        id
    }

    /// Deleting a group keeps its profiles (they become ungrouped).
    pub fn remove_group(&mut self, id: Uuid) {
        self.data.groups.retain(|g| g.id != id);
        for p in self.data.profiles.iter_mut().filter(|p| p.group == Some(id)) {
            p.group = None;
        }
    }

    pub fn mark_connected(&mut self, id: Uuid) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if let Some(p) = self.data.profiles.iter_mut().find(|p| p.id == id) {
            p.last_connected = Some(now);
        }
    }

    /// Filtered and searched view. Recent: last 8 connected, newest first. Others: by name.
    pub fn list(&self, filter: Filter, query: &str) -> Vec<&Profile> {
        let mut v: Vec<&Profile> = self
            .data
            .profiles
            .iter()
            .filter(|p| match filter {
                Filter::All => true,
                Filter::Favorites => p.favorite,
                Filter::Recent => p.last_connected.is_some(),
                Filter::Group(g) => p.group == Some(g),
            })
            .filter(|p| p.matches(query))
            .collect();
        if filter == Filter::Recent {
            v.sort_by_key(|p| std::cmp::Reverse(p.last_connected));
            v.truncate(8);
        } else {
            v.sort_by_key(|p| p.name.to_lowercase());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("rustrdp-test-{}-{name}", Uuid::new_v4()))
            .join("profiles.json")
    }

    #[test]
    fn roundtrip_search_filters() {
        let path = tmp("a");
        let mut s = Store::open(&path).unwrap();
        let g = s.add_group("Production");
        let mut a = Profile::new("web-prod-01", "10.20.1.14");
        a.group = Some(g);
        a.favorite = true;
        a.tags = vec!["iis".into()];
        let b = Profile::new("db-prod-02", "10.20.1.22");
        let (aid, bid) = (a.id, b.id);
        s.upsert(a);
        s.upsert(b);
        s.mark_connected(bid);
        s.save().unwrap();

        let s = Store::open(&path).unwrap();
        assert_eq!(s.profiles().len(), 2);
        assert_eq!(s.list(Filter::All, "")[0].name, "db-prod-02"); // sorted by name
        assert_eq!(s.list(Filter::Favorites, "")[0].id, aid);
        assert_eq!(s.list(Filter::Group(g), "")[0].id, aid);
        assert_eq!(s.list(Filter::Recent, "")[0].id, bid);
        assert_eq!(s.list(Filter::All, "IIS").len(), 1); // tag, case-insensitive
        assert_eq!(s.list(Filter::All, "10.20.1.2")[0].id, bid); // host substring
        assert!(s.list(Filter::All, "nope").is_empty());
    }

    #[test]
    fn remove_group_ungroups_and_corrupt_file_errors() {
        let path = tmp("b");
        let mut s = Store::open(&path).unwrap();
        let g = s.add_group("Lab");
        let mut p = Profile::new("x", "h");
        p.group = Some(g);
        let id = p.id;
        s.upsert(p);
        s.remove_group(g);
        assert_eq!(s.get(id).unwrap().group, None);
        assert!(s.remove(id) && !s.remove(id));

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        assert!(Store::open(&path).is_err());
    }
}
