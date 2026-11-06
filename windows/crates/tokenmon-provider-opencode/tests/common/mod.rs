//! Shared fixtures: a real-schema `opencode.db` built in a temp dir.

#![allow(dead_code, missing_docs)]

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use tempfile::TempDir;

/// Spec 03 section 7.7 schema (columns the reader relies on, plus extras
/// OpenCode has).
const SCHEMA: &str = "
CREATE TABLE session (
  id TEXT PRIMARY KEY, time_created INTEGER NOT NULL, time_archived INTEGER,
  cost REAL, tokens_input INTEGER, model TEXT);
CREATE TABLE message (
  id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL,
  time_updated INTEGER NOT NULL, data TEXT NOT NULL);
";

pub struct Fixture {
    pub dir: TempDir,
    pub path: PathBuf,
    pub conn: Connection,
    next: u32,
}

/// A message to insert.
#[derive(Clone, Copy)]
pub struct Msg<'a> {
    pub session: &'a str,
    pub secs: i64,
    pub provider: &'a str,
    pub model: &'a str,
    pub cost: f64,
    pub tokens: [i64; 4],
}

impl<'a> Msg<'a> {
    pub fn go(session: &'a str, secs: i64, model: &'a str, cost: f64) -> Self {
        Self {
            session,
            secs,
            provider: "opencode-go",
            model,
            cost,
            tokens: [0; 4],
        }
    }

    pub fn tokens(self, tokens: [i64; 4]) -> Self {
        Self { tokens, ..self }
    }

    pub fn provider(self, provider: &'a str) -> Self {
        Self { provider, ..self }
    }
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).expect("create fixture db");
        conn.execute_batch(SCHEMA).expect("schema");
        Self {
            dir,
            path,
            conn,
            next: 0,
        }
    }

    pub fn session(&mut self, id: &str, secs: i64, provider: &str, model: &str, archived: bool) {
        let model_json = format!(r#"{{"id":"{model}","providerID":"{provider}"}}"#);
        self.conn
            .execute(
                "INSERT INTO session (id, time_created, time_archived, model) VALUES (?1, ?2, ?3, ?4)",
                params![id, secs * 1000, archived.then_some(secs * 1000 + 1), model_json],
            )
            .expect("insert session");
    }

    pub fn message_json(&mut self, session: &str, secs: i64, data: &str) {
        self.next += 1;
        self.conn
            .execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?3, ?4)",
                params![format!("m{}", self.next), session, secs * 1000, data],
            )
            .expect("insert message");
    }

    pub fn message(&mut self, m: Msg<'_>) {
        let data = format!(
            r#"{{"role":"assistant","cost":{},"providerID":"{}","modelID":"{}","tokens":{{"input":{},"output":{},"cache":{{"read":{},"write":{}}}}}}}"#,
            m.cost, m.provider, m.model, m.tokens[0], m.tokens[1], m.tokens[2], m.tokens[3]
        );
        self.message_json(m.session, m.secs, &data);
    }

    /// Like the Swift `insert`: a session row plus a mirrored assistant
    /// message unless archived.
    pub fn insert(&mut self, id: &str, m: Msg<'_>, archived: bool) {
        self.session(id, m.secs, m.provider, m.model, archived);
        if !archived {
            self.message(Msg { session: id, ..m });
        }
    }
}

pub fn path_of(f: &Fixture) -> &Path {
    &f.path
}
