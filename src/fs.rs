use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

/// Определяем корень проекта: git → cwd
pub fn detect_project_root() -> Result<PathBuf> {
    if let Ok(out) = Command::new("git").args(["rev-parse", "--show-toplevel"]).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() { return Ok(PathBuf::from(s)); }
        }
    }
    Ok(std::env::current_dir()?)
}

/// Убедиться, что .gptcli существует
pub fn ensure_project_dirs(root: &PathBuf) -> Result<()> {
    std::fs::create_dir_all(root.join(".gptcli"))?;
    Ok(())
}
