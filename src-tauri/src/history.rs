//! Chat history on the user's own device: one SQLite file (`leo.db`) in the
//! app data folder. No server, nothing leaves the machine.
//!
//! Each chat keeps its messages, its rolling context summary and its steps
//! summary (the tool calls made so far). Both summaries are written here the
//! moment they are created, and read back when the chat is opened or the next
//! request in it starts, so a chat resumes with the same notes it had.
//! A database problem is logged and never stops the assistant.

use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::profile::Profile;
use crate::types::ChatMessage;

const TITLE_CHARS: usize = 60;
const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS conversations (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    summary       TEXT    NOT NULL DEFAULT '',
    folded        INTEGER NOT NULL DEFAULT 0,
    steps_summary TEXT    NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT    NOT NULL,
    content         TEXT    NOT NULL,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_by_chat ON messages(conversation_id, id);
-- The user's profile (profile.rs): at most one row. Everyone has the table
-- after an update, and it starts empty.
CREATE TABLE IF NOT EXISTS profile (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    name       TEXT    NOT NULL DEFAULT '',
    dob        TEXT    NOT NULL DEFAULT '',
    location   TEXT    NOT NULL DEFAULT '',
    occupation TEXT    NOT NULL DEFAULT '',
    interests  TEXT    NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL DEFAULT 0
);
";

#[derive(Default)]
pub struct Db {
    conn: Mutex<Option<Connection>>,
}

#[derive(Serialize, Clone)]
pub struct ChatInfo {
    pub id: i64,
    pub title: String,
    pub updated_at: i64,
}

#[derive(Serialize, Clone)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
}

/// Everything saved for one chat.
pub struct Loaded {
    pub messages: Vec<StoredMessage>,
    pub summary: String,
    /// How many of the oldest messages the summary already covers.
    pub folded: usize,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("leo.db"))
}

fn lines(text: &str) -> Vec<String> {
    text.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
}

impl Db {
    /// Runs `f` on the database, opening it on first use.
    fn run<T>(&self, app: &AppHandle, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
        let mut guard = self.conn.lock().unwrap();
        if guard.is_none() {
            let conn = Connection::open(path(app)?).map_err(|e| e.to_string())?;
            conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
            *guard = Some(conn);
        }
        f(guard.as_ref().unwrap()).map_err(|e| e.to_string())
    }

    /// The saved profile; empty when none was ever saved.
    pub fn get_profile(&self, app: &AppHandle) -> Result<Profile, String> {
        self.run(app, |c| {
            let row = c
                .query_row("SELECT name, dob, location, occupation, interests FROM profile WHERE id = 1", [], |r| {
                    Ok(Profile {
                        name: r.get(0)?,
                        dob: r.get(1)?,
                        location: r.get(2)?,
                        occupation: r.get(3)?,
                        interests: r.get(4)?,
                    })
                })
                .optional()?;
            Ok(row.unwrap_or_default())
        })
    }

    pub fn save_profile(&self, app: &AppHandle, p: &Profile) -> Result<(), String> {
        self.run(app, |c| {
            c.execute(
                "INSERT INTO profile (id, name, dob, location, occupation, interests, updated_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET name = ?1, dob = ?2, location = ?3,
                     occupation = ?4, interests = ?5, updated_at = ?6",
                params![p.name, p.dob, p.location, p.occupation, p.interests, now()],
            )?;
            Ok(())
        })
    }

    pub fn create_chat(&self, app: &AppHandle, first_message: &str) -> Result<i64, String> {
        let title: String = first_message.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(TITLE_CHARS).collect();
        let title = if title.is_empty() { "New chat".to_string() } else { title };
        self.run(app, |c| {
            c.execute(
                "INSERT INTO conversations (title, created_at, updated_at) VALUES (?1, ?2, ?2)",
                params![title, now()],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn add_messages(&self, app: &AppHandle, chat: i64, messages: &[(&str, &str)]) -> Result<(), String> {
        self.run(app, |c| {
            for (role, content) in messages {
                c.execute(
                    "INSERT INTO messages (conversation_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![chat, role, content, now()],
                )?;
            }
            c.execute("UPDATE conversations SET updated_at = ?1 WHERE id = ?2", params![now(), chat])?;
            Ok(())
        })
    }

    /// Stores a new context summary and how many messages it now covers.
    pub fn set_summary(&self, app: &AppHandle, chat: i64, summary: &str, folded: usize) -> Result<(), String> {
        self.run(app, |c| {
            c.execute(
                "UPDATE conversations SET summary = ?1, folded = ?2 WHERE id = ?3",
                params![summary, folded as i64, chat],
            )?;
            Ok(())
        })
    }

    /// Stores the steps summary (one line per tool call made).
    pub fn set_steps(&self, app: &AppHandle, chat: i64, steps: &[String]) -> Result<(), String> {
        self.run(app, |c| {
            c.execute(
                "UPDATE conversations SET steps_summary = ?1 WHERE id = ?2",
                params![steps.join("\n"), chat],
            )?;
            Ok(())
        })
    }

    pub fn steps(&self, app: &AppHandle, chat: i64) -> Vec<String> {
        self.run(app, |c| {
            c.query_row("SELECT steps_summary FROM conversations WHERE id = ?1", params![chat], |r| r.get::<_, String>(0))
                .optional()
        })
        .ok()
        .flatten()
        .map(|s| lines(&s))
        .unwrap_or_default()
    }

    pub fn list(&self, app: &AppHandle) -> Result<Vec<ChatInfo>, String> {
        self.run(app, |c| {
            let mut stmt = c.prepare("SELECT id, title, updated_at FROM conversations WHERE EXISTS (SELECT 1 FROM messages WHERE conversation_id = conversations.id) ORDER BY updated_at DESC, id DESC LIMIT 200")?;
            let rows = stmt.query_map([], |r| Ok(ChatInfo { id: r.get(0)?, title: r.get(1)?, updated_at: r.get(2)? }))?;
            rows.collect()
        })
    }

    pub fn load(&self, app: &AppHandle, chat: i64) -> Result<Loaded, String> {
        self.run(app, |c| {
            let (summary, folded): (String, i64) = c.query_row(
                "SELECT summary, folded FROM conversations WHERE id = ?1",
                params![chat],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut stmt = c.prepare("SELECT role, content FROM messages WHERE conversation_id = ?1 ORDER BY id")?;
            let messages = stmt
                .query_map(params![chat], |r| Ok(StoredMessage { role: r.get(0)?, content: r.get(1)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Loaded { messages, summary, folded: folded.max(0) as usize })
        })
    }

    pub fn set_title(&self, app: &AppHandle, chat: i64, title: &str) -> Result<(), String> {
        self.run(app, |c| {
            c.execute("UPDATE conversations SET title = ?1 WHERE id = ?2", params![title, chat])?;
            Ok(())
        })
    }

    pub fn delete(&self, app: &AppHandle, chat: i64) -> Result<(), String> {
        self.run(app, |c| {
            c.execute("DELETE FROM conversations WHERE id = ?1", params![chat])?;
            Ok(())
        })
    }
}

/// The messages the model still sees word for word: everything after the part
/// the summary covers, starting at a user message.
pub fn live_history(stored: &[StoredMessage], folded: usize) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = stored
        .iter()
        .skip(folded)
        .map(|m| ChatMessage::text(&m.role, m.content.clone()))
        .collect();
    while out.first().is_some_and(|m| m.role != "user") {
        out.remove(0);
    }
    out
}

const NAMER: &str = "You name chats. You are given the first message of a chat. Reply with a short title of 2 to 5 words that says what the chat is about, in the same language as the message. No quotes, no trailing punctuation, no emoji. Reply with the title only.";

/// A model reply tidied into a title: first line, no quotes or end punctuation,
/// at most `TITLE_CHARS` characters.
fn clean_title(raw: &str) -> String {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_matches(|c: char| matches!(c, '"' | '\'' | '\u{201c}' | '\u{201d}' | '\u{2018}' | '\u{2019}' | '`' | '*' | '#'));
    let line = line.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!'));
    line.trim().chars().take(TITLE_CHARS).collect()
}

/// Right after the first message of a chat, asks the model for a short name
/// and stores it; the sidebar is told so it can show the new name. Until then
/// the chat carries the start of the message as its name. A failure just
/// leaves that placeholder.
pub async fn name_chat(app: AppHandle, api_key: String, model: String, chat: i64, first_message: String) {
    let clip: String = first_message.chars().take(1500).collect();
    let mut body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": NAMER },
            { "role": "user", "content": clip },
        ],
        "temperature": 0.3,
        "max_tokens": 40,
    });
    crate::openrouter::cache_system_prompt(&mut body);
    let Ok(reply) = crate::openrouter::quick_chat(&api_key, body).await else {
        return;
    };
    let title = clean_title(&reply.content.unwrap_or_default());
    if title.is_empty() {
        return;
    }
    if app.state::<Db>().set_title(&app, chat, &title).is_ok() {
        let _ = app.emit("history://titled", serde_json::json!({ "id": chat, "title": title }));
    }
}

#[tauri::command]
pub fn history_list(app: AppHandle, db: tauri::State<'_, Db>) -> Result<Vec<ChatInfo>, String> {
    db.list(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c
    }

    #[test]
    fn schema_round_trips_and_cascades() {
        let c = memory_db();
        c.execute(
            "INSERT INTO conversations (title, created_at, updated_at, summary, folded, steps_summary) VALUES ('t', 1, 1, 'sum', 2, 'a\nb')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO messages (conversation_id, role, content, created_at) VALUES (1, 'user', 'hi', 1)", []).unwrap();
        let steps: String = c.query_row("SELECT steps_summary FROM conversations WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(lines(&steps).len(), 2);
        c.execute("DELETE FROM conversations WHERE id = 1", []).unwrap();
        let left: i64 = c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn profile_table_starts_empty_and_saves_one_row() {
        let c = memory_db();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM profile", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        for name in ["A", "B"] {
            c.execute(
                "INSERT INTO profile (id, name) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET name = ?1",
                params![name],
            )
            .unwrap();
        }
        let n: i64 = c.query_row("SELECT COUNT(*) FROM profile", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // A second row with another id is not allowed.
        assert!(c.execute("INSERT INTO profile (id, name) VALUES (2, 'x')", []).is_err());
    }

    #[test]
    fn titles_are_tidied() {
        assert_eq!(clean_title("\"Trip to Lisbon in May.\"\n(extra)"), "Trip to Lisbon in May");
        assert_eq!(clean_title("**Fix my resume**"), "Fix my resume");
        assert_eq!(clean_title("   \n  "), "");
        assert!(clean_title(&"word ".repeat(40)).chars().count() <= TITLE_CHARS);
    }

    #[test]
    fn live_history_skips_what_the_summary_covers() {
        let m = |r: &str, t: &str| StoredMessage { role: r.into(), content: t.into() };
        let stored = vec![m("user", "1"), m("assistant", "2"), m("user", "3"), m("assistant", "4")];
        let live = live_history(&stored, 2);
        assert_eq!(live.len(), 2);
        assert_eq!(live[0].content.as_deref(), Some("3"));
        // Starts at a user message even if the fold point is odd.
        assert_eq!(live_history(&stored, 1)[0].role, "user");
    }
}
