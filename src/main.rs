use std::path::PathBuf;

use anyhow::Result;
use tracing_subscriber::EnvFilter;
use clap::{Parser, Subcommand};

mod state;
mod fs;
mod commands;
mod db;
mod context;

mod appconfig;

use commands::{init, scan, chunk, index, reindex_changed, stats, summarize, budget};

use crate::context::AppCtx;

#[derive(Parser)]
#[command(name="gptcli", version, about="Project-aware CLI for RAG + code edits")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Создать .gptcli и базу состояния
    Init { #[arg(long)] namespace: Option<String> },

    /// Просканировать дерево проекта
    Scan {},

    /// Разрезать файлы на логические чанки (пока заглушка)
    Chunk {},

    /// Записать чанки в БД / подготовить индекс (заглушка)
    Index {},

    /// Переиндексировать только изменённые (заглушка)
    ReindexChanged {},

    /// Показать статистику индекса/состояния
    Stats {},

    /// Сгенерировать секционный обзор проекта для LLM
    Summarize {
    #[arg(long)] llm: bool,
    #[arg(long, default_value="gpt-4.1-mini")] model: String,
    #[arg(long, default_value_t=1200)] max_output: usize,
    #[arg(long)] system_file: Option<String>,
    #[arg(long, default_value="summarize.txt")] facts: String,
    },

    /// Объяснить назначение и работу функции/класса
    Explain {
        #[arg(long)] symbol: Option<String>,    // напр. "net::TlsClient::handshake"
        #[arg(long)] file: Option<String>,      // относительный путь
        #[arg(long)] lines: Option<String>,     // "A:B"
        #[arg(long, default_value="gpt-4.1-mini")] model: String,
        #[arg(long, default_value_t=900)] max_output: u32,
        #[arg(long, default_value_t=15)] window: u32,   // контекст ±N строк
    },

    /// Показать бюджет
    Budget {},

    /// Конфигурация
    Config {
        /// Инициализировать
        #[arg(long)] init: bool,
        /// Показать
        #[arg(long)] show: bool,
    },

    /// Установить параметр конфигурации
    Set {
        key: String, value: String,
        #[arg(long)] profile: Option<String>,
    },

    /// Один запрос без контекста
    Oneshot {
        /// Инструкция, например "Ты средневековый повар"
        #[arg(long)] system: Option<String>,
        /// Запрос, например "Напиши рецепт супа из доступных продуктов"
        #[arg(long)] user: String,
        /// К запросу можно приложить файл
        #[arg(long)] file: Option<PathBuf>,
    },

    Whoami,

}

#[tokio::main]
async fn main() -> Result<()> {

    // Ignore result
    let _ = dotenvy::dotenv();

    // Инициализация журналирования в stderr
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    let ctx = AppCtx::new()?;
    
    match cli.cmd {
        Cmd::Init { namespace } => {
            init::run(&ctx, namespace)?;
        },
        Cmd::Scan {} => {
            scan::run(&ctx)?;
        },
        Cmd::Chunk {} => {
            chunk::run()?;
        },
        Cmd::Index {} => {
            index::run(&ctx)?;
        },
        Cmd::ReindexChanged {} => {
            reindex_changed::run()?;
        },
        Cmd::Stats {} => {
            stats::run()?;
        },
        Cmd::Summarize { llm, model, max_output, system_file, facts } => {
            if llm {
                summarize::run_llm(&ctx, model, max_output, system_file, facts).await?;
            } else {
                summarize::run(&ctx, max_output)?;
            }
        },
        Cmd::Explain { symbol, file, lines, model, max_output, window } => {
            commands::explain::run(&ctx, symbol, file, lines, model, max_output, window).await?;
        },
        Cmd::Budget {} => {
            budget::run().await?;
        },
        Cmd::Config { init, show } => {
            if init {
                commands::config_cmd::run(commands::config_cmd::ConfigSub::Init)?;
            } else if show {
                commands::config_cmd::run(commands::config_cmd::ConfigSub::Show)?;
}
        },
        Cmd::Set { key, value, profile } => {
            commands::set_cmd::run(&key, &value, profile.as_deref())?;
        },
        Cmd::Whoami => {
            commands::whoami::run().await?;
        },
        Cmd::Oneshot { system, user, file } => {
            commands::oneshot::run(&ctx, &system, &user, &file).await?;
        }
    }

    Ok(())
}
