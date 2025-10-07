use anyhow::{bail, Context, Result};
use async_openai::{
    types::responses::{ContentType, CreateResponseArgs, Input, InputContent, InputItem, InputMessageArgs, InputMessageType, InputText, Role, Usage}, Client
};
use regex::Regex;
use rusqlite::{fallible_streaming_iterator::FallibleStreamingIterator, params, Connection};
use std::{fs, path::{Path, PathBuf}};
use time::OffsetDateTime;

use crate::{commands::extract_output_text, db::open_db, fs as ufs, state::ProjectState};

pub async fn run(
    symbol: Option<String>,
    file: Option<String>,
    lines: Option<String>,
    model: String,
    max_output: u32,
    window: u32,
) -> Result<()> {
    let root = ufs::detect_project_root()?;
    let st   = ProjectState::load(&root)?;
    let ns   = st.namespace.clone();
    let conn = open_db(&root)?;

    // 1) Определяем цель
    let tgt = resolve_target(&conn, &root, &ns, symbol.as_deref(), file.as_deref(), lines.as_deref())?
        .context("не удалось определить цель (symbol/lines)")?;

    // 2) Собираем контекстные секции
    let decl_def   = section_decl_def(&root, &tgt, window as i64)?;
    let class_type = section_class_type(&conn, &root, &ns, &tgt, window as i64)?;
    let pp         = section_preproc(&root, &tgt, 30)?;
    let callees    = section_callees(&conn, &root, &ns, &tgt, 12)?;
    let usage      = section_usage_examples(&conn, &ns, &tgt.name, 3)?;
    let comments   = section_comments(&root, &tgt, 12)?;

    // 3) Формируем секционный prompt
    let system = "Ты — senior C/C++ reviewer. Объясняй по фактам, кратко и структурированно. Не выдумывай.
Структура ответа: Назначение; Как работает; Ввод/вывод и инварианты; Ошибки/исключения;
Потоки/память/реентерабельность; Сложность/перф; Примеры применения; Риски/краевые случаи.";

    let facts = format!(r#"[TARGET]
name: {name}
file: {path}:{bl}-{el}
kind: {kind}
signature: {sig}

[DECL/DEF]
{decl_def}

[CLASS/TYPE]
{class_type}

[PREPROCESSOR]
{pp}

[CALLEES]
{callees}

[USAGE]
{usage}

[COMMENTS]
{comments}

[ASK]
Дай обзор по структуре из system. Если данных недостаточно — явно отметь «не найдено» в соответствующих секциях."#,
        name=tgt.fqn.as_deref().unwrap_or(&tgt.name),
        path=tgt.path, bl=tgt.begin_line, el=tgt.end_line,
        kind=tgt.kind, sig=tgt.signature.unwrap_or_default(),
        decl_def=decl_def, class_type=class_type, pp=pp, callees=callees,
        usage=usage, comments=comments
    );

    // 4) Запрос к OpenAI (Responses API через async-openai) + лог в /tmp
    let (text, usage, req_path, resp_path) = call_openai(model, max_output, &facts, system).await?;

    println!("{text}\n");
    eprintln!("— raw request:  {req_path}");
    eprintln!("— raw response: {resp_path}");
    Ok(())
}

/* ---------- target resolve ---------- */

#[derive(Debug, Clone)]
struct Target {
    file_id: i64,
    path: String,
    name: String,             // короткое имя
    fqn: Option<String>,      // scope::name
    kind: String,             // function|class|...
    begin_line: i64,
    end_line: i64,
    signature: Option<String>,
}

fn resolve_target(
    conn: &Connection,
    root: &Path,
    ns: &str,
    symbol: Option<&str>,
    file: Option<&str>,
    lines: Option<&str>,
) -> Result<Option<Target>> {
    if let Some(sym) = symbol {
        // FQN или короткое имя
        let (scope, name) = split_fqn(sym);
        let mut q = conn.prepare(
            "SELECT t.rowid, f.path, t.name, t.kind, t.line, COALESCE(t.end_line,0), t.scope, t.signature
               FROM tags t
               JOIN files f ON f.id=t.file_id
              WHERE f.namespace=?1
                AND (t.name=?2 OR (t.scope IS NOT NULL AND (t.scope||'::'||t.name)=?3))
              ORDER BY (CASE WHEN t.scope IS NULL THEN 1 ELSE 0 END), f.path, t.line
              LIMIT 1"
        )?;
        let mut rows = q.query(params![ns, name, sym])?;
        if let Some(r) = rows.next()? {
            let file_id: i64 = r.get(0)?;
            let path: String = r.get(1)?;
            let name: String = r.get(2)?;
            let kind: String = r.get(3)?;
            let line: i64    = r.get(4)?;
            let mut end: i64 = r.get(5)?;
            let scope: Option<String> = r.get(6)?;
            let sig: Option<String>   = r.get(7)?;
            if end <= 0 {
                end = approx_end_line(conn, ns, &path, line)?;
            }
            return Ok(Some(Target{
                file_id, path, name: name.clone(),
                fqn: scope.map(|s| format!("{s}::{name}")),
                kind, begin_line: line, end_line: end, signature: sig,
            }));
        }
    }
    if let (Some(p), Some(rng)) = (file, lines) {
        let (a,b) = parse_range(rng)?;
        let mut qf = conn.prepare("SELECT id FROM files WHERE namespace=?1 AND path=?2 LIMIT 1")?;
        let file_id: i64 = qf.query_row(params![ns,p], |r| r.get(0))
            .with_context(|| format!("file not indexed: {p}"))?;
        // nearest tag starting at/above A
        let mut qt = conn.prepare(
            "SELECT name,kind,line,COALESCE(end_line,0),scope,signature
               FROM tags WHERE file_id=?1 AND line<=?2
               ORDER BY line DESC LIMIT 1"
        )?;

        let mut rows = qt.query(rusqlite::params![file_id, a])?;

        if let Some(r) = rows.next()? {
            let name: String = r.get(0)?;
            let kind: String = r.get(1)?;
            let line: i64    = r.get(2)?;
            let mut end: i64 = r.get(3)?;
            let scope: Option<String> = r.get(4)?;
            let sig:   Option<String> = r.get(5)?;

            if end <= 0 {
                // теперь это выполняется в функции с anyhow::Result — ? легален
                end = b.max(approx_end_line(conn, ns, p, line)?);
            }

            return Ok(Some(Target {
                file_id,
                path: p.to_string(),
                name: name.clone(),
                fqn: scope.map(|s| format!("{s}::{}", name)),
                kind,
                begin_line: line,
                end_line: end,
                signature: sig,
            }));
        } else {
            // нет тега — используем прямой диапазон
            return Ok(Some(Target {
                file_id,
                path: p.to_string(),
                name: "<range>".into(),
                fqn: None,
                kind: "block".into(),
                begin_line: a,
                end_line: b,
                signature: None,
            }));
        }

    }
    Ok(None)
}

fn split_fqn(s: &str) -> (Option<&str>, &str) {
    if let Some(pos) = s.rfind("::") { (Some(&s[..pos]), &s[pos+2..]) } else { (None, s) }
}

fn parse_range(s: &str) -> Result<(i64,i64)> {
    let parts: Vec<_> = s.split(':').collect();
    if parts.len()!=2 { bail!("lines must be A:B"); }
    let a: i64 = parts[0].parse()?; let b: i64 = parts[1].parse()?;
    Ok((a.min(b), a.max(b)))
}

fn approx_end_line(conn:&Connection, ns:&str, path:&str, begin:i64) -> Result<i64> {
    // следующий тег − 1, иначе "конец файла"
    let mut q = conn.prepare(
        "SELECT COALESCE(MIN(line),0) FROM tags t
           JOIN files f ON f.id=t.file_id
          WHERE f.namespace=?1 AND f.path=?2 AND t.line>?3"
    )?;
    let next: i64 = q.query_row(params![ns,path,begin], |r| r.get(0))?;
    if next>0 { Ok(next-1) } else {
        // конец по числу строк в файле
        let full = read_text_sanitized(&ufs::detect_project_root()?.join(path))?;
        Ok(full.lines().count() as i64)
    }
}

/* ---------- sections ---------- */

fn section_decl_def(root:&Path, tgt:&Target, win:i64) -> Result<String> {
    let txt = read_text_sanitized(&root.join(&tgt.path))?;
    Ok(slice_lines(&txt, (tgt.begin_line-win).max(1), tgt.end_line+win))
}

fn section_class_type(conn:&Connection, root:&Path, ns:&str, tgt:&Target, win:i64) -> Result<String> {
    // если есть scope "A::B", возьмём последний компонент как имя класса/пространства
    let class_name = tgt.fqn.as_ref()
        .and_then(|fqn| fqn.rsplit("::").nth(1)) // компонент перед именем
        .map(|s| s.to_string());
    if class_name.is_none() { return Ok("—".into()); }
    let cls = class_name.unwrap();
    let mut q = conn.prepare(
        "SELECT f.path, t.line, COALESCE(t.end_line,0)
           FROM tags t JOIN files f ON f.id=t.file_id
          WHERE f.namespace=?1 AND t.kind IN ('class','struct') AND t.name=?2
          ORDER BY (CASE WHEN f.path LIKE '%.hpp' THEN 0 ELSE 1 END), f.path LIMIT 1"
    )?;
    let row = q.query_row(params![ns,&cls], |r| Ok((r.get::<_,String>(0)?, r.get::<_,i64>(1)?, r.get::<_,i64>(2)?)));
    if let Ok((path, line, mut end)) = row {
        if end<=0 { end = approx_end_line(conn, ns, &path, line)?; }
        let txt = read_text_sanitized(&root.join(path))?;
        return Ok(slice_lines(&txt, (line-win).max(1), end+win));
    }
    Ok("—".into())
}

fn section_preproc(root:&Path, tgt:&Target, span:i64) -> Result<String> {
    let txt = read_text_sanitized(&root.join(&tgt.path))?;
    let slice = slice_lines(&txt, (tgt.begin_line-span).max(1), tgt.end_line+span);
    let out = slice.lines().filter(|l| l.trim_start().starts_with('#')).take(30).collect::<Vec<_>>().join("\n");
    Ok(if out.is_empty() {"—".into()} else {out})
}

fn section_callees(conn:&Connection, root:&Path, ns:&str, tgt:&Target, limit:usize) -> Result<String> {
    let txt = read_text_sanitized(&root.join(&tgt.path))?;
    let body = slice_lines(&txt, tgt.begin_line, tgt.end_line);
    let re = Regex::new(r#"(?x)\b([A-Za-z_][\w:<>]*)\s*\("#).unwrap();
    let mut names = Vec::<String>::new();
    for cap in re.captures_iter(&body) {
        let n = cap.get(1).unwrap().as_str();
        if ["if","for","while","switch","return","sizeof","static_cast","dynamic_cast","new","delete"].contains(&n) { continue; }
        if !names.iter().any(|x| x==n) { names.push(n.to_string()); }
        if names.len()>=limit { break; }
    }
    if names.is_empty() { return Ok("—".into()); }

    // найдём сигнатуры по имени (короткому)
    let mut out = Vec::new();
    let mut qs = conn.prepare(
        "SELECT DISTINCT t.name, t.scope, t.signature
           FROM tags t JOIN files f ON f.id=t.file_id
          WHERE f.namespace=?1 AND t.name=?2 AND t.kind IN ('function','prototype','member')
          LIMIT 3"
    )?;
    for n in names {
        let mut rows = qs.query(params![ns, n])?;
        let mut lines = Vec::new();
        while let Some(r) = rows.next()? {
            let name:String = r.get(0)?; let scope:Option<String>=r.get(1)?;
            let sig:Option<String> = r.get(2)?;
            let fqn = scope.map(|s| format!("{s}::{name}")).unwrap_or(name);
            lines.push(format!("• {}{}", fqn, sig.as_deref().unwrap_or("")));
        }
        if !lines.is_empty() {
            out.push(lines.join(" | "));
        }
    }
    Ok(out.join("\n"))
}

fn section_usage_examples(conn:&Connection, ns:&str, symbol:&str, limit:usize) -> Result<String> {
    // ищем в тестовых чанках упоминания имени символа
    let like = format!("%{}%", symbol);
    let mut q = conn.prepare(
        "SELECT f.path, c.begin_line
           FROM chunks c JOIN files f ON f.id=c.file_id
          WHERE f.namespace=?1
            AND (f.path LIKE '%test%' OR f.path LIKE '%tests%' OR f.doc_kind='tests')
            AND c.text LIKE ?2
          ORDER BY f.path, c.begin_line
          LIMIT ?3"
    )?;
    let mut rows = q.query(params![ns, like, limit as i64])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let path:String=r.get(0)?; let line:i64=r.get(1)?;
        out.push(format!("• {}:{}", path, line));
    }
    Ok(if out.is_empty() { "—".into() } else { out.join("\n") })
}

fn section_comments(root:&Path, tgt:&Target, up:i64) -> Result<String> {
    let txt = read_text_sanitized(&root.join(&tgt.path))?;
    let start = (tgt.begin_line - up).max(1);
    let head = slice_lines(&txt, start, tgt.begin_line);
    // возьмём только комментарии
    let mut out = Vec::new();
    for l in head.lines().rev().take(40) {
        let t = l.trim_start();
        if t.starts_with("//") || t.starts_with("/*") || t.starts_with("*") || t.starts_with("*/") {
            out.push(l.to_string());
        } else if !out.is_empty() {
            break; // как только встретили не-комментарий — прекращаем (берём ближайший блок)
        }
    }
    out.reverse();
    Ok(if out.is_empty() { "—".into() } else { out.join("\n") })
}

/* ---------- OpenAI call + logging ---------- */

async fn call_openai(model:String, max_output:u32, facts:&str, system:&str)
-> Result<(String, Option<Usage>, String, String)> {
    // messages → Input

    let system_msg = InputItem::Message(
        InputMessageArgs::default()
            .kind(InputMessageType::Message)                // можно опустить: Default
            .role(Role::System)
            .content(InputContent::TextInput(system.to_string())) // <-- оборачиваем текст
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


    let args = CreateResponseArgs::default()
        .model(model.clone())
        .max_output_tokens(max_output as u32)
        .input(Input::Items(input))
        .build()?;


    // лог в /tmp
    let ts = OffsetDateTime::now_utc().unix_timestamp();
    let req_path  = format!("/tmp/gptcli-explain-req-{}-{}.json", model, ts);
    let resp_path = format!("/tmp/gptcli-explain-resp-{}-{}.json", model, ts);
    fs::write(&req_path, serde_json::to_vec_pretty(&args)?)?;

    let client = Client::new();
    let resp = client.responses().create(args).await?;
    fs::write(&resp_path, serde_json::to_vec_pretty(&resp)?)?;

    let text = extract_output_text(&resp);
    // usage может отсутствовать — учитываем это
    let (pt, ct, tt) = if let Some(ref u) = resp.usage {
        (u.input_tokens, u.output_tokens, u.total_tokens)
    } else { (0,0,0) };

    println!("{text}\n");
    eprintln!("— usage: prompt={pt}, completion={ct}, total={tt}");
    eprintln!("— raw request: {req_path}");
    eprintln!("— raw response: {resp_path}");
    Ok((text, resp.usage.clone(), req_path, resp_path))
}

/* ---------- text utils (sanitizer + slicing) ---------- */

fn read_text_sanitized(path: &Path) -> Result<String> {
    let bytes = fs::read(path)
        .with_context(|| format!("read {}", path.display()))?;
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.to_string());
    }
    Ok(sanitize_non_utf8_runs(&bytes))
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
            0x20..=0x7E => {
                if in_non_ascii { out.push_str("???"); in_non_ascii = false; }
                out.push(b as char);
            }
            _ => { in_non_ascii = true; }
        }
    }
    if in_non_ascii { out.push_str("???"); }
    out
}

fn slice_lines(full:&str, begin:i64, end:i64) -> String {
    let mut res = String::new();
    for (idx, line) in full.lines().enumerate() {
        let ln = (idx as i64)+1;
        if ln < begin { continue; }
        if ln > end { break; }
        res.push_str(line);
        res.push('\n');
    }
    res
}
