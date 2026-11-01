//! Durable evidence review only. No approval, execution or model capability.
use crate::advisory::ReviewProposal;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Pending,
    Acknowledged,
    Dismissed,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: String,
    pub proposal: ReviewProposal,
    pub review: Decision,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    context: serde_json::Value,
    rows: Vec<Row>,
}
pub struct Store {
    directory: PathBuf,
    context: serde_json::Value,
    rows: Vec<Row>,
    _lock: fs::File,
    poisoned: bool,
}
impl Store {
    pub fn open(directory: &Path, context: serde_json::Value) -> Result<Self, &'static str> {
        let metadata = fs::symlink_metadata(directory).map_err(|_| "state unavailable")?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("unsafe state");
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join("lock"))
            .map_err(|_| "state unavailable")?;
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("state busy");
        }
        let state: Option<State> = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory.join("proposals.json"))
        {
            Ok(file) => {
                let mut data = Vec::new();
                file.take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut data)
                    .map_err(|_| "state unavailable")?;
                if data.len() > 8 * 1024 * 1024 {
                    return Err("state bounds");
                }
                Some(serde_json::from_slice(&data).map_err(|_| "corrupt or unbound state")?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("state unavailable"),
        };
        let fresh = state.is_none();
        if fresh {
            match fs::symlink_metadata(directory.join("proposals.next")) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("unbound or uncertain state"),
            }
        }
        let rows = match state {
            Some(state) if state.context == context => state.rows,
            Some(_) => return Err("review context mismatch"),
            None => Vec::new(),
        };
        if rows.len() > 100
            || rows
                .iter()
                .enumerate()
                .any(|(i, r)| r.id != format!("proposal-{}", i + 1))
        {
            return Err("corrupt state");
        }
        let mut store = Self {
            directory: directory.into(),
            context,
            rows,
            _lock: lock,
            poisoned: false,
        };
        // Bind even an empty store before admitting any caller.
        if fresh {
            store.save()?;
        }
        Ok(store)
    }
    fn save(&mut self) -> Result<(), &'static str> {
        // Any uncertain publication poisons this process: never acknowledge or replay it.
        self.poisoned = true;
        #[derive(Serialize)]
        struct Snapshot<'a> {
            context: &'a serde_json::Value,
            rows: &'a [Row],
        }
        let bytes = serde_json::to_vec(&Snapshot {
            context: &self.context,
            rows: &self.rows,
        })
        .map_err(|_| "state unavailable")?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("state bounds");
        }
        let path = self.directory.join("proposals.next");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|_| "state unavailable")?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "state unavailable")?;
        fs::rename(path, self.directory.join("proposals.json")).map_err(|_| "state unavailable")?;
        fs::File::open(&self.directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| "state unavailable")?;
        self.poisoned = false;
        Ok(())
    }
    pub fn list(&self) -> Result<serde_json::Value, &'static str> {
        if self.poisoned {
            return Err("state uncertain");
        }
        Ok(serde_json::json!({"proposals": self.rows}))
    }
    pub fn submit(&mut self, proposal: ReviewProposal) -> Result<serde_json::Value, &'static str> {
        if self.poisoned || self.rows.len() >= 100 {
            return Err("state unavailable");
        }
        let id = format!("proposal-{}", self.rows.len() + 1);
        self.rows.push(Row {
            id,
            proposal,
            review: Decision::Pending,
        });
        self.save()?;
        serde_json::to_value(self.rows.last()).map_err(|_| "state unavailable")
    }
    pub fn review(
        &mut self,
        id: &str,
        decision: Decision,
    ) -> Result<serde_json::Value, &'static str> {
        if self.poisoned || decision == Decision::Pending {
            return Err("denied");
        }
        let index = self
            .rows
            .iter()
            .position(|r| r.id == id)
            .ok_or("not found")?;
        if self.rows[index].review != Decision::Pending && self.rows[index].review != decision {
            return Err("already reviewed");
        }
        if self.rows[index].review == Decision::Pending {
            self.rows[index].review = decision;
            self.save()?;
        }
        serde_json::to_value(&self.rows[index]).map_err(|_| "state unavailable")
    }
}
