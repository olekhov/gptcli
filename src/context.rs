use async_openai::config::OpenAIConfig;

use crate::{appconfig, db, fs::detect_project_root, state::ProjectState};


// маленький контейнер, без тяжёлых полей
pub struct AppCtx {
    pub root: std::path::PathBuf,
    pub state: ProjectState,     // namespace и т.п.
    pub eff: appconfig::Effective,  // api_base, api_key, model, lang, max_out
}

impl AppCtx {
    pub fn new() -> anyhow::Result<Self> {
        let root = detect_project_root()?;
        let state = ProjectState::load(&root)?;
        let eff = appconfig::load_effective(&root)?;
        Ok(Self { root, state, eff })
    }
    // лёгкие фабрики
    pub fn open_db(&self) -> anyhow::Result<rusqlite::Connection> {
        db::open_db(&self.root)
    }

    pub fn openai_client(&self) -> async_openai::Client<OpenAIConfig> {
        use async_openai::config::OpenAIConfig;
        let cfg = OpenAIConfig::new()
            .with_api_key(self.eff.api_key.clone())
            .with_api_base(self.eff.api_base.clone());
        async_openai::Client::with_config(cfg)
    }
}
