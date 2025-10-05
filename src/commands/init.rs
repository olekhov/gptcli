use anyhow::{Context, Result};
use crate::{fs as ufs, state::ProjectState};
use crate::db::open_db;

pub fn run(namespace_opt: Option<String>) -> Result<()> {
    let root = ufs::detect_project_root()?;
    ufs::ensure_project_dirs(&root)?;

    // по умолчанию namespace = basename(root)@main (можно улучшить позже)
    let default_ns = format!("{}@main", root.file_name().unwrap().to_string_lossy());
    let namespace = namespace_opt.unwrap_or(default_ns);

    let st = ProjectState::new(root.clone(), namespace)?;
    st.save().context("failed to save state")?;

    let _conn = open_db(&root)?;

    println!("Инициализировано: {}", root.display());
    println!("• .gptcli/state.json\n• .gptcli/index.sqlite (schema v1)");

    Ok(())
}
