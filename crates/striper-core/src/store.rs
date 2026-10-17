//! SQLite persistence for learned vocabulary, its audit trail and the edit
//! transaction log (PLAN.md §6). Independent of the recognizer's models and
//! of the app's settings store; versioned with `rusqlite_migration`.

use crate::session::{LearnRequest, VocabularySink};
use crate::text::fold;
use crate::vocabulary::{MatchPolicy, Scope, VocabularyEntry};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

static MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE vocabulary_entry (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        preferred_text TEXT NOT NULL,
        aliases TEXT NOT NULL DEFAULT '[]',
        spoken_hint TEXT,
        policy TEXT NOT NULL DEFAULT 'contextual',
        context_words TEXT NOT NULL DEFAULT '[]',
        negative_context_words TEXT NOT NULL DEFAULT '[]',
        scope TEXT NOT NULL DEFAULT '{\"kind\":\"global\"}',
        user_confirmed INTEGER NOT NULL DEFAULT 1,
        enabled INTEGER NOT NULL DEFAULT 1,
        use_as_asr_hint INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        deleted_at INTEGER
    );
    CREATE INDEX idx_vocabulary_preferred ON vocabulary_entry(preferred_text);
    CREATE TABLE vocabulary_audit (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        entry_id INTEGER NOT NULL,
        action TEXT NOT NULL,
        snapshot TEXT NOT NULL,
        timestamp INTEGER NOT NULL
    );
    CREATE TABLE edit_transaction (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp INTEGER NOT NULL,
        app_id TEXT,
        operation TEXT NOT NULL,
        instruction TEXT NOT NULL,
        before_text TEXT NOT NULL,
        after_text TEXT NOT NULL,
        result TEXT NOT NULL,
        interpreter TEXT NOT NULL
    );",
)];

/// Edit transactions kept; older rows are pruned on insert.
pub const MAX_TRANSACTIONS: i64 = 500;

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Migration(String),
    Json(serde_json::Error),
    NotFound(i64),
    Invalid(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "database error: {e}"),
            StoreError::Migration(e) => write!(f, "database migration failed: {e}"),
            StoreError::Json(e) => write!(f, "invalid stored data: {e}"),
            StoreError::NotFound(id) => write!(f, "vocabulary entry {id} not found"),
            StoreError::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Fields a user may change from the vocabulary manager.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntryUpdate {
    pub preferred_text: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub policy: Option<MatchPolicy>,
    pub context_words: Option<Vec<String>>,
    pub negative_context_words: Option<Vec<String>>,
    pub scope: Option<Scope>,
    pub enabled: Option<bool>,
    pub use_as_asr_hint: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: i64,
    pub entry_id: i64,
    pub action: String,
    pub snapshot: VocabularyEntry,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditTransactionRecord {
    pub id: i64,
    pub timestamp: i64,
    pub app_id: Option<String>,
    pub operation: String,
    pub instruction: String,
    pub before_text: String,
    pub after_text: String,
    pub result: String,
    /// "local" (deterministic parser) or "llm".
    pub interpreter: String,
}

/// Portable export format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocabularyExport {
    pub format: String,
    pub version: u32,
    pub entries: Vec<VocabularyEntry>,
}

pub const EXPORT_FORMAT: &str = "striper-vocabulary";

pub struct Store {
    conn: Connection,
}

fn clean_list(items: &[String], folded: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let t = item.trim();
        let t = if folded { fold(t) } else { t.to_string() };
        if !t.is_empty() && !out.iter().any(|o| fold(o) == fold(&t)) {
            out.push(t);
        }
    }
    out
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Invalid(e.to_string()))?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Migrations::new(MIGRATIONS.to_vec())
            .to_latest(&mut conn)
            .map_err(|e| StoreError::Migration(e.to_string()))?;
        Ok(Self { conn })
    }

    fn map_entry(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<(VocabularyEntry, String, String, String, String, String)> {
        Ok((
            VocabularyEntry {
                id: row.get("id")?,
                preferred_text: row.get("preferred_text")?,
                aliases: vec![],
                spoken_hint: row.get("spoken_hint")?,
                policy: MatchPolicy::Contextual,
                context_words: vec![],
                negative_context_words: vec![],
                scope: Scope::Global,
                user_confirmed: row.get("user_confirmed")?,
                enabled: row.get("enabled")?,
                use_as_asr_hint: row.get("use_as_asr_hint")?,
                created_at: row.get("created_at")?,
                updated_at: row.get("updated_at")?,
            },
            row.get("aliases")?,
            row.get("policy")?,
            row.get("context_words")?,
            row.get("negative_context_words")?,
            row.get("scope")?,
        ))
    }

    fn finish_entry(
        raw: (VocabularyEntry, String, String, String, String, String),
    ) -> Result<VocabularyEntry> {
        let (mut e, aliases, policy, context, negative, scope) = raw;
        e.aliases = serde_json::from_str(&aliases)?;
        e.policy = serde_json::from_value(serde_json::Value::String(policy))?;
        e.context_words = serde_json::from_str(&context)?;
        e.negative_context_words = serde_json::from_str(&negative)?;
        e.scope = serde_json::from_str(&scope)?;
        Ok(e)
    }

    /// All entries that have not been deleted, newest first.
    pub fn list_entries(&self) -> Result<Vec<VocabularyEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM vocabulary_entry WHERE deleted_at IS NULL ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], Self::map_entry)?;
        rows.map(|r| Self::finish_entry(r?)).collect()
    }

    /// Entries the dictation path should apply.
    pub fn active_entries(&self) -> Result<Vec<VocabularyEntry>> {
        Ok(self
            .list_entries()?
            .into_iter()
            .filter(|e| e.is_active())
            .collect())
    }

    /// Preferred spellings to pass to the recognizer as vocabulary hints.
    pub fn asr_hints(&self) -> Result<Vec<String>> {
        Ok(self
            .active_entries()?
            .into_iter()
            .filter(|e| e.use_as_asr_hint)
            .map(|e| e.preferred_text)
            .collect())
    }

    pub fn get_entry(&self, id: i64) -> Result<VocabularyEntry> {
        let raw = self
            .conn
            .query_row(
                "SELECT * FROM vocabulary_entry WHERE id = ?1 AND deleted_at IS NULL",
                params![id],
                Self::map_entry,
            )
            .optional()?
            .ok_or(StoreError::NotFound(id))?;
        Self::finish_entry(raw)
    }

    fn find_by_preferred(&self, preferred: &str) -> Result<Option<VocabularyEntry>> {
        Ok(self
            .list_entries()?
            .into_iter()
            .find(|e| fold(&e.preferred_text) == fold(preferred) && e.preferred_text == preferred))
    }

    fn audit(&self, entry_id: i64, action: &str) -> Result<()> {
        let snapshot = serde_json::to_string(&self.get_entry_any(entry_id)?)?;
        self.conn.execute(
            "INSERT INTO vocabulary_audit (entry_id, action, snapshot, timestamp) VALUES (?1, ?2, ?3, ?4)",
            params![entry_id, action, snapshot, now_secs()],
        )?;
        Ok(())
    }

    fn get_entry_any(&self, id: i64) -> Result<VocabularyEntry> {
        let raw = self
            .conn
            .query_row(
                "SELECT * FROM vocabulary_entry WHERE id = ?1",
                params![id],
                Self::map_entry,
            )
            .optional()?
            .ok_or(StoreError::NotFound(id))?;
        Self::finish_entry(raw)
    }

    /// Save a confirmed spelling. Teaching the same preferred spelling again
    /// merges the new aliases and context into the existing entry.
    pub fn learn(&self, req: &LearnRequest) -> Result<i64> {
        let preferred = req.preferred_text.trim();
        if preferred.is_empty() {
            return Err(StoreError::Invalid("preferred spelling is empty".into()));
        }
        let now = now_secs();
        let aliases = clean_list(&req.aliases, false);
        let context = clean_list(&req.context_words, true);
        if let Some(mut existing) = self.find_by_preferred(preferred)? {
            existing.aliases = clean_list(&[existing.aliases, aliases].concat(), false);
            existing.context_words = clean_list(&[existing.context_words, context].concat(), true);
            if req.always {
                existing.policy = MatchPolicy::Always;
            }
            self.conn.execute(
                "UPDATE vocabulary_entry SET aliases = ?2, context_words = ?3, policy = ?4, enabled = 1, user_confirmed = 1, updated_at = ?5 WHERE id = ?1",
                params![
                    existing.id,
                    serde_json::to_string(&existing.aliases)?,
                    serde_json::to_string(&existing.context_words)?,
                    policy_str(existing.policy),
                    now
                ],
            )?;
            self.audit(existing.id, "relearned")?;
            return Ok(existing.id);
        }
        let policy = if req.always {
            MatchPolicy::Always
        } else {
            MatchPolicy::Contextual
        };
        self.conn.execute(
            "INSERT INTO vocabulary_entry (preferred_text, aliases, policy, context_words, scope, user_confirmed, enabled, use_as_asr_hint, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, 1, 1, ?6, ?6)",
            params![
                preferred,
                serde_json::to_string(&aliases)?,
                policy_str(policy),
                serde_json::to_string(&context)?,
                serde_json::to_string(&Scope::Global)?,
                now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.audit(id, "created")?;
        Ok(id)
    }

    pub fn update_entry(&self, id: i64, update: &EntryUpdate) -> Result<VocabularyEntry> {
        let mut e = self.get_entry(id)?;
        if let Some(p) = &update.preferred_text {
            let p = p.trim();
            if p.is_empty() {
                return Err(StoreError::Invalid("preferred spelling is empty".into()));
            }
            e.preferred_text = p.to_string();
        }
        if let Some(a) = &update.aliases {
            e.aliases = clean_list(a, false);
        }
        if let Some(p) = update.policy {
            e.policy = p;
        }
        if let Some(c) = &update.context_words {
            e.context_words = clean_list(c, true);
        }
        if let Some(n) = &update.negative_context_words {
            e.negative_context_words = clean_list(n, true);
        }
        if let Some(s) = &update.scope {
            e.scope = s.clone();
        }
        if let Some(v) = update.enabled {
            e.enabled = v;
        }
        if let Some(v) = update.use_as_asr_hint {
            e.use_as_asr_hint = v;
        }
        self.conn.execute(
            "UPDATE vocabulary_entry SET preferred_text = ?2, aliases = ?3, policy = ?4, context_words = ?5,
             negative_context_words = ?6, scope = ?7, enabled = ?8, use_as_asr_hint = ?9, updated_at = ?10 WHERE id = ?1",
            params![
                id,
                e.preferred_text,
                serde_json::to_string(&e.aliases)?,
                policy_str(e.policy),
                serde_json::to_string(&e.context_words)?,
                serde_json::to_string(&e.negative_context_words)?,
                serde_json::to_string(&e.scope)?,
                e.enabled,
                e.use_as_asr_hint,
                now_secs()
            ],
        )?;
        self.audit(id, "updated")?;
        self.get_entry(id)
    }

    /// Soft-delete (restorable from the audit trail).
    pub fn delete_entry(&self, id: i64) -> Result<()> {
        self.get_entry(id)?;
        self.conn.execute(
            "UPDATE vocabulary_entry SET deleted_at = ?2, enabled = 0, updated_at = ?2 WHERE id = ?1",
            params![id, now_secs()],
        )?;
        self.audit(id, "deleted")
    }

    pub fn restore_entry(&self, id: i64) -> Result<VocabularyEntry> {
        self.conn.execute(
            "UPDATE vocabulary_entry SET deleted_at = NULL, enabled = 1, updated_at = ?2 WHERE id = ?1",
            params![id, now_secs()],
        )?;
        self.audit(id, "restored")?;
        self.get_entry(id)
    }

    /// Disable the entry whose preferred spelling or alias matches `text`,
    /// or the most recently learned/updated one when `text` is `None`.
    pub fn forget(&self, text: Option<&str>) -> Result<Option<String>> {
        let entries = self.active_entries()?;
        let found = match text {
            Some(t) => {
                let t = fold(t.trim());
                entries.into_iter().find(|e| {
                    fold(&e.preferred_text) == t || e.aliases.iter().any(|a| fold(a) == t)
                })
            }
            None => entries.into_iter().next(),
        };
        let Some(entry) = found else {
            return Ok(None);
        };
        self.conn.execute(
            "UPDATE vocabulary_entry SET enabled = 0, updated_at = ?2 WHERE id = ?1",
            params![entry.id, now_secs()],
        )?;
        self.audit(entry.id, "forgotten")?;
        Ok(Some(entry.preferred_text))
    }

    pub fn audit_log(&self, entry_id: i64) -> Result<Vec<AuditRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, entry_id, action, snapshot, timestamp FROM vocabulary_audit WHERE entry_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![entry_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (id, entry_id, action, snapshot, timestamp) = r?;
            Ok(AuditRecord {
                id,
                entry_id,
                action,
                snapshot: serde_json::from_str(&snapshot)?,
                timestamp,
            })
        })
        .collect()
    }

    pub fn export(&self) -> Result<VocabularyExport> {
        Ok(VocabularyExport {
            format: EXPORT_FORMAT.into(),
            version: 1,
            entries: self.list_entries()?,
        })
    }

    /// Import entries (merging by preferred spelling). Returns how many
    /// entries were created or updated.
    pub fn import(&self, data: &VocabularyExport) -> Result<usize> {
        if data.format != EXPORT_FORMAT || data.version != 1 {
            return Err(StoreError::Invalid(
                "not a Striper vocabulary export".into(),
            ));
        }
        let mut n = 0;
        for e in &data.entries {
            let id = self.learn(&LearnRequest {
                preferred_text: e.preferred_text.clone(),
                aliases: e.aliases.clone(),
                context_words: e.context_words.clone(),
                always: e.policy == MatchPolicy::Always,
                app_id: None,
            })?;
            self.update_entry(
                id,
                &EntryUpdate {
                    negative_context_words: Some(e.negative_context_words.clone()),
                    scope: Some(e.scope.clone()),
                    enabled: Some(e.enabled),
                    use_as_asr_hint: Some(e.use_as_asr_hint),
                    ..Default::default()
                },
            )?;
            n += 1;
        }
        Ok(n)
    }

    pub fn record_transaction(&self, t: &EditTransactionRecord) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO edit_transaction (timestamp, app_id, operation, instruction, before_text, after_text, result, interpreter)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![t.timestamp, t.app_id, t.operation, t.instruction, t.before_text, t.after_text, t.result, t.interpreter],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM edit_transaction WHERE id <= ?1",
            params![id - MAX_TRANSACTIONS],
        )?;
        Ok(id)
    }

    pub fn recent_transactions(&self, limit: usize) -> Result<Vec<EditTransactionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, app_id, operation, instruction, before_text, after_text, result, interpreter
             FROM edit_transaction ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(EditTransactionRecord {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                app_id: r.get(2)?,
                operation: r.get(3)?,
                instruction: r.get(4)?,
                before_text: r.get(5)?,
                after_text: r.get(6)?,
                result: r.get(7)?,
                interpreter: r.get(8)?,
            })
        })?;
        rows.map(|r| r.map_err(StoreError::from)).collect()
    }
}

fn policy_str(p: MatchPolicy) -> &'static str {
    match p {
        MatchPolicy::Contextual => "contextual",
        MatchPolicy::Always => "always",
    }
}

impl VocabularySink for Store {
    fn learn(&mut self, request: &LearnRequest) -> std::result::Result<i64, String> {
        Store::learn(self, request).map_err(|e| e.to_string())
    }

    fn forget(&mut self, text: Option<&str>) -> std::result::Result<Option<String>, String> {
        Store::forget(self, text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocabulary::{apply_vocabulary, ApplyContext};

    fn striper_request() -> LearnRequest {
        LearnRequest {
            preferred_text: "Striper".into(),
            aliases: vec!["striped".into()],
            context_words: vec!["opened".into(), "Write".into()],
            always: false,
            app_id: Some("notepad.exe".into()),
        }
    }

    // PLAN.md §0 step 3: the preference survives a restart.
    #[test]
    fn learned_spelling_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("striper.db");
        {
            let store = Store::open(&path).unwrap();
            store.learn(&striper_request()).unwrap();
        }
        let store = Store::open(&path).unwrap();
        let entries = store.active_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].context_words, vec!["opened", "write"]);
        let out = apply_vocabulary(
            "Then I opened striped again",
            &entries,
            &ApplyContext::default(),
        );
        assert_eq!(out.text, "Then I opened Striper again");
        let out = apply_vocabulary("A striped shirt", &entries, &ApplyContext::default());
        assert_eq!(out.text, "A striped shirt");
        assert_eq!(store.asr_hints().unwrap(), vec!["Striper"]);
    }

    #[test]
    fn relearning_merges_aliases_and_context() {
        let store = Store::open_in_memory().unwrap();
        let id = store.learn(&striper_request()).unwrap();
        let id2 = store
            .learn(&LearnRequest {
                aliases: vec!["stripper".into(), "STRIPED".into()],
                context_words: vec!["launch".into()],
                ..striper_request()
            })
            .unwrap();
        assert_eq!(id, id2);
        let e = store.get_entry(id).unwrap();
        assert_eq!(e.aliases, vec!["striped", "stripper"]);
        assert_eq!(e.context_words, vec!["opened", "write", "launch"]);
    }

    #[test]
    fn forget_disables_and_audits() {
        let store = Store::open_in_memory().unwrap();
        let id = store.learn(&striper_request()).unwrap();
        assert_eq!(
            store.forget(Some("striped")).unwrap().as_deref(),
            Some("Striper")
        );
        assert!(store.active_entries().unwrap().is_empty());
        assert_eq!(store.list_entries().unwrap().len(), 1, "kept for review");
        let actions: Vec<String> = store
            .audit_log(id)
            .unwrap()
            .into_iter()
            .map(|a| a.action)
            .collect();
        assert_eq!(actions, vec!["created", "forgotten"]);
        assert_eq!(store.forget(Some("nothing")).unwrap(), None);
    }

    #[test]
    fn forget_without_text_targets_latest() {
        let store = Store::open_in_memory().unwrap();
        store.learn(&striper_request()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        store
            .learn(&LearnRequest {
                preferred_text: "Stephen".into(),
                aliases: vec!["Steven".into()],
                ..striper_request()
            })
            .unwrap();
        // Same-second timestamps fall back to id order (newest first).
        assert_eq!(store.forget(None).unwrap().as_deref(), Some("Stephen"));
    }

    #[test]
    fn update_delete_restore() {
        let store = Store::open_in_memory().unwrap();
        let id = store.learn(&striper_request()).unwrap();
        let e = store
            .update_entry(
                id,
                &EntryUpdate {
                    policy: Some(MatchPolicy::Always),
                    scope: Some(Scope::App("kate".into())),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(e.policy, MatchPolicy::Always);
        assert_eq!(e.scope, Scope::App("kate".into()));
        store.delete_entry(id).unwrap();
        assert!(store.list_entries().unwrap().is_empty());
        assert!(matches!(store.get_entry(id), Err(StoreError::NotFound(_))));
        store.restore_entry(id).unwrap();
        assert_eq!(store.list_entries().unwrap().len(), 1);
    }

    #[test]
    fn export_import_round_trip() {
        let a = Store::open_in_memory().unwrap();
        a.learn(&striper_request()).unwrap();
        let json = serde_json::to_string(&a.export().unwrap()).unwrap();
        let b = Store::open_in_memory().unwrap();
        assert_eq!(b.import(&serde_json::from_str(&json).unwrap()).unwrap(), 1);
        let e = &b.list_entries().unwrap()[0];
        assert_eq!(e.preferred_text, "Striper");
        assert_eq!(e.aliases, vec!["striped"]);
        let bad = VocabularyExport {
            format: "other".into(),
            version: 1,
            entries: vec![],
        };
        assert!(b.import(&bad).is_err());
    }

    #[test]
    fn transactions_are_recorded_and_pruned() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..(MAX_TRANSACTIONS + 5) {
            store
                .record_transaction(&EditTransactionRecord {
                    id: 0,
                    timestamp: i,
                    app_id: None,
                    operation: "replace_text".into(),
                    instruction: "change a to b".into(),
                    before_text: "a".into(),
                    after_text: "b".into(),
                    result: "applied".into(),
                    interpreter: "local".into(),
                })
                .unwrap();
        }
        let recent = store.recent_transactions(1000).unwrap();
        assert_eq!(recent.len() as i64, MAX_TRANSACTIONS);
        assert_eq!(recent[0].timestamp, MAX_TRANSACTIONS + 4);
    }
}
