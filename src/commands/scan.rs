use anyhow::{Context, Result};
use ignore::{types::TypesBuilder, WalkBuilder};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{db::open_db, fs as ufs, state::ProjectState};

pub fn run() -> Result<()> {
    let root = ufs::detect_project_root()?;
    let st = ProjectState::load(&root)?;
    let mut conn = open_db(&root)?;

    // --- файловые типы (пока C/C++ + манифесты; расширим языковыми пакетами позже)
    let mut tb = TypesBuilder::new();
    for g in ["*.c","*.cc","*.cpp","*.cxx","*.h","*.hh","*.hpp","*.inl","*.ipp"] { tb.add("code", g)?; }
    for g in ["CMakeLists.txt","*.cmake","Makefile","meson.build","conanfile.*","vcpkg.json","compile_commands.json","README*","*.md"] {
        tb.add("meta", g)?;
    }
    let types = tb.select("code").select("meta").build()?;

    // --- исключения директорий (поверх .gitignore)
    let mut wb = WalkBuilder::new(&root);
    wb.types(types).hidden(false).follow_links(false).git_ignore(true);
    wb.filter_entry(|e| {
        let Some(name) = e.file_name().to_str() else { return true };
        if name == ".git" || name == ".gptcli" { return false; }
        if e.path().is_dir() {
            return !matches!(name,
                "build"|"out"|"dist"|"target"|"node_modules"|"__pycache__"|".cache"|".ccls-cache"|".venv"|"venv"
            ) && !name.starts_with("cmake-build-");
        }
        true
    });

    let mut files = 0usize;
    let mut bytes = 0u64;

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let tx = conn.transaction()?;

    /* transaction */
    {
        let mut upsert = tx.prepare(
            r#"INSERT INTO files(namespace,path,size,mtime,sha,lang_guess,doc_kind,seen_at)
            VALUES(?,?,?,?,?,?,?,?)
            ON CONFLICT(namespace,path) DO UPDATE SET
            size=excluded.size, mtime=excluded.mtime, sha=excluded.sha,
            lang_guess=excluded.lang_guess, doc_kind=excluded.doc_kind, seen_at=excluded.seen_at"#,
        )?;


        for dent in wb.build() {
            let Ok(entry) = dent else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(&root).unwrap().to_string_lossy().to_string();
            let md = entry.metadata().ok();
            let size = md.as_ref().map(|m| m.len() as i64).unwrap_or(0);
            let mtime = md
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let sha = sha256_file(path).unwrap_or_else(|_| String::new());
            let lang = guess_lang(&rel);
            let kind = classify_doc(&rel);

            upsert.execute(params![st.namespace, rel, size, mtime, sha, lang, kind, now])?;
            files += 1;
            bytes += size as u64;
        }

    }
    tx.commit()?;

    eprintln!("— scanned: {files} files, ~{} KB", bytes / 1024);
    Ok(())
}

fn sha256_file(p: &Path) -> Result<String> {
    let f = File::open(p).with_context(|| format!("open {}", p.display()))?;
    let mut r = BufReader::new(f);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// очень лёгкая эвристика; позже заменим языковыми адаптерами
fn guess_lang(rel: &str) -> &'static str {
    let rel = rel.to_ascii_lowercase();
    match () {
        _ if rel.ends_with(".rs") => "rust",
        _ if rel.ends_with(".py") => "python",
        _ if rel.ends_with(".ts") || rel.ends_with(".tsx") => "ts",
        _ if rel.ends_with(".js") || rel.ends_with(".jsx") => "js",
        _ if rel.ends_with(".lua") => "lua",
        _ if rel.ends_with(".c") => "c",
        _ if rel.ends_with(".cc") || rel.ends_with(".cpp") || rel.ends_with(".cxx")
            || rel.ends_with(".hh") || rel.ends_with(".hpp") || rel.ends_with(".inl") || rel.ends_with(".ipp") || rel.ends_with(".h") => "cpp",
        _ => "other",
    }
}

fn classify_doc(rel: &str) -> &'static str {
    println!("Classfying: {}", rel);
    let r = rel.to_ascii_lowercase();
    if r.ends_with(".md") || r.starts_with("docs/") { return "docs"; }
    if r == "cmakelists.txt" || r.ends_with(".cmake") || r == "makefile" || r == "meson.build"
        || r.ends_with("/cmakelists.txt")
        || r.starts_with("conanfile.") || r == "vcpkg.json" || r == "compile_commands.json" {
        return "manifest";
    }
    if r.contains("/test/") || r.contains("/tests/") || r.ends_with("_test.c") || r.ends_with("_test.cpp") {
        return "tests";
    }
    "code"
}
