use anyhow::{Context, Result};
use regex::Regex;
use rusqlite::params;
use std::{collections::BTreeMap, fs};

use async_openai::{
    types::responses::{Content, ContentType, CreateResponseArgs, Input, InputContent, InputItem, InputMessageArgs, InputMessageType, InputText, Response, Role}, Client
//    types::{ ResponseInput, InputContent, ResponseCreateArgs }
};

use time::OffsetDateTime;

use crate::{db::open_db, fs as ufs, state::ProjectState};

// Главная точка
pub fn run(build_limit: usize) -> Result<()> {
    let root = ufs::detect_project_root()?;
    let st = ProjectState::load(&root)?;
    let ns = &st.namespace;
    let conn = open_db(&root)?;

    let build = collect_build_facts(&conn, &root, ns, build_limit)?;
    let entry = collect_entry_points(&conn, ns)?;
    let stru  = collect_structure(&conn, ns)?;
    let todos = collect_todos(&conn, ns, 20)?;

    // Секционный текст под любую LLM
    println!("[BUILD]\n{}\n", build.trim());
    println!("[ENTRYPOINTS]\n{}\n", entry.trim());
    println!("[STRUCTURE]\n{}\n", stru.trim());
    println!("[TODOs]\n{}\n", todos.trim());
    Ok(())
}

// --- BUILD: вытягиваем только сигнальные директивы из CMake
fn collect_build_facts(conn: &rusqlite::Connection, root: &std::path::Path, ns: &str, limit: usize) -> Result<String> {
    let mut q = conn.prepare(
        "SELECT path FROM files
         WHERE namespace=?1 AND doc_kind='manifest'
           AND (lower(path)='cmakelists.txt' OR lower(path) LIKE '%.cmake') 
         ORDER BY path"
    )?;
    let rows = q.query_map(params![ns], |r| r.get::<_, String>(0))?;
    let re = Regex::new(r#"(?ix)
        ^\s*(?:project\s*\(|add_(?:executable|library)\s*\(|target_link_libraries\s*\(|find_package\s*\(|target_compile_features\s*\(|set\s*\(\s*CMAKE_CXX_STANDARD\b|target_include_directories\s*\(|include_directories\s*\(|add_subdirectory\s*\(|option\s*\()
    "#).unwrap();

    let mut out = Vec::<String>::new();
    let mut seen = std::collections::BTreeSet::<String>::new();

    for path in rows.flatten() {
        let text = fs::read_to_string(root.join(&path)).unwrap_or_default();
        for line in text.lines() {
            if re.is_match(line) {
                // нормализуем пробелы и убираем лишние кавычки
                let norm = normalize_cmake_line(line);
                if seen.insert(norm.clone()) {
                    out.push(format!("{}: {}", path, norm));
                    if out.len() >= limit { break; }
                }
            }
        }
        if out.len() >= limit { break; }
    }
    if out.is_empty() {
        Ok("— (нет CMake фактов либо не найдены сигнальные директивы)".into())
    } else {
        Ok(out.join("\n"))
    }
}

fn normalize_cmake_line(s: &str) -> String {
    let mut x = s.trim().to_string();
    // сводим множественные пробелы
    x = x.split_whitespace().collect::<Vec<_>>().join(" ");
    // мелкая чистка
    x
}

// --- ENTRYPOINTS: main() + примитивные маркеры тестов
fn collect_entry_points(conn: &rusqlite::Connection, ns: &str) -> Result<String> {
    let mut out = Vec::<String>::new();

    // main из ctags
    let mut q = conn.prepare(
        "SELECT f.path, t.line FROM tags t
           JOIN files f ON f.id=t.file_id
          WHERE f.namespace=?1 AND t.kind='function' AND t.name='main'
          ORDER BY f.path, t.line"
    )?;
    let mut rows = q.query(params![ns])?;
    while let Some(r) = rows.next()? {
        let path: String = r.get(0)?;
        let line: i64 = r.get(1)?;
        out.push(format!("main: {}:{}", path, line));
    }

    // простые тестовые маркеры из chunks (если уже есть)
    let tests_cnt: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks c JOIN files f ON f.id=c.file_id
         WHERE f.namespace=?1 AND (c.text LIKE '%TEST(' OR c.text LIKE '%TEST_CASE(' OR c.text LIKE '%Catch::Session%')",
        params![ns], |r| r.get(0)
    ).unwrap_or(0);
    if tests_cnt > 0 {
        out.push(format!("tests: ~{} chunks with test markers (gtest/catch2)", tests_cnt));
    }

    if out.is_empty() { Ok("— не найдено main()".into()) } else { Ok(out.join("\n")) }
}

// --- STRUCTURE: счётчики по каталогам и видам символов
fn collect_structure(conn: &rusqlite::Connection, ns: &str) -> Result<String> {
    // агрегируем по директориям (верхний уровень / два уровня)
    let mut q = conn.prepare(
        "SELECT f.path, t.kind FROM tags t
           JOIN files f ON f.id=t.file_id
          WHERE f.namespace=?1"
    )?;
    let mut rows = q.query(params![ns])?;

    let mut per_dir: BTreeMap<String, (i64,i64,i64)> = BTreeMap::new(); // dir -> (classes, functions, namespaces)
    while let Some(r) = rows.next()? {
        let path: String = r.get(0)?;
        let kind: String = r.get(1)?;
        let dir = short_dir(&path);
        let e = per_dir.entry(dir).or_insert((0,0,0));
        match kind.as_str() {
            "class" | "struct" => e.0 += 1,
            "function" | "member" | "prototype" => e.1 += 1,
            "namespace" => e.2 += 1,
            _ => {}
        }
    }
    // выводим топ-10
    let mut v: Vec<_> = per_dir.into_iter().collect();
    v.sort_by_key(|(_, (c,f,n))| -(c+f+n));
    let mut out = Vec::new();
    for (i,(d,(c,f,n))) in v.into_iter().enumerate() {
        if i>=10 { break; }
        out.push(format!("{d}: classes={c}, funcs={f}, namespaces={n}"));
    }
    if out.is_empty() { Ok("— нет тегов (запусти index)".into()) } else { Ok(out.join("\n")) }
}

fn short_dir(path: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop(); // drop filename
    while parts.len()>2 { parts.remove(0); } // оставим 1–2 уровня для читабельности
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

// --- TODOs: простая выборка из чанков
fn collect_todos(conn: &rusqlite::Connection, ns: &str, limit: usize) -> Result<String> {
    let mut q = conn.prepare(
        "SELECT f.path, c.begin_line
           FROM chunks c JOIN files f ON f.id=c.file_id
          WHERE f.namespace=?1 AND (c.text LIKE '%TODO%' OR c.text LIKE '%FIXME%' OR c.text LIKE '%HACK%')
          ORDER BY f.path, c.begin_line LIMIT ?2"
    )?;
    let mut rows = q.query(params![ns, limit as i64])?;
    let mut out = Vec::<String>::new();
    while let Some(r) = rows.next()? {
        let path: String = r.get(0)?;
        let line: i64 = r.get(1)?;
        out.push(format!("{}:{}", path, line));
    }
    if out.is_empty() { Ok("— не найдено TODO/FIXME/HACK".into()) } else { Ok(out.join("\n")) }
}


use async_openai::types::{responses::OutputContent};

fn extract_output_text(resp: &Response) -> String {
    if let Some(t) = resp.output_text.clone() {
        return t;
    }
    let mut parts = Vec::new();
    for oc in &resp.output {
        if let OutputContent::Message(msg) = oc {
            // msg.content: Vec<...>; ищем текстовые куски
            for item in &msg.content {
                // В новых типах это обычно вариант с названием наподобие `OutputText { text }`.
                // У некоторых версий есть удобный метод вида `item.output_text()`.
                match item {

                    Content::OutputText(output_text) => {
                        parts.push(output_text.text.clone());
                    },
                    Content::Refusal(refusal) => todo!(),
                    /*
                    // если есть удобный геттер:
                    _ if item.output_text().is_some() => {
                        parts.push(item.output_text().unwrap().to_string());
                    }
                    // либо матч на явный вариант (название может отличаться в минорных версиях):
                    async_openai::types::MessageContent::OutputText(t) => {
                        parts.push(t.text.clone());
                    }
                    _ => {}
                    */
                }
            }
        }
    }
    parts.join("\n")
}

pub async fn run_llm(model: String, max_output: usize, system_file: Option<String>, facts_path: String) -> Result<()> {
    // 1) читаем данные
    let facts = fs::read_to_string(&facts_path)
        .with_context(|| format!("read {}", facts_path))?;
    let system = if let Some(p) = system_file {
        fs::read_to_string(&p).context("read system_file")?
    } else {
        // дефолтная короткая инструкция
        "Ты — технический обзорщик C/C++ проектов. Пиши кратко и структурировано. Не выдумывай: опирайся только на предоставленные секции [BUILD]/[ENTRYPOINTS]/[STRUCTURE]/[TODOs]. Вывод: 1) краткое описание; 2) сборка (список); 3) модули и ответственность; 4) внешние зависимости и зачем; 5) тесты/инфраструктура; 6) риски/технический долг (списком).".to_string()
    };


    let system_msg = InputItem::Message(
        InputMessageArgs::default()
            .kind(InputMessageType::Message)                // можно опустить: Default
            .role(Role::System)
            .content(InputContent::TextInput(system.clone())) // <-- оборачиваем текст
            .build()?
    );

    let user_msg = InputItem::Message(
        InputMessageArgs::default()
            .role(Role::User)
            .content(InputContent::TextInput(
                format!("Ниже факты о проекте (BUILD/ENTRYPOINTS/STRUCTURE/TODOs). Подготовь обзор.\n{}", &facts)
            ))
            .build()?
    );


    // 2) соберём объект запроса (Responses API)
    let input :Vec<InputItem> = vec![ system_msg, user_msg ];
        /*
    {
            role: "system".into(),
            content: vec![InputContent::input_text(system.clone())],
        },
        ResponseInput {
            role: "user".into(),
            content: vec![
                InputContent::input_text("Ниже факты о проекте (BUILD/ENTRYPOINTS/STRUCTURE/TODOs). Подготовь обзор."),
                InputContent::input_text(facts.clone()),
            ],
        },
        */

    let args = CreateResponseArgs::default()
        .model(model.clone())
        .max_output_tokens(max_output as u32)
        .input(Input::Items(input))
        .build()?;

    // 3) сохраним сырой запрос в /tmp
    let ts = OffsetDateTime::now_utc().unix_timestamp();
    let req_path = format!("/tmp/gptcli-request-{}-{}.json", model, ts);
    let resp_path = format!("/tmp/gptcli-response-{}-{}.json", model, ts);
    fs::write(&req_path, serde_json::to_vec_pretty(&args)?)?;

    // 4) вызов
    let client = Client::new(); // использует OPENAI_API_KEY из окружения
    let resp = client.responses().create(args).await?;

    // 5) лог сырых ответов
    fs::write(&resp_path, serde_json::to_vec_pretty(&resp)?)?;

    // 6) вытащим текст и usage
    let text = extract_output_text(&resp);

    // usage может отсутствовать — учитываем это
    let (pt, ct, tt) = if let Some(u) = resp.usage {
        (u.input_tokens, u.output_tokens, u.total_tokens)
    } else { (0,0,0) };

    println!("{text}\n");
    eprintln!("— usage: prompt={pt}, completion={ct}, total={tt}");
    eprintln!("— raw request: {req_path}");
    eprintln!("— raw response: {resp_path}");
    Ok(())
}
