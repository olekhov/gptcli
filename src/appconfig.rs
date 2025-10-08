use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, fs, path::{Path, PathBuf}};
use dirs::{config_dir, home_dir};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Profile {
    pub provider: String,              // "openai" | "openai_compat" | "azure" (пока неважно)
    pub api_base: Option<String>,      // e.g. https://api.openai.com/v1, http://localhost:8000/v1
    pub api_key: Option<String>,       // discouraged: лучше api_key_env
    pub api_key_env: Option<String>,   // e.g. OPENAI_API_KEY
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RootCfg {
    pub default_profile: Option<String>,
    pub lang: Option<String>,                // "ru"|"en"|"auto"
    pub model: Option<String>,
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
}

#[derive(Debug, Clone)]
pub struct Effective {
    pub profile_name: String,
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub lang: String,
    pub max_output_tokens: u32,
    pub global_path: Option<PathBuf>,
    pub project_path: PathBuf,
}

pub fn global_config_path() -> PathBuf {
    if let Some(dir) = config_dir() {
        return dir.join("gptcli/config.toml");
    }
    home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config/gptcli/config.toml")
}

pub fn project_config_path(project_root: &Path) -> PathBuf {
    project_root.join(".gptcli/config.toml")
}

/// загрузить глобальный + проектный конфиг, смержить, выбрать профиль и построить Effective
pub fn load_effective(project_root: &Path) -> Result<Effective> {
    let gpath = global_config_path();
    let ppath = project_config_path(project_root);

    let mut g: RootCfg = if gpath.exists() {
        toml::from_str(&fs::read_to_string(&gpath).context("read global config")?)
            .context("parse global config toml")?
    } else { RootCfg::default() };

    let mut p: RootCfg = if ppath.exists() {
        toml::from_str(&fs::read_to_string(&ppath).context("read project config")?)
            .context("parse project config toml")?
    } else { RootCfg::default() };

    // merge: g <- p (p переопределяет)
    let mut m = g.clone();
    if p.default_profile.is_some() { m.default_profile = p.default_profile.clone(); }
    if p.lang.is_some()            { m.lang = p.lang.clone(); }
    if p.model.is_some()           { m.model = p.model.clone(); }
    if p.max_output_tokens.is_some(){ m.max_output_tokens = p.max_output_tokens; }
    for (k,v) in p.profiles.drain() { m.profiles.insert(k, v); }

    // профиль: ENV > merged.default_profile > "openai"
    let profile_name = env::var("GPTCLI_PROFILE").ok()
        .or(m.default_profile.clone()).unwrap_or_else(|| "openai".to_string());

    let prof = m.profiles.get(&profile_name)
        .with_context(|| format!("profile '{profile_name}' not found (define in ~/.config/gptcli/config.toml or .gptcli/config.toml)"))?;

    // api_base: профайл → дефолт openai
    let api_base = prof.api_base.clone()
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

    // api_key: ENV(priority) -> literal -> common OPENAI_API_KEY
    let api_key = if let Some(var) = &prof.api_key_env {
        env::var(var).with_context(|| format!("env {var} is not set"))?
    } else if let Some(k) = &prof.api_key {
        k.clone()
    } else if let Ok(k) = env::var("OPENAI_API_KEY") {
        k
    } else {
        bail!("API key is missing: set env OPENAI_API_KEY or profiles.<name>.api_key(_env)");
    };

    // модель/язык/лимит
    let model = p.model.or(m.model).unwrap_or_else(|| "gpt-4.1-mini".into());
    let lang  = p.lang.or(m.lang).unwrap_or_else(|| "auto".into());
    let max_output_tokens = p.max_output_tokens.or(m.max_output_tokens).unwrap_or(1200);

    Ok(Effective {
        profile_name,
        api_base,
        api_key,
        model,
        lang,
        max_output_tokens,
        global_path: if gpath.exists() { Some(gpath) } else { None },
        project_path: ppath,
    })
}
