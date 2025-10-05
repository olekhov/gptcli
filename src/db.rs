use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

pub fn open_db(project_root: &Path) -> Result<Connection> {
    let db_path = project_root.join(".gptcli/index.sqlite");
    std::fs::create_dir_all(db_path.parent().unwrap())?;
    let conn = Connection::open(&db_path)?;
    // базовые PRAGMA
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;"
    )?;
    ensure_schema(&conn)?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version;", [], |r| r.get(0))?;
    if v == 0 {
        create_v1(conn)?;
        conn.execute("PRAGMA user_version = 1;", [])?;
    }
    Ok(())
}

fn create_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(r#"
    -- файлы, обнаруженные scan'ом
    CREATE TABLE IF NOT EXISTS files(
      id            INTEGER PRIMARY KEY,
      namespace     TEXT NOT NULL,
      path          TEXT NOT NULL,
      size          INTEGER,
      mtime         INTEGER,
      sha           TEXT,
      lang_guess    TEXT,
      doc_kind      TEXT,
      seen_at       INTEGER,
      indexed_sha   TEXT,
      indexed_at    INTEGER,
      UNIQUE(namespace, path)
    );
    CREATE INDEX IF NOT EXISTS idx_files_ns_path ON files(namespace, path);

    -- теги из ctags
    CREATE TABLE IF NOT EXISTS tags(
      id          INTEGER PRIMARY KEY,
      file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
      name        TEXT NOT NULL,
      kind        TEXT NOT NULL,
      line        INTEGER,
      scope       TEXT,
      scope_kind  TEXT,
      signature   TEXT,
      lang        TEXT,
      end_line    INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_tags_file_line ON tags(file_id, line);
    CREATE INDEX IF NOT EXISTS idx_tags_name      ON tags(name);

    -- чанки для RAG/FTS
    CREATE TABLE IF NOT EXISTS chunks(
      id          INTEGER PRIMARY KEY,
      file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
      kind        TEXT NOT NULL,              -- function|class|namespace|block|manifest|...
      symbol      TEXT,                       -- FQN если есть
      begin_line  INTEGER,
      end_line    INTEGER,
      sha         TEXT,
      mtime       INTEGER,
      text        TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_chunks_file_begin ON chunks(file_id, begin_line);

    -- полнотекстовый индекс по тексту чанков
    CREATE VIRTUAL TABLE IF NOT EXISTS fts_chunks
      USING fts5(text, content='chunks', content_rowid='id');

    -- синхронизация fts_chunks с chunks
    CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
      INSERT INTO fts_chunks(rowid, text) VALUES (new.id, new.text);
    END;
    CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
      INSERT INTO fts_chunks(fts_chunks, rowid, text) VALUES('delete', old.id, old.text);
    END;
    CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE OF text ON chunks BEGIN
      INSERT INTO fts_chunks(fts_chunks, rowid, text) VALUES('delete', old.id, old.text);
      INSERT INTO fts_chunks(rowid, text) VALUES (new.id, new.text);
    END;
    "#)?;
    Ok(())
}
