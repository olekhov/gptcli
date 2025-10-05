use anyhow::Result;
use tracing_subscriber::EnvFilter;
use clap::{Parser, Subcommand};

mod state;
mod fs;
mod commands;
mod db;

use commands::{init, scan, chunk, index, reindex_changed, stats, summarize, budget};

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
    #[arg(long, default_value_t=1200)] max_output: u32,
    #[arg(long)] system_file: Option<String>,
    #[arg(long, default_value="summarize.txt")] facts: String,
    },

    /// Показать бюджет
    Budget {},

}

fn main() -> Result<()> {
    // Инициализация журналирования в stderr
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Init { namespace } => init::run(namespace),
        Cmd::Scan {} => scan::run(),
        Cmd::Chunk {} => chunk::run(),
        Cmd::Index {} => index::run(),
        Cmd::ReindexChanged {} => reindex_changed::run(),
        Cmd::Stats {} => stats::run(),
        Cmd::Summarize { build_limit } => summarize::run(build_limit),
        Cmd::Budget {} => budget::run(),
    }
}
