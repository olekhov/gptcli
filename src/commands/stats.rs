use anyhow::Result;
use rusqlite::params;
use std::fs;
use crate::{db::open_db, fs as ufs, state::ProjectState};

pub fn run() -> Result<()> {
    let root = ufs::detect_project_root()?;
    let st = ProjectState::load(&root)?;
    let ns = &st.namespace;
    let conn = open_db(&root)?;

    // --- размеры и числа
    let db_path = root.join(".gptcli/index.sqlite");
    let db_bytes = fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    let (files_total, bytes_total):(i64,i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(size),0) FROM files WHERE namespace=?1",
        params![ns],
        |r| Ok((r.get(0)?, r.get(1)?))
    )?;

    let (indexed_ok, pending):(i64,i64) = conn.query_row(
        "SELECT \
           SUM(CASE WHEN indexed_sha IS NOT NULL AND indexed_sha = sha THEN 1 ELSE 0 END), \
           SUM(CASE WHEN indexed_sha IS NULL OR indexed_sha != sha THEN 1 ELSE 0 END) \
         FROM files WHERE namespace=?1",
        params![ns],
        |r| Ok((r.get(0)?, r.get(1)?))
    )?;

    let tags_cnt: i64 = conn.query_row(
        "SELECT COALESCE(COUNT(*),0) \
           FROM tags t JOIN files f ON f.id=t.file_id \
          WHERE f.namespace=?1",
        params![ns],
        |r| r.get(0)
    )?;

    let (chunks_cnt, chunk_text_bytes):(i64,i64) = conn.query_row(
        "SELECT COALESCE(COUNT(*),0), COALESCE(SUM(length(text)),0) \
           FROM chunks c JOIN files f ON f.id=c.file_id \
          WHERE f.namespace=?1",
        params![ns],
        |r| Ok((r.get(0)?, r.get(1)?))
    )?;

    let (seen_max, indexed_max):(Option<i64>,Option<i64>) = conn.query_row(
        "SELECT MAX(seen_at), MAX(indexed_at) FROM files WHERE namespace=?1",
        params![ns],
        |r| Ok((r.get(0)?, r.get(1)?))
    )?;

    // --- распределение по doc_kind
    let mut kinds_stmt = conn.prepare(
        "SELECT doc_kind, COUNT(*) FROM files WHERE namespace=?1 GROUP BY doc_kind ORDER BY COUNT(*) DESC"
    )?;
    let mut kinds_rows = kinds_stmt.query(params![ns])?;
    let mut kinds: Vec<(String,i64)> = Vec::new();
    while let Some(row) = kinds_rows.next()? {
        kinds.push((row.get::<_,String>(0)?, row.get::<_,i64>(1)?));
    }

    // --- вывод
    println!("Namespace: {}", ns);
    println!("DB: {} ({})", db_path.display(), human_size(db_bytes));
    println!("Files: {} total | {} indexed | {} pending | size ~{}",
        files_total, indexed_ok, pending, human_size(bytes_total as u64)
    );
    if !kinds.is_empty() {
        print!("Kinds: ");
        for (i,(k,c)) in kinds.iter().enumerate() {
            if i>0 { print!(", "); }
            print!("{k}:{c}");
        }
        println!();
    }
    println!("Tags: {}", tags_cnt);
    println!("Chunks: {} (text ~{})", chunks_cnt, human_size(chunk_text_bytes as u64));
    println!("Last seen_at: {}", seen_max.map(fmt_ts).unwrap_or_else(|| "-".into()));
    println!("Last indexed_at: {}", indexed_max.map(fmt_ts).unwrap_or_else(|| "-".into()));

    Ok(())
}

// --- утилиты

fn human_size(n: u64) -> String {
    const UNITS: [&str; 6] = ["B","KB","MB","GB","TB","PB"];
    if n == 0 { return "0 B".into(); }
    let i = ( (n as f64).ln() / 1024_f64.ln() ).floor() as usize;
    let i = i.min(UNITS.len()-1);
    let v = (n as f64) / 1024_f64.powi(i as i32);
    if v >= 100.0 { format!("{:.0} {}", v, UNITS[i]) }
    else if v >= 10.0 { format!("{:.1} {}", v, UNITS[i]) }
    else { format!("{:.2} {}", v, UNITS[i]) }
}

fn fmt_ts(secs: i64) -> String {
    // печатаем простой ISO UTC без зависимостей
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};
    match OffsetDateTime::from_unix_timestamp(secs) {
        Ok(dt) => dt.format(&Rfc3339).unwrap_or_else(|_| secs.to_string()),
        Err(_) => secs.to_string(),
    }
}

