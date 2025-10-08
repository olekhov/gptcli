use anyhow::{Result, Context};
use std::{fs, path::PathBuf};
use crate::appconfig::{load_effective, global_config_path, project_config_path};
use crate::fs as ufs;

pub enum ConfigSub {
    Init,
    Show,
}

pub fn run(sub: ConfigSub) -> Result<()> {
    match sub {
        ConfigSub::Init => init(),
        ConfigSub::Show => show(),
    }
}

fn init() -> Result<()> {
    let root = ufs::detect_project_root()?;
    let g = global_config_path();
    let p = project_config_path(&root);

    // создаём каталоги
    if let Some(dir) = g.parent() { fs::create_dir_all(dir)?; }
    if let Some(dir) = p.parent() { fs::create_dir_all(dir)?; }

    // заготовки (если не существуют)
    if !g.exists() {
        let tpl = r#"# ~/.config/gptcli/config.toml
default_profile = "openai"
lang = "ru"
model = "gpt-4.1-mini"
max_output_tokens = 1200

[profiles.openai]
provider = "openai"
api_base = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[profiles.local]
provider = "openai"
api_base = "http://localhost:8000/v1"
api_key = "EMPTY"
model = "qwen2.5-coder-32b"
"#;
        fs::write(&g, tpl)?;
        println!("created global config: {}", g.display());
    } else {
        println!("global config exists: {}", g.display());
    }

    if !p.exists() {
        let tpl = r#"# .gptcli/config.toml (project override)
# default_profile = "local"
# lang = "en"
# model = "qwen2.5-coder-32b"
"#;
        fs::write(&p, tpl)?;
        println!("created project config: {}", p.display());
    } else {
        println!("project config exists: {}", p.display());
    }
    Ok(())
}

fn show() -> Result<()> {
    let root = ufs::detect_project_root()?;
    let eff = load_effective(&root)?;
    println!("Profile:   {}", eff.profile_name);
    println!("API base:  {}", eff.api_base);
    println!("Model:     {}", eff.model);
    println!("Lang:      {}", eff.lang);
    println!("Max out:   {}", eff.max_output_tokens);
    println!("Global:    {}", eff.global_path.as_ref().map(|p| p.display().to_string()).unwrap_or("-".into()));
    println!("Project:   {}", eff.project_path.display());
    // ключ не печатаем, только источник
    println!("API key:   {}", if std::env::var("OPENAI_API_KEY").is_ok() { "env:OPENAI_API_KEY" } else { "config (hidden)" });
    Ok(())
}
