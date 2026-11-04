use crate::names::random_name;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use std::error::Error;
use std::path::Path;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MAX_NAME_ATTEMPTS: usize = 20;

pub struct Message {
    pub id: i64,
    pub user: String,
    pub at: i64,
    pub data: String,
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS topics (
            name TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            closed_at INTEGER
        );
        -- AUTOINCREMENT keeps ids rising after deletes, so a saved cursor never skips new messages.
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            topic TEXT NOT NULL REFERENCES topics(name),
            user TEXT NOT NULL,
            at INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS cursors (
            topic TEXT NOT NULL REFERENCES topics(name),
            listener TEXT NOT NULL,
            last_id INTEGER NOT NULL,
            PRIMARY KEY (topic, listener)
        );",
    )?;
    Ok(conn)
}

pub fn create_topic(conn: &Connection) -> Result<String> {
    for _ in 0..MAX_NAME_ATTEMPTS {
        let name = random_name();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO topics (name, created_at) VALUES (?1, ?2)",
            params![name, Utc::now().timestamp()],
        )?;
        if inserted == 1 {
            return Ok(name);
        }
    }
    Err("could not find an unused topic name".into())
}

/// `None` means the topic does not exist; `Some(true)` means it is closed.
pub fn topic_closed(conn: &Connection, topic: &str) -> Result<Option<bool>> {
    let closed = conn
        .query_row(
            "SELECT closed_at IS NOT NULL FROM topics WHERE name = ?1",
            [topic],
            |row| row.get(0),
        )
        .optional()?;
    Ok(closed)
}

pub fn send(conn: &Connection, topic: &str, user: &str, body: &str) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    match topic_closed(&tx, topic)? {
        None => return Err(format!("unknown topic {topic}").into()),
        Some(true) => return Err(format!("topic {topic} is closed").into()),
        Some(false) => {}
    }
    tx.execute(
        "INSERT INTO messages (topic, user, at, data) VALUES (?1, ?2, ?3, ?4)",
        params![topic, user, Utc::now().timestamp(), body],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

pub fn close(conn: &Connection, topic: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    if topic_closed(&tx, topic)?.ok_or_else(|| format!("unknown topic {topic}"))? {
        return Ok(());
    }
    tx.execute("DELETE FROM messages WHERE topic = ?1", [topic])?;
    tx.execute("DELETE FROM cursors WHERE topic = ?1", [topic])?;
    tx.execute(
        "UPDATE topics SET closed_at = ?2 WHERE name = ?1",
        params![topic, Utc::now().timestamp()],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn max_id(conn: &Connection, topic: &str) -> Result<i64> {
    let id = conn.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM messages WHERE topic = ?1",
        [topic],
        |row| row.get(0),
    )?;
    Ok(id)
}

pub fn messages_after(conn: &Connection, topic: &str, last: i64) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT id, user, at, data FROM messages WHERE topic = ?1 AND id > ?2 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![topic, last], |row| {
        Ok(Message {
            id: row.get(0)?,
            user: row.get(1)?,
            at: row.get(2)?,
            data: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn cursor(conn: &Connection, topic: &str, listener: &str) -> Result<Option<i64>> {
    let last = conn
        .query_row(
            "SELECT last_id FROM cursors WHERE topic = ?1 AND listener = ?2",
            params![topic, listener],
            |row| row.get(0),
        )
        .optional()?;
    Ok(last)
}

pub fn save_cursor(conn: &Connection, topic: &str, listener: &str, last_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO cursors (topic, listener, last_id) VALUES (?1, ?2, ?3)
         ON CONFLICT (topic, listener) DO UPDATE SET last_id = excluded.last_id",
        params![topic, listener, last_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("t.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn closed_topic_name_cannot_be_reinserted() {
        let (_dir, conn) = test_conn();
        let name = create_topic(&conn).unwrap();
        close(&conn, &name).unwrap();
        let reinsert = conn.execute(
            "INSERT INTO topics (name, created_at) VALUES (?1, 0)",
            [&name],
        );
        assert!(reinsert.is_err());
    }

    #[test]
    fn message_ids_do_not_go_backwards_after_close() {
        let (_dir, conn) = test_conn();
        let a = create_topic(&conn).unwrap();
        let first = send(&conn, &a, "u", "x").unwrap();
        close(&conn, &a).unwrap();
        let b = create_topic(&conn).unwrap();
        assert!(send(&conn, &b, "u", "y").unwrap() > first);
    }
}
