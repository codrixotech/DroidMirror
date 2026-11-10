//! Pairings with companion phones, kept in
//! `~/Library/Application Support/DroidMirror/companion-pairings`
//! (one line per phone: id, certificate fingerprint, token, name; mode 0600).

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::sha256::{from_hex, hex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingRecord {
    pub id: String,
    pub name: String,
    pub fingerprint: [u8; 32],
    pub token: [u8; 32],
}

impl PairingRecord {
    fn to_line(&self) -> String {
        let name: String = self
            .name
            .chars()
            .filter(|c| *c != '\t' && *c != '\n')
            .collect();
        format!(
            "{}\t{}\t{}\t{}",
            self.id,
            hex(&self.fingerprint),
            hex(&self.token),
            name
        )
    }

    fn from_line(line: &str) -> Option<Self> {
        let mut p = line.splitn(4, '\t');
        let id = p.next()?.to_owned();
        let fingerprint = from_hex(p.next()?)?.try_into().ok()?;
        let token = from_hex(p.next()?)?.try_into().ok()?;
        let name = p.next().unwrap_or("").to_owned();
        (!id.is_empty()).then_some(Self {
            id,
            name,
            fingerprint,
            token,
        })
    }
}

pub struct PairingStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl PairingStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// The default location on macOS.
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support/DroidMirror/companion-pairings"))
    }

    pub fn all(&self) -> Vec<PairingRecord> {
        let _g = self.lock.lock().unwrap();
        self.read()
    }

    pub fn get(&self, id: &str) -> Option<PairingRecord> {
        self.all().into_iter().find(|r| r.id == id)
    }

    pub fn save(&self, record: &PairingRecord) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut all: Vec<_> = self
            .read()
            .into_iter()
            .filter(|r| r.id != record.id)
            .collect();
        all.push(record.clone());
        self.write(&all)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let all: Vec<_> = self.read().into_iter().filter(|r| r.id != id).collect();
        self.write(&all)
    }

    fn read(&self) -> Vec<PairingRecord> {
        std::fs::read_to_string(&self.path)
            .map(|s| s.lines().filter_map(PairingRecord::from_line).collect())
            .unwrap_or_default()
    }

    fn write(&self, records: &[PairingRecord]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let text: String = records.iter().map(|r| r.to_line() + "\n").collect();
        let tmp = self.path.with_extension("tmp");
        write_private(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("saving {}", self.path.display()))
    }
}

/// Writes a file readable only by the user (it holds access tokens).
fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, n: u8) -> PairingRecord {
        PairingRecord {
            id: id.into(),
            name: format!("Phone\t{n}"),
            fingerprint: [n; 32],
            token: [n + 1; 32],
        }
    }

    #[test]
    fn save_load_replace_remove() {
        let dir = std::env::temp_dir().join(format!("dm-store-{}", std::process::id()));
        let store = PairingStore::new(dir.join("sub/pairings"));
        assert!(store.all().is_empty());
        store.save(&record("a", 1)).unwrap();
        store.save(&record("b", 2)).unwrap();
        store.save(&record("a", 3)).unwrap(); // replaces
        let all = store.all();
        assert_eq!(all.len(), 2);
        let a = store.get("a").unwrap();
        assert_eq!(a.fingerprint, [3; 32]);
        assert_eq!(a.token, [4; 32]);
        assert_eq!(a.name, "Phone3"); // tabs stripped
        store.remove("a").unwrap();
        assert!(store.get("a").is_none());
        assert!(store.get("b").is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("sub/pairings"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ignores_bad_lines() {
        assert!(PairingRecord::from_line("x\tnothex\t00\tn").is_none());
        assert!(PairingRecord::from_line("").is_none());
    }
}
