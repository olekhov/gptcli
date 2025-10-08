use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, time::SystemTime};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ProjectState {
    pub project_root: PathBuf,
    pub namespace: String,          // repo@branch
    pub current_thread_id: Option<String>,
    pub last_head: Option<String>,  // короткий SHA, если нужно
    pub created_at: i64,
}

impl ProjectState {
    pub fn path(root: &PathBuf) -> PathBuf {
        root.join(".gptcli/state.json")
    }

    pub fn load(root: &PathBuf) -> Result<Self> {
        let p = Self::path(root);
        match fs::read_to_string(&p) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(_) => {
                tracing::warn!("State file not found, using temporary");
                ProjectState::new(p, "temporary".to_string())
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path(&self.project_root);
        if let Some(parent) = p.parent() { fs::create_dir_all(parent)?; }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&p, data)?;
        Ok(())
    }

    pub fn new(root: PathBuf, namespace: String) -> Result<Self> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Ok(Self {
            project_root: root,
            namespace,
            current_thread_id: None,
            last_head: None,
            created_at: now,
        })
    }
}
