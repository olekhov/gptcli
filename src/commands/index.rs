use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use time::{OffsetDateTime};

use crate::{db::open_db, fs as ufs, state::ProjectState};

#[derive(Debug, Deserialize, Clone)]
struct CtagsTag {
    name: String,
    path: String,
    #[serde(default)]
    kind: String,                    // "function" | "class" | ... (с +K long kind)
    #[serde(default)]
    language: Option<String>,        // "C" | "C++"
    #[serde(default)]
    line: Option<u32>,
    #[serde(default, rename="end")]
    end_line: Option<u32>,           // если сборка ctags умеет
    #[serde(default)]
    scope: Option<String>,           // ns::Class
    #[serde(default, rename="scopeKind")]
    scope_kind: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default, rename="typeref")]
    type_ref: Option<String>,
}

#[derive(Debug)]
struct PendingFile {
    id: i64,
    rel_path: String,
    sha: String,
    mtime: i64,
}

pub fn run() -> Result<()> {
    let root = ufs::detect_project_root()?;
    let st = ProjectState::load(&root)?;
    let mut conn = open_db(&root)?;

    let pending = pending_files(&conn, &st.namespace)?;
    if pending.is_empty() {
        println!("index: up-to-date (нет изменённых файлов)");
        return Ok(());
    }

    // Список путей для ctags (относительно корня)
    let paths: Vec<String> = pending.iter().map(|p| p.rel_path.clone()).collect();
    let tags = run_ctags(&root, &paths).context("ctags failed")?;

    // Группируем теги по пути
    let mut by_path: HashMap<String, Vec<CtagsTag>> = HashMap::new();
    for t in tags {
        if t.line.is_none() { continue; }
        let k = t.kind.as_str();
        // фильтруем только полезные для чанкинга
        if !matches!(k, "function" | "class" | "struct" | "namespace" | "prototype" | "member" | "enum" | "union" | "typedef") {
            continue;
        }
        by_path.entry(t.path.clone()).or_default().push(t);
    }

    for v in by_path.values_mut() {
        v.sort_by_key(|t| t.line.unwrap_or(0));
    }

    let now = OffsetDateTime::now_utc().unix_timestamp();

    // Транзакция на весь батч
    let tx = conn.transaction()?;
    {
        let mut del_tags   = tx.prepare("DELETE FROM tags WHERE file_id=?1")?;
        let mut del_chunks = tx.prepare("DELETE FROM chunks WHERE file_id=?1")?;
        let mut ins_tag = tx.prepare(
            "INSERT INTO tags(file_id,name,kind,line,scope,scope_kind,signature,lang,end_line)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
        let mut ins_chunk = tx.prepare(
            "INSERT INTO chunks(file_id,kind,symbol,begin_line,end_line,sha,mtime,text)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")?;
        let mut upd_file = tx.prepare(
            "UPDATE files SET indexed_sha=?1, indexed_at=?2 WHERE id=?3")?;

        let total = pending.len();

        for (idx, pf) in pending.into_iter().enumerate() {
            println!("Indexing {}/{} : {}", idx+1, total, &pf.rel_path);
            // читаем текст файла (для чанков)
            let abs = root.join(&pf.rel_path);
            let file_text = match read_text_sanitized(&abs) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("warn: не удалось прочитать {}: {e}", abs.display());
                    continue;
                }
            };
            let total_lines = (file_text.lines().count() as i64).max(1);

            // теги по файлу
            let ftags = by_path.get(pf.rel_path.as_str()).map(|v| v.as_slice()).unwrap_or(&[]);
            // пересоздаём индексацию
            del_tags.execute(params![pf.id])?;
            del_chunks.execute(params![pf.id])?;

            // вставляем теги
            for t in ftags {
                ins_tag.execute(params![
                    pf.id,
                    t.name,
                    t.kind,
                    t.line.unwrap_or(0) as i64,
                    t.scope,
                    t.scope_kind,
                    t.signature,
                    t.language.as_deref().unwrap_or(""),
                    t.end_line.map(|x| x as i64),
                ])?;
            }

            // строим чанки v1
            let chunk_specs = build_chunks_v1(ftags, total_lines);
            for c in chunk_specs {
                let text = slice_text(&file_text, c.begin_line, c.end_line);
                let sha = sha256_str(&text);
                let symbol = c.symbol;
                ins_chunk.execute(params![
                    pf.id,
                    c.kind,
                    symbol,
                    c.begin_line,
                    c.end_line,
                    sha,
                    pf.mtime,
                    text,
                ])?;
            }

            // отметить файл как проиндексированный
            upd_file.execute(params![pf.sha, now, pf.id])?;
        }
    } // statements drop here

    tx.commit()?;
    println!("index: ok");
    Ok(())
}

// -------- helpers --------

fn pending_files(conn: &Connection, ns: &str) -> Result<Vec<PendingFile>> {
    let mut q = conn.prepare(
        "SELECT id, path, COALESCE(sha,''), COALESCE(mtime,0)
           FROM files
          WHERE namespace=?1
            AND (indexed_sha IS NULL OR indexed_sha != sha)
          ORDER BY path"
    )?;
    let mut rows = q.query(params![ns])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(PendingFile {
            id: r.get(0)?,
            rel_path: r.get(1)?,
            sha: r.get(2)?,
            mtime: r.get(3)?,
        });
    }
    Ok(out)
}

fn run_ctags(project_root: &Path, paths: &[String]) -> Result<Vec<CtagsTag>> {
    // запускаем из корня проекта, чтобы относительные пути совпадали с теми, что в БД
    let mut child = Command::new("ctags");
    child.current_dir(project_root);
    child
        .args([
            "-n",
            "--output-format=json",
            "--languages=C,C++",
            "--fields=+KlnSmt",
            "--extras=+F",
            "--sort=no",
            "-L",
            "-",
            "-f",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());

    let mut child = child.spawn().context("spawn ctags")?;
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for p in paths {
            writeln!(stdin, "{p}")?;
        }
    }
    let out = child.wait_with_output().context("ctags output")?;
    if !out.status.success() {
        anyhow::bail!("ctags exited with {}", out.status);
    }

    let mut tags = Vec::new();
    for line in out.stdout.split(|b| *b == b'\n') {
        if line.is_empty() { continue; }
        // universal-ctags JSON: NDJSON, один объект на строку
        // есть строки meta (kind: "tag") и т.п.; фильтруем десериализацией
        if let Ok(tag) = serde_json::from_slice::<CtagsTag>(line) {
            tags.push(tag);
        }
    }
    Ok(tags)
}

// Простая версия чанкинга: один тег → один чанк
#[derive(Debug)]
struct ChunkSpec {
    kind: String,
    symbol: Option<String>,
    begin_line: i64, // 1-based
    end_line: i64,   // inclusive
}

fn build_chunks_v1(tags: &[CtagsTag], total_lines: i64) -> Vec<ChunkSpec> {
    let mut out = Vec::new();
    if tags.is_empty() {
        // нет тегов — пока пропускаем (можно добавить fallback блоки позже)
        return out;
    }
    for (i, t) in tags.iter().enumerate() {
        let begin = t.line.unwrap_or(1) as i64;
        let end = if let Some(e) = t.end_line { e as i64 }
                  else if let Some(next) = tags.get(i + 1) {
                      (next.line.unwrap_or((begin + 1) as u32) as i64 - 1).max(begin)
                  } else {
                      total_lines
                  };
        let sym = if let Some(scope) = &t.scope {
            Some(format!("{scope}::{}", t.name))
        } else {
            Some(t.name.clone())
        };
        let kind = match t.kind.as_str() {
            "function" | "prototype" | "member" => "function",
            "class" | "struct" => "class",
            "namespace" => "namespace",
            "enum" => "enum",
            "union" => "union",
            "typedef" => "typedef",
            _ => "block",
        }.to_string();

        // защита от мусора
        if begin <= 0 || end < begin { continue; }

        out.push(ChunkSpec {
            kind,
            symbol: sym,
            begin_line: begin,
            end_line: end,
        });
    }
    out
}

fn sanitize_non_utf8_runs(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut in_non_ascii = false;
    for &b in bytes {
        match b {
            b'\n' | b'\t' | b'\r' => {
                if in_non_ascii { out.push_str("???"); in_non_ascii = false; }
                out.push(b as char);
            }
            0x20..=0x7E => { // печатный ASCII
                if in_non_ascii { out.push_str("???"); in_non_ascii = false; }
                out.push(b as char);
            }
            _ => { in_non_ascii = true; }
        }
    }
    if in_non_ascii { out.push_str("???"); }
    out
}

fn read_text_sanitized(path: &std::path::Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    // Если это валидный UTF-8 — не трогаем
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.to_string());
    }
    Ok(sanitize_non_utf8_runs(&bytes))
}

fn slice_text(full: &str, begin_line: i64, end_line: i64) -> String {
    // берём [begin-1, end) построчно; сохраняем разделители строк как '\n'
    let mut res = String::new();
    for (idx, line) in full.lines().enumerate() {
        let ln = (idx as i64) + 1;
        if ln < begin_line { continue; }
        if ln > end_line { break; }
        res.push_str(line);
        res.push('\n');
    }
    res
}

fn sha256_str(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

