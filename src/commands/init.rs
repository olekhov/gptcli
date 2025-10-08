use anyhow::{Context, Result};
use crate::context::AppCtx;
use crate::{fs as ufs, state::ProjectState};

pub fn run(ctx: &AppCtx, namespace_opt: Option<String>) -> Result<()> {
    ufs::ensure_project_dirs(&ctx.root)?;

    // по умолчанию namespace = basename(root)@main (можно улучшить позже)
    let default_ns = format!("{}@main", ctx.root.file_name().unwrap().to_string_lossy());
    let namespace = namespace_opt.unwrap_or(default_ns);

    let st = ProjectState::new(ctx.root.clone(), namespace)?;
    st.save().context("failed to save state")?;

    let _conn = ctx.open_db()?;

    println!("Инициализировано: {}", ctx.root.display());
    println!("• .gptcli/state.json\n• .gptcli/index.sqlite (schema v1)");

    Ok(())
}
