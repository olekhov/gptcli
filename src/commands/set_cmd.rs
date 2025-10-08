use anyhow::{Result, Context, bail};
use std::{fs, path::PathBuf};
use crate::fs as ufs;
use crate::appconfig::{project_config_path, RootCfg, Profile};

pub fn run(key: &str, value: &str, profile: Option<&str>) -> Result<()> {
    let root = ufs::detect_project_root()?;
    let ppath = project_config_path(&root);
    let mut cfg: RootCfg = if ppath.exists() {
        toml::from_str(&fs::read_to_string(&ppath).context("read project config")?)
            .context("parse project config")?
    } else { RootCfg::default() };

    match key {
        "default_profile" => cfg.default_profile = Some(value.to_string()),
        "lang" => cfg.lang = Some(value.to_string()),
        "model" => cfg.model = Some(value.to_string()),
        "max_output_tokens" => cfg.max_output_tokens = Some(value.parse()?),
        k if k.starts_with("profiles.") => {
            // format: profiles.<name>.(provider|api_base|api_key|api_key_env|model)
            let parts: Vec<&str> = k.split('.').collect();
            if parts.len()!=3 { bail!("use: profiles.<name>.<field>"); }
            let name = parts[1].to_string();
            let field = parts[2];
            let prof = cfg.profiles.entry(name).or_insert(Profile{
                provider: "openai".into(), ..Default::default()
            });
            match field {
                "provider" => prof.provider = value.into(),
                "api_base" => prof.api_base = Some(value.into()),
                "api_key" => prof.api_key = Some(value.into()),
                "api_key_env" => prof.api_key_env = Some(value.into()),
                "model" => prof.model = Some(value.into()),
                _ => bail!("unknown field '{}'", field),
            }
        }
        _ => {
            // короткие алиасы: set api_base/ api_key(_env)/ provider/ model для текущего профиля
            let prof_name = profile.map(|s| s.to_string())
                .or(cfg.default_profile.clone()).unwrap_or_else(|| "openai".into());
            let prof = cfg.profiles.entry(prof_name).or_insert(Profile{
                provider: "openai".into(), ..Default::default()
            });
            match key {
                "provider" => prof.provider = value.into(),
                "api_base" => prof.api_base = Some(value.into()),
                "api_key" => prof.api_key = Some(value.into()),
                "api_key_env" => prof.api_key_env = Some(value.into()),
                "profile_model" => prof.model = Some(value.into()),
                _ => bail!("unknown key '{}'", key),
            }
        }
    }

    if let Some(dir) = ppath.parent() { fs::create_dir_all(dir)?; }
    fs::write(&ppath, toml::to_string_pretty(&cfg)?)?;
    println!("updated {}", ppath.display());
    Ok(())
}
