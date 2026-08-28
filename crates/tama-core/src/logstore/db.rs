//! Log store database: SQLite schema, FTS5, batch insert, query, and
//! retention prune (plan-195 task 1).
//!
//! ## Concurrency
//!
//! [`LogStore`] is `Send` (single `rusqlite::Connection`). The tracing
//! writer (task 2) is the sole writer; read endpoints (task 4) open
//! separate connections. WAL journal mode keeps readers available during
//! write bursts.
//!
//! ## `VACUUM` policy
//!
//! No routine `VACUUM` anywhere in this feature. Freed pages after deletes
//! are reclaimed with `PRAGMA incremental_vacuum(n)` (which requires the
//! database to be in `auto_vacuum = INCREMENTAL` mode). The single permitted
//! `VACUUM` is inside [`LogStore::open`] and only as a one-time mode
//! migration when resuming a non-empty pre-existing database file that was
//! created before this feature (reading `auto_vacuum = 0`); it migrates the
//! mode, it is not the routine path the "no VACUUM" rule refers to.
//!
//! ## Schema
//!
//! One JSON document per row + indexed label columns (ADR-0013):
//! the `logs` table is indexed by `(level, ts)` and `source`; `logs_fts`
//! is an external-content FTS5 table (unicode61) over the whole `msg`
//! JSON document kept in sync by `AFTER INSERT` / `AFTER DELETE` triggers.
//! Because it indexes the whole document, searching structural keys
//! (`message`, `target`, `dropped`) matches most rows — expected, and
//! documented on the endpoints.
//!
//! ## LIKE escaping
//!
//! Free-text and source filters use `LIKE` with `!` as the escape
//! character (`ESCAPE '!'`); [`LogStore::escape_like`] neutralises `%`,
//! `_` and `!` in user text so it is matched literally.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};

use crate::logstore::types::{
    LevelCount, LogEntry, LogQuery, LogRecord, LogstoreLevel, PruneBounds, QueryOrder, Source,
    SourceInfo,
};

/// Batch size for chunked id-range `DELETE`s (SQLite has no `DELETE … LIMIT`
/// — the pruner loops id-range chunks, see [`LogStore::prune`]).
const DELETE_CHUNK_ROWS: i64 = 10_000;

/// Chunk size for the oldest-first byte-accounting scan in
/// [`LogStore::prune`].
const BYTE_SCAN_CHUNK_ROWS: i64 = 10_000;

/// Pages released per `PRAGMA incremental_vacuum(n)` after deletes.
const INCREMENTAL_VACUUM_PAGES: i64 = 4096;

/// SQLite `auto_vacuum` mode constant for INCREMENTAL.
const AUTO_VACUUM_INCREMENTAL: i64 = 2;

/// SQL escape character used by every `LIKE` in this module (`ESCAPE '!'`).
const LIKE_ESCAPE: &str = "'!'";

/// Idempotent schema init (safe to run on every open).
const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS logs (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    level INTEGER NOT NULL,
    source TEXT NOT NULL,
    msg TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS logs_level_ts ON logs (level, ts);
CREATE INDEX IF NOT EXISTS logs_source ON logs (source);
CREATE VIRTUAL TABLE IF NOT EXISTS logs_fts USING fts5(
    msg, content='logs', content_rowid='id', tokenize='unicode61'
);
CREATE TRIGGER IF NOT EXISTS logs_ai AFTER INSERT ON logs BEGIN
    INSERT INTO logs_fts(rowid, msg) VALUES (new.id, new.msg);
END;
CREATE TRIGGER IF NOT EXISTS logs_ad AFTER DELETE ON logs BEGIN
    INSERT INTO logs_fts(logs_fts, rowid, msg) VALUES ('delete', old.id, old.msg);
END;
";

/// Embedded SQLite log store (one connection).
///
/// `Send` — holds a single `rusqlite::Connection`. The writer (tracing
/// appender, task 2) is the sole writer of this store; readers open
/// separate connections (task 4).
pub struct LogStore {
    conn: Connection,
    /// Path of the underlying database (or `":memory:"`).
    pub path: PathBuf,
    /// Test-only fault injection: when set, the next `insert_batch` fails
    /// exactly once. Compiled out of production builds (zero cost).
    #[cfg(test)]
    fail_next: std::sync::atomic::AtomicBool,
}

impl LogStore {
    /// Open (or create) the log store at `path`, creating parent
    /// directories.
    ///
    /// Applies `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`
    /// and (before any data page exists — or via the one-time migration
    /// `VACUUM`, see module docs for the sole permitted `VACUUM`)
    /// `auto_vacuum=INCREMENTAL`. Schema init is idempotent; on resume
    /// (file already existed non-empty) a `PRAGMA quick_check` sanity
    /// check runs.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        // "Resume" = the database file existed non-empty BEFORE we opened
        // it (Connection::open would otherwise create an empty file).
        let resume = path.metadata().map(|m| m.len() > 0).unwrap_or(false);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create log store parent dir {}", parent.display()))?;
            }
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("open log store at {}", path.display()))?;
        Self::init_connection(conn, path, resume)
    }

    /// `:memory:` variant for tests and convenience.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory log store")?;
        Self::init_connection(conn, PathBuf::from(":memory:"), false)
    }

    /// Shared open logic: PRAGMAs, one-time auto_vacuum migration,
    /// idempotent schema init, resume sanity check.
    fn init_connection(conn: Connection, path: PathBuf, resume: bool) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("PRAGMA journal_mode=WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .context("PRAGMA synchronous=NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5000i64)
            .context("PRAGMA busy_timeout")?;

        // auto_vacuum=INCREMENTAL can only be set while the database file is
        // empty; a newly created file picks it up on the first page. A
        // non-empty file created before this feature (auto_vacuum = 0 =
        // NONE) is migrated here with the ONE permitted VACUUM in this
        // feature — a mode migration, not the routine path the "no VACUUM"
        // rule refers to. Load-bearing: `PRAGMA incremental_vacuum(n)`, run
        // after every delete batch, is a no-op unless this mode is
        // INCREMENTAL (otherwise the file grows monotonically).
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .context("PRAGMA auto_vacuum=INCREMENTAL")?;
        if Self::auto_vacuum_mode(&conn)? != AUTO_VACUUM_INCREMENTAL && resume {
            // The single permitted VACUUM: migrate a legacy non-empty
            // auto_vacuum=NONE file into INCREMENTAL mode.
            conn.execute_batch("VACUUM")
                .context("auto_vacuum mode-migration VACUUM")?;
            if Self::auto_vacuum_mode(&conn)? != AUTO_VACUUM_INCREMENTAL {
                bail!("could not migrate existing log store to auto_vacuum=INCREMENTAL");
            }
        }

        conn.execute_batch(SCHEMA_SQL)
            .context("init log store schema")?;

        if resume {
            let status: String = conn
                .query_row("PRAGMA quick_check", [], |r| r.get(0))
                .context("PRAGMA quick_check on resume")?;
            if status != "ok" {
                bail!("log store quick_check failed on resume: {status}");
            }
        }

        Ok(Self {
            conn,
            path,
            #[cfg(test)]
            fail_next: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Current `auto_vacuum` mode (0 = NONE, 1 = FULL, 2 = INCREMENTAL).
    fn auto_vacuum_mode(conn: &Connection) -> Result<i64> {
        conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
            .context("PRAGMA auto_vacuum (read back)")
    }

    /// Insert a batch of records in ONE transaction; returns the generated
    /// row ids in input order.
    ///
    /// One prepared statement per call (nothing mutable is stored on the
    /// store), one `INSERT` per record — the 200-row writer batches keep
    /// this fast.
    pub fn insert_batch(&self, records: &[LogRecord]) -> Result<Vec<i64>> {
        #[cfg(test)]
        if self
            .fail_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            bail!("injected");
        }

        let tx = self
            .conn
            .unchecked_transaction()
            .context("begin insert transaction")?;
        let mut ids = Vec::with_capacity(records.len());
        {
            let mut stmt = tx
                .prepare("INSERT INTO logs (ts, level, source, msg) VALUES (?1, ?2, ?3, ?4)")
                .context("prepare insert")?;
            for record in records {
                stmt.execute((
                    record.ts,
                    record.level.as_u8() as i32,
                    record.source.as_str(),
                    record.msg.to_string(),
                ))
                .with_context(|| format!("insert log row (source={})", record.source))?;
                ids.push(tx.last_insert_rowid());
            }
        }
        tx.commit().context("commit insert transaction")?;
        Ok(ids)
    }

    /// Run a read query; returns `(entries, next_cursor)` where
    /// `next_cursor` is `None` once the window end is reached.
    ///
    /// `q` uses FTS5 `MATCH` first; on ANY rusqlite error from the MATCH
    /// attempt (e.g. malformed FTS syntax) — or when it yields zero rows —
    /// the query transparently falls back to a `LIKE '%q%'` scan so search
    /// text never surfaces a 500.
    pub fn query(&self, qu: &LogQuery) -> Result<(Vec<LogEntry>, Option<i64>)> {
        let limit = qu.effective_limit();
        let (mut params, mut clauses) = Self::base_clauses(qu);

        if let Some(cursor) = qu.cursor {
            let op = match qu.order {
                QueryOrder::Desc => "<",
                QueryOrder::Asc => ">",
            };
            params.push(Value::Integer(cursor));
            clauses.push(format!("id {op} ?{}", params.len()));
        }

        let dir = match qu.order {
            QueryOrder::Desc => "DESC",
            QueryOrder::Asc => "ASC",
        };
        let base_where = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };

        // Builds the final SELECT. When `clause` (+ `value`) is given, an
        // extra `WHERE`/`AND` clause is appended with one bound parameter
        // inserted before the LIMIT argument; `clause(n)` receives the
        // positional slot n and returns the clause SQL.
        let make_sql = |clause: Option<fn(usize) -> String>,
                        value: Option<Value>|
         -> (String, Vec<Value>) {
            let mut p = params.clone();
            let where_sql = match (clause, value) {
                (Some(c), Some(v)) => {
                    p.push(v);
                    let clause_sql = c(p.len());
                    if base_where.is_empty() {
                        format!(" WHERE {clause_sql}")
                    } else {
                        format!("{base_where} AND {clause_sql}")
                    }
                }
                _ => base_where.clone(),
            };
            p.push(Value::Integer(limit + 1));
            let sql = format!(
                "SELECT id, ts, level, source, msg FROM logs{where_sql} ORDER BY id {dir} LIMIT ?{}",
                p.len()
            );
            (sql, p)
        };

        if let Some(term) = qu.q.as_deref().filter(|t| !t.trim().is_empty()) {
            let (sql, p) = make_sql(
                Some(|n| format!("logs_fts MATCH ?{n}")),
                Some(Value::Text(term.to_string())),
            );
            return match self.run_query(&sql, &p, limit) {
                Ok((entries, next)) if !entries.is_empty() => Ok((entries, next)),
                Ok(_) | Err(_) => {
                    // Zero FTS rows, or a malformed-FTS-syntax error:
                    // fall through to the LIKE fallback with the same
                    // window and ordering. An FTS error must never
                    // propagate to the handler.
                    let like = format!("%{}%", Self::escape_like(term));
                    let (sql, p) = make_sql(
                        Some(|n| format!("msg LIKE ?{n} ESCAPE '!'")),
                        Some(Value::Text(like)),
                    );
                    self.run_query(&sql, &p, limit)
                }
            };
        }

        let (sql, p) = make_sql(None, None);
        self.run_query(&sql, &p, limit)
    }

    /// Row count under the same read-side filters as [`LogStore::query`]
    /// (`min_level` / `source` / `since` / `until` / `q`). `limit` and
    /// `cursor` are deliberately ignored — the count spans the whole
    /// window (task 4's export cap check). `q` mirrors `query`'s FTS
    /// semantics: `MATCH` first, transparent `LIKE` fallback (zero rows
    /// or malformed FTS syntax) so search text never surfaces an error.
    pub fn count(&self, qu: &LogQuery) -> Result<i64> {
        let (params, clauses) = Self::base_clauses(qu);
        let base_where = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };

        // Same closure shape as `query`: `clause(n)` builds the extra
        // WHERE slot for the q-term, `value` is its bound parameter.
        let make_sql =
            |clause: Option<fn(usize) -> String>, value: Option<Value>| -> (String, Vec<Value>) {
                let mut p = params.clone();
                let where_sql = match (clause, value) {
                    (Some(c), Some(v)) => {
                        p.push(v);
                        let clause_sql = c(p.len());
                        if base_where.is_empty() {
                            format!(" WHERE {clause_sql}")
                        } else {
                            format!("{base_where} AND {clause_sql}")
                        }
                    }
                    _ => base_where.clone(),
                };
                let sql = format!("SELECT COUNT(*) FROM logs{where_sql}");
                (sql, p)
            };

        if let Some(term) = qu.q.as_deref().filter(|t| !t.trim().is_empty()) {
            let (sql, p) = make_sql(
                Some(|n| format!("logs_fts MATCH ?{n}")),
                Some(Value::Text(term.to_string())),
            );
            return match self.count_once(&sql, &p) {
                Ok(n) if n > 0 => Ok(n),
                Ok(_) | Err(_) => {
                    // Same fallback rule as `query`: zero FTS rows, or a
                    // malformed-FTS error — LIKE answers instead.
                    let like = format!("%{}%", Self::escape_like(term));
                    let (sql, p) = make_sql(
                        Some(|n| format!("msg LIKE ?{n} ESCAPE '!'")),
                        Some(Value::Text(like)),
                    );
                    self.count_once(&sql, &p)
                }
            };
        }

        let (sql, p) = make_sql(None, None);
        self.count_once(&sql, &p)
    }

    /// Execute a fully-formed `SELECT COUNT(*)` (params bound positionally).
    fn count_once(&self, sql: &str, params: &[Value]) -> Result<i64> {
        let n: i64 = self
            .conn
            .query_row(sql, params_from_iter(params.iter().cloned()), |row| {
                row.get(0)
            })
            .with_context(|| format!("run log count: {sql}"))?;
        Ok(n)
    }

    /// Shared WHERE-clause builder for `query` and `count`: `min_level`,
    /// `since`, `until`, and `source` (exact OR delimiter-aware prefix in
    /// ONE parameterized clause — the `:` before the wildcard keeps
    /// `tamad:gpu-box` from over-matching `tamad:gpu-boxer`. User input is
    /// never string-concatenated into SQL).
    fn base_clauses(qu: &LogQuery) -> (Vec<Value>, Vec<String>) {
        let mut params: Vec<Value> = Vec::new();
        let mut clauses: Vec<String> = Vec::new();

        if let Some(min_level) = qu.min_level {
            params.push(Value::Integer(min_level.as_u8() as i64));
            clauses.push(format!("level >= ?{}", params.len()));
        }
        if let Some(since) = qu.since {
            params.push(Value::Integer(since));
            clauses.push(format!("ts >= ?{}", params.len()));
        }
        if let Some(until) = qu.until {
            params.push(Value::Integer(until));
            clauses.push(format!("ts < ?{}", params.len()));
        }
        if let Some(source) = &qu.source {
            params.push(Value::Text(source.as_str().to_string()));
            let exact_slot = params.len();
            let prefix = format!("{}:", Self::escape_like(source.as_str()));
            params.push(Value::Text(prefix));
            let prefix_slot = params.len();
            clauses.push(format!(
                "(source = ?{exact_slot} OR source LIKE ?{prefix_slot} || '%' ESCAPE {esc})",
                esc = LIKE_ESCAPE
            ));
        }
        (params, clauses)
    }

    /// Escape `LIKE` wildcards using `!` as the escape character (SQL side
    /// uses `ESCAPE '!'`) so query text is treated literally.
    fn escape_like(s: &str) -> String {
        s.replace('!', "!!").replace('%', "!%").replace('_', "!_")
    }

    /// Prepare/execute a fully-formed log query; fetches `limit + 1` rows,
    /// truncates, and derives `next_cursor` from the overflowing row.
    fn run_query(
        &self,
        sql: &str,
        params: &[Value],
        limit: i64,
    ) -> Result<(Vec<LogEntry>, Option<i64>)> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .with_context(|| format!("prepare log query: {sql}"))?;
        let raws = stmt
            .query_map(params_from_iter(params.iter().cloned()), |row| {
                let id: i64 = row.get(0)?;
                let ts: i64 = row.get(1)?;
                let level_raw: i64 = row.get(2)?;
                let source_raw: String = row.get(3)?;
                let msg_raw: String = row.get(4)?;
                Ok((id, ts, level_raw, source_raw, msg_raw))
            })
            .with_context(|| format!("run log query: {sql}"))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut entries = Vec::with_capacity(raws.len());
        for (id, ts, level_raw, source_raw, msg_raw) in raws {
            let level = LogstoreLevel::from_u8(level_raw as u8).ok_or_else(|| {
                anyhow::anyhow!("log row {id} stored out-of-domain level {level_raw}")
            })?;
            let source = Source::parse(&source_raw)
                .ok_or_else(|| anyhow::anyhow!("log row {id} stored empty source"))?;
            let msg: serde_json::Value = serde_json::from_str(&msg_raw)
                .with_context(|| format!("log row {id} stored non-JSON msg"))?;
            entries.push(LogEntry {
                id,
                ts,
                level,
                source,
                msg,
            });
        }
        // Fetched limit+1: an overflowing row proves the window continues.
        // next_cursor = last returned entry's id (id < cursor / id > cursor
        // on the next page), None when the window end has been reached.
        let next_cursor = if entries.len() > limit as usize {
            entries.truncate(limit as usize);
            entries.last().map(|e| e.id)
        } else {
            None
        };
        Ok((entries, next_cursor))
    }

    /// Distinct sources with their latest ts, newest source first.
    pub fn distinct_sources(&self) -> Result<Vec<SourceInfo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source, MAX(ts) FROM logs GROUP BY source ORDER BY MAX(ts) DESC")
            .context("prepare distinct_sources")?;
        let raws = stmt
            .query_map([], |row| {
                let source_raw: String = row.get(0)?;
                let last_ts: i64 = row.get(1)?;
                Ok((source_raw, last_ts))
            })
            .with_context(|| "run distinct_sources")?
            .collect::<Result<Vec<_>, _>>()?;
        let mut sources = Vec::with_capacity(raws.len());
        for (source_raw, last_ts) in raws {
            let source =
                Source::parse(&source_raw).ok_or_else(|| anyhow::anyhow!("stored empty source"))?;
            sources.push(SourceInfo { source, last_ts });
        }
        Ok(sources)
    }

    /// Per-level row counts for rows with `ts >= since_ms`, ordered by
    /// level (only levels with rows appear).
    ///
    /// The `(level, ts)` index does not cover a ts-only scan — fine at the
    /// 50k-row scale (note if it ever hurts; do not add an index preemptively).
    pub fn level_counts_since(&self, since_ms: i64) -> Result<Vec<LevelCount>> {
        let mut stmt = self
            .conn
            .prepare("SELECT level, COUNT(*) FROM logs WHERE ts >= ? GROUP BY level ORDER BY level")
            .context("prepare level_counts_since")?;
        let raws = stmt
            .query_map([since_ms], |row| {
                let level_raw: i64 = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((level_raw, count))
            })
            .with_context(|| "run level_counts_since")?
            .collect::<Result<Vec<_>, _>>()?;
        let mut counts = Vec::with_capacity(raws.len());
        for (level_raw, count) in raws {
            let level = LogstoreLevel::from_u8(level_raw as u8)
                .ok_or_else(|| anyhow::anyhow!("stored out-of-domain level {level_raw}"))?;
            counts.push(LevelCount { level, count });
        }
        Ok(counts)
    }

    /// Retention prune: compute the id watermark `W` such that keeping
    /// `id >= W` satisfies ALL bounds (`ts >= now - max_age`, row count
    /// `<= max_rows`, estimated kept bytes `<= max_bytes`), delete every
    /// older row in `DELETE_CHUNK_ROWS`-sized id-range batches, then run
    /// `PRAGMA incremental_vacuum(4096)` (mandatory — omitting it leaves
    /// the file growing monotonically, see openai/codex#35823).
    ///
    /// Returns rows deleted; a no-op (apart from the vacuum step) when
    /// already within bounds.
    pub fn prune(&self, b: &PruneBounds) -> Result<i64> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM logs", [], |r| r.get(0))
            .context("prune: count rows")?;
        if total == 0 {
            self.incremental_vacuum()
                .context("prune: empty store, incremental_vacuum")?;
            return Ok(0);
        }

        let min_id: i64 = self
            .conn
            .query_row("SELECT MIN(id) FROM logs", [], |r| r.get(0))
            .context("prune: min id")?;
        let last_id: i64 = self
            .conn
            .query_row("SELECT MAX(id) FROM logs", [], |r| r.get(0))
            .context("prune: max id")?;
        let now_ms = Self::now_unix_ms();

        // Age bound: watermark = oldest id of rows that are not aged out.
        // No surviving row → everything is delete budget (last_id + 1).
        let age_cutoff = now_ms - b.max_age_secs * 1000;
        let age_w: i64 = self
            .conn
            .query_row(
                "SELECT MIN(id) FROM logs WHERE ts >= ?",
                [age_cutoff],
                |r| r.get::<_, Option<i64>>(0),
            )
            .context("prune: age watermark")?
            .unwrap_or(last_id + 1);

        // Row-count bound: W = id of the (count - max_rows)-th oldest row,
        // clamped when already under the bound.
        let rows_w: i64 = if total > b.max_rows {
            let offset = total - b.max_rows;
            self.conn
                .query_row(
                    "SELECT id FROM logs ORDER BY id ASC LIMIT 1 OFFSET ?",
                    [offset],
                    |r| r.get(0),
                )
                .context("prune: row-count watermark")?
        } else {
            min_id
        };

        // Bytes bound: estimated kept bytes (SUM of LENGTH(msg)+LENGTH(source))
        // must fit max_bytes — walk the oldest rows first in chunks and
        // accumulate until the delete budget (total - max_bytes) is used.
        let total_bytes: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(msg) + LENGTH(source)), 0) FROM logs",
                [],
                |r| r.get(0),
            )
            .context("prune: total estimated bytes")?;
        let bytes_w: i64 = {
            let delete_budget = total_bytes - b.max_bytes;
            if delete_budget <= 0 {
                min_id
            } else {
                let mut stmt = self
                    .conn
                    .prepare(concat!(
                        "SELECT id, LENGTH(msg) + LENGTH(source) ",
                        "FROM logs WHERE id >= ? ORDER BY id ASC LIMIT ?",
                    ))
                    .context("prune: byte scan prepare")?;
                let mut lo = min_id;
                let mut accounted = 0i64;
                let mut watermark = None;
                loop {
                    let chunk: Vec<(i64, i64)> = stmt
                        .query_map([lo, BYTE_SCAN_CHUNK_ROWS], |r| Ok((r.get(0)?, r.get(1)?)))
                        .context("prune: byte scan query")?
                        .collect::<Result<Vec<_>, _>>()?;
                    if chunk.is_empty() {
                        break;
                    }
                    let chunk_sum: i64 = chunk.iter().map(|(_, n)| *n).sum();
                    if accounted + chunk_sum <= delete_budget {
                        accounted += chunk_sum;
                        lo = chunk.last().expect("non-empty chunk").0 + 1;
                        continue;
                    }
                    for (id, n) in &chunk {
                        accounted += n;
                        if accounted >= delete_budget {
                            // Rows up to and including `id` use up the
                            // delete budget; keep from the next id onward.
                            // Id ordering preserves recency, so gaps are
                            // safe.
                            watermark = Some(id + 1);
                            break;
                        }
                    }
                    if watermark.is_some() {
                        break;
                    }
                    // Unreachable given the check above; advance anyway.
                    lo = chunk.last().expect("non-empty chunk").0 + 1;
                }
                watermark.unwrap_or(min_id)
            }
        };

        // Keeping id >= max of the three watermarks satisfies every bound.
        let watermark = age_w.max(rows_w).max(bytes_w);
        if watermark <= min_id {
            // Already within bounds — still run the (no-op) vacuum step.
            self.incremental_vacuum()
                .context("prune: within bounds, incremental_vacuum")?;
            return Ok(0);
        }

        let deleted = self.delete_ids_below(watermark)?;
        self.incremental_vacuum()
            .context("prune: incremental_vacuum")?;
        Ok(deleted)
    }

    /// Delete every row with `id < watermark` in id-range batches of
    /// `DELETE_CHUNK_ROWS`; returns rows deleted. (SQLite has no
    /// `DELETE … LIMIT`; the range form loops instead.)
    fn delete_ids_below(&self, watermark: i64) -> Result<i64> {
        let mut deleted = 0i64;
        loop {
            let remaining: i64 = self
                .conn
                .query_row("SELECT COUNT(*) FROM logs WHERE id < ?", [watermark], |r| {
                    r.get(0)
                })
                .context("chunked delete: remaining count")?;
            if remaining == 0 {
                break;
            }
            let chunk_lo: i64 = self
                .conn
                .query_row("SELECT MIN(id) FROM logs WHERE id < ?", [watermark], |r| {
                    r.get(0)
                })
                .context("chunked delete: chunk start id")?;
            // Never overshoot the watermark with the chunk bound.
            let chunk_hi = (chunk_lo + DELETE_CHUNK_ROWS).min(watermark);
            let n = self
                .conn
                .execute(
                    "DELETE FROM logs WHERE id >= ? AND id < ?",
                    (chunk_lo, chunk_hi),
                )
                .with_context(|| format!("chunked delete [{chunk_lo}, {chunk_hi})"))?;
            deleted += n as i64;
        }
        Ok(deleted)
    }

    /// `PRAGMA incremental_vacuum(4096)` — mandatory after deletes so freed
    /// pages are reclaimed; without it an INCREMENTAL-auto_vacuum database
    /// grows monotonically (anti openai/codex#35823).
    fn incremental_vacuum(&self) -> Result<()> {
        self.conn
            .pragma_update(None, "incremental_vacuum", INCREMENTAL_VACUUM_PAGES)
            .context("PRAGMA incremental_vacuum")
    }

    /// Current unix time in milliseconds.
    fn now_unix_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Highest row id, `None` when the store is empty.
    pub fn last_id(&self) -> Result<Option<i64>> {
        self.conn
            .query_row("SELECT MAX(id) FROM logs", [], |r| r.get(0))
            .context("last_id")
    }

    /// Delete every row in id-range chunks of `DELETE_CHUNK_ROWS` + the
    /// final `incremental_vacuum`; returns count deleted (task 4's
    /// DELETE-all endpoint builds on this).
    pub fn delete_all(&self) -> Result<i64> {
        let mut deleted = 0i64;
        loop {
            let chunk_lo: Option<i64> = self
                .conn
                .query_row("SELECT MIN(id) FROM logs", [], |r| r.get(0))
                .context("delete_all: min id")?;
            let Some(lo) = chunk_lo else {
                break;
            };
            let hi = lo + DELETE_CHUNK_ROWS;
            let n = self
                .conn
                .execute("DELETE FROM logs WHERE id >= ? AND id < ?", (lo, hi))
                .with_context(|| format!("delete_all chunk [{lo}, {hi})"))?;
            if n == 0 {
                break; // safety against a pathological re-read
            }
            deleted += n as i64;
        }
        self.incremental_vacuum()
            .context("delete_all: incremental_vacuum")?;
        Ok(deleted)
    }

    /// Test-only fault injection: make the next `insert_batch` fail exactly
    /// once with `Err("injected")`.
    #[cfg(test)]
    pub fn fail_next_insert_for_tests(&self, on: bool) {
        self.fail_next
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(ts: i64, level: LogstoreLevel, source: &str, msg: serde_json::Value) -> LogRecord {
        LogRecord {
            ts,
            level,
            source: Source::parse(source).expect("valid source in test"),
            msg,
        }
    }

    /// Default `{"message": ...}` JSON document.
    fn msg(v: &str) -> serde_json::Value {
        json!({ "message": v })
    }

    /// Shared in-memory store (fresh per test).
    fn store() -> LogStore {
        LogStore::in_memory().expect("in_memory")
    }

    /// Empty query (all defaults: any level/source, newest first).
    fn q() -> LogQuery {
        LogQuery::default()
    }

    /// Insert the example record set used by the filter/sort tests
    /// (proxy dominant, two backends, one tamad host+model pair).
    fn insert_example_set(s: &LogStore) {
        s.insert_batch(&[
            rec(100, LogstoreLevel::TRACE, "proxy", msg("proxy boot")),
            rec(200, LogstoreLevel::INFO, "proxy", msg("proxy hot")),
            rec(
                300,
                LogstoreLevel::WARN,
                "backend:llama-cpp",
                msg("backend warn"),
            ),
            rec(400, LogstoreLevel::ERROR, "proxy", msg("proxy error")),
            rec(
                500,
                LogstoreLevel::WARN,
                "tamad:host1",
                msg("tamad host warn"),
            ),
            rec(
                600,
                LogstoreLevel::INFO,
                "tamad:host1:model:qwen3:8b",
                json!({"message": "tamad model info", "model": "qwen3:8b"}),
            ),
        ])
        .expect("insert example set");
    }

    fn entry_ids(entries: &[LogEntry]) -> Vec<i64> {
        entries.iter().map(|e| e.id).collect()
    }

    #[test]
    fn test_open_insert_query_roundtrip() {
        let s = store();
        let ids = s
            .insert_batch(&[
                rec(1000, LogstoreLevel::INFO, "proxy", msg("first")),
                rec(
                    2000,
                    LogstoreLevel::ERROR,
                    "backend:llama-cpp",
                    msg("second"),
                ),
                rec(
                    3000,
                    LogstoreLevel::DEBUG,
                    "tamad:host1",
                    json!({"message": "third", "extra": 1}),
                ),
            ])
            .expect("insert");
        assert_eq!(ids, vec![1, 2, 3]);

        let (entries, next) = s.query(&q()).expect("query");
        assert!(next.is_none(), "small result: window end reached");
        assert_eq!(entry_ids(&entries), vec![3, 2, 1], "default order is Desc");
        assert_eq!(entries[0].ts, 3000);
        assert_eq!(entries[0].level, LogstoreLevel::DEBUG);
        assert_eq!(entries[0].source.as_str(), "tamad:host1");
        assert_eq!(entries[0].msg, json!({"message": "third", "extra": 1}));
        assert_eq!(s.last_id().expect("last_id"), Some(3));
    }

    #[test]
    fn test_fts_match() {
        let s = store();
        s.insert_batch(&[
            rec(1, LogstoreLevel::INFO, "proxy", msg("wave and gimlet")),
            rec(2, LogstoreLevel::INFO, "proxy", msg("lorem ipsum dolor")),
        ])
        .expect("insert");

        let mut query = q();
        query.q = Some("lorem".into());
        let (entries, next) = s.query(&query).expect("query");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, 2);
        assert!(next.is_none());
    }

    #[test]
    fn test_fts_zero_rows_falls_back_to_like() {
        // FTS5 has no prefix matching: MATCH 'tokeni' finds nothing, so the
        // LIKE '%tokeni%' fallback path must produce the substring match.
        let s = store();
        s.insert_batch(&[
            rec(
                1,
                LogstoreLevel::INFO,
                "proxy",
                msg("partial tokenization pass"),
            ),
            rec(2, LogstoreLevel::INFO, "proxy", msg("zebra crusade")),
        ])
        .expect("insert");

        let mut query = q();
        query.q = Some("tokeni".into());
        let (entries, _) = s.query(&query).expect("query did not fall back");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, 1, "LIKE matched the substring");
    }

    #[test]
    fn test_malformed_fts_falls_back_to_like() {
        // A bare FTS operator keyword makes MATCH raise a rusqlite error —
        // that error must not propagate; the LIKE fallback answers instead.
        let s = store();
        s.insert_batch(&[
            rec(1, LogstoreLevel::INFO, "proxy", msg("GROUP AND HAVING")),
            rec(2, LogstoreLevel::INFO, "proxy", msg("silence")),
        ])
        .expect("insert");

        let mut query = q();
        query.q = Some("AND".into());
        let (entries, _) = s
            .query(&query)
            .expect("malformed FTS must not surface as error");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, 1);
    }

    #[test]
    fn test_source_exact_prefix_and_no_overmatch() {
        let s = store();
        s.insert_batch(&[
            rec(1, LogstoreLevel::INFO, "tamad:gpu-box", msg("a")),
            rec(2, LogstoreLevel::INFO, "tamad:gpu-box:model:x", msg("b")),
            rec(3, LogstoreLevel::INFO, "tamad:gpu-boxer", msg("c")),
            rec(4, LogstoreLevel::INFO, "proxy", msg("d")),
        ])
        .expect("insert");

        // "tamad:gpu-box" matches itself (exact) + its model child (prefix)
        // but NOT "tamad:gpu-boxer" (delimiter-aware prefix appends ':').
        let mut query = q();
        query.source = Some(Source::parse("tamad:gpu-box").unwrap());
        let (entries, _) = s.query(&query).expect("query");
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str().to_string())
                .collect::<Vec<_>>(),
            vec![
                "tamad:gpu-box:model:x".to_string(),
                "tamad:gpu-box".to_string()
            ],
            "exact + prefix, but no gpu-boxer overmatch"
        );

        // Leaf source: only the exact match.
        let mut query = q();
        query.source = Some(Source::parse("tamad:gpu-box:model:x").unwrap());
        let (entries, _) = s.query(&query).expect("query");
        assert_eq!(entry_ids(&entries), vec![2]);

        // Source with no prefix children: exact only.
        let mut query = q();
        query.source = Some(Source::parse("proxy").unwrap());
        let (entries, _) = s.query(&query).expect("query");
        assert_eq!(entry_ids(&entries), vec![4]);
    }

    #[test]
    fn test_cursor_pagination_desc() {
        let s = store();
        let rows: Vec<LogRecord> = (1..=10i64)
            .map(|i| rec(i * 10, LogstoreLevel::INFO, "proxy", msg(&format!("m{i}"))))
            .collect();
        s.insert_batch(&rows).expect("insert");

        let mut query = q();
        query.limit = Some(3);

        let (page, next) = s.query(&query).expect("p1");
        assert_eq!(entry_ids(&page), vec![10, 9, 8]);
        assert_eq!(next, Some(8));

        query.cursor = next;
        let (page, next) = s.query(&query).expect("p2");
        assert_eq!(entry_ids(&page), vec![7, 6, 5]);
        assert_eq!(next, Some(5));

        query.cursor = next;
        let (page, next) = s.query(&query).expect("p3");
        assert_eq!(entry_ids(&page), vec![4, 3, 2]);
        assert_eq!(next, Some(2));

        query.cursor = next;
        let (page, next) = s.query(&query).expect("p4 — final");
        assert_eq!(entry_ids(&page), vec![1]);
        assert!(next.is_none(), "window end reached");
    }

    #[test]
    fn test_cursor_pagination_asc() {
        let s = store();
        let rows: Vec<LogRecord> = (1..=7i64)
            .map(|i| {
                rec(
                    i * 10,
                    LogstoreLevel::INFO,
                    "backend:llama-cpp",
                    msg(&format!("a{i}")),
                )
            })
            .collect();
        s.insert_batch(&rows).expect("insert");

        let mut query = q();
        query.limit = Some(3);
        query.order = QueryOrder::Asc;

        let (page, next) = s.query(&query).expect("p1");
        assert_eq!(entry_ids(&page), vec![1, 2, 3]);
        assert_eq!(next, Some(3));

        query.cursor = next;
        let (page, next) = s.query(&query).expect("p2");
        assert_eq!(entry_ids(&page), vec![4, 5, 6]);
        assert_eq!(next, Some(6));

        query.cursor = next;
        let (page, next) = s.query(&query).expect("p3 — final");
        assert_eq!(entry_ids(&page), vec![7]);
        assert!(next.is_none());
    }

    #[test]
    fn test_filters_level_since_until() {
        let s = store();
        insert_example_set(&s);

        // min_level=warn
        let mut query = q();
        query.min_level = Some(LogstoreLevel::WARN);
        let (entries, _) = s.query(&query).expect("min_level");
        assert_eq!(entry_ids(&entries), vec![5, 4, 3]);

        // since is inclusive
        let mut query = q();
        query.since = Some(300);
        let (entries, _) = s.query(&query).expect("since");
        assert_eq!(entry_ids(&entries), vec![6, 5, 4, 3]);

        // until is exclusive (row id 4 at ts 400 is excluded)
        let mut query = q();
        query.until = Some(400);
        let (entries, _) = s.query(&query).expect("until");
        assert_eq!(entry_ids(&entries), vec![3, 2, 1]);

        // combined: min_level=info AND since=150 AND until=500
        let mut query = q();
        query.min_level = Some(LogstoreLevel::INFO);
        query.since = Some(150);
        query.until = Some(500);
        let (entries, _) = s.query(&query).expect("combined");
        assert_eq!(entry_ids(&entries), vec![4, 3, 2]);
    }

    #[test]
    fn test_limit_default_200_and_clamp_1000() {
        let s = store();
        let rows: Vec<LogRecord> = (0..1500i64)
            .map(|i| rec(i, LogstoreLevel::INFO, "proxy", msg(&format!("row {i}"))))
            .collect();
        s.insert_batch(&rows).expect("insert 1500");

        // default limit = 200
        let (entries, next) = s.query(&q()).expect("default limit");
        assert_eq!(entries.len(), 200);
        assert_eq!(next, Some(1301), "next page starts at id 1301");

        // oversized limit clamps to 1000 (clamp, not error)
        let mut query = q();
        query.limit = Some(99_999);
        let (entries, next) = s.query(&query).expect("clamp");
        assert_eq!(entries.len(), 1000);
        assert_eq!(next, Some(501));
    }

    #[test]
    fn test_level_counts_since() {
        let s = store();
        insert_example_set(&s);
        // (300 warn) (400 error) (600 info) plus a debug at 700
        s.insert_batch(&[rec(700, LogstoreLevel::DEBUG, "proxy", msg("debug line"))])
            .expect("insert");

        // full window: counts for levels that exist, ordered by level
        let counts = s.level_counts_since(0).expect("counts");
        assert_eq!(
            counts
                .iter()
                .map(|c| (c.level, c.count))
                .collect::<Vec<_>>(),
            vec![
                (LogstoreLevel::TRACE, 1),
                (LogstoreLevel::DEBUG, 1),
                (LogstoreLevel::INFO, 2),
                (LogstoreLevel::WARN, 2),
                (LogstoreLevel::ERROR, 1),
            ]
        );

        // since=500 is inclusive: rows with ts < 500 are excluded
        let counts = s.level_counts_since(500).expect("counts since");
        assert_eq!(
            counts
                .iter()
                .map(|c| (c.level, c.count))
                .collect::<Vec<_>>(),
            vec![
                (LogstoreLevel::DEBUG, 1),
                (LogstoreLevel::INFO, 1),
                (LogstoreLevel::WARN, 1),
            ],
            "ts 500 (WARN) is included, ts < 500 excluded"
        );
    }

    #[test]
    fn test_distinct_sources() {
        let s = store();
        s.insert_batch(&[
            rec(100, LogstoreLevel::INFO, "proxy", msg("a")),
            rec(500, LogstoreLevel::INFO, "proxy", msg("b")),
            rec(300, LogstoreLevel::INFO, "backend:llama-cpp", msg("c")),
            rec(250, LogstoreLevel::INFO, "backend:lmstudio", msg("d")),
        ])
        .expect("insert");

        let sources = s.distinct_sources().expect("sources");
        assert_eq!(
            sources
                .iter()
                .map(|x| (x.source.as_str().to_string(), x.last_ts))
                .collect::<Vec<_>>(),
            vec![
                ("proxy".to_string(), 500),
                ("backend:llama-cpp".to_string(), 300),
                ("backend:lmstudio".to_string(), 250),
            ],
            "ordered newest-source first, last_ts = per-source max"
        );
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis() as i64
    }

    #[test]
    fn test_prune_by_age() {
        let s = store();
        let now = now_ms();
        // 5 old rows (older than the 60s bound) + 5 fresh rows
        let old: Vec<LogRecord> = (1..=5i64)
            .map(|i| {
                rec(
                    now - 500_000 + i,
                    LogstoreLevel::INFO,
                    "proxy",
                    msg(&format!("old{i}")),
                )
            })
            .collect();
        let fresh: Vec<LogRecord> = (6..=10i64)
            .map(|i| {
                rec(
                    now - 10_000 + i,
                    LogstoreLevel::INFO,
                    "proxy",
                    msg(&format!("new{i}")),
                )
            })
            .collect();
        s.insert_batch(&old).expect("old");
        s.insert_batch(&fresh).expect("fresh");

        let bounds = PruneBounds {
            max_age_secs: 60,
            max_rows: 10_000,
            max_bytes: 100_000_000,
        };
        let deleted = s.prune(&bounds).expect("prune");
        assert_eq!(deleted, 5, "only the aged rows are deleted");

        let (entries, _) = s.query(&q()).expect("query after prune");
        assert_eq!(entries.len(), 5);
        assert!(entries.iter().all(|e| e.ts >= now - 60_000));

        // idempotent: already within bounds → 0 deleted
        let deleted = s.prune(&bounds).expect("prune again");
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_prune_by_rows() {
        let s = store();
        let now = now_ms();
        // Fresh, monotonic-with-id timestamps (age bound not the constraint).
        let rows: Vec<LogRecord> = (1..=50i64)
            .map(|i| {
                rec(
                    now - 100_000 + i,
                    LogstoreLevel::INFO,
                    "proxy",
                    msg(&format!("row{i}")),
                )
            })
            .collect();
        s.insert_batch(&rows).expect("insert");

        let bounds = PruneBounds {
            max_age_secs: 100_000_000_i64, // age bound is not the constraint
            max_rows: 40,
            max_bytes: 100_000_000,
        };
        let deleted = s.prune(&bounds).expect("prune");
        assert_eq!(deleted, 10, "oldest 10 rows drop out");

        let (entries, _) = s.query(&q()).expect("query");
        assert_eq!(entries.len(), 40);
        assert_eq!(entries[0].id, 50, "newest rows survive");
    }

    #[test]
    fn test_prune_by_bytes() {
        let s = store();
        // 30 rows × ~350 bytes (documents + source labels) ≈ 10.5 KB total
        let payload = "x".repeat(300);
        let now = now_ms();
        // Fresh, monotonic-with-id timestamps (age bound not the constraint).
        let rows: Vec<LogRecord> = (1..=30i64)
            .map(|i| {
                rec(
                    now - 100_000 + i,
                    LogstoreLevel::INFO,
                    "backend:llama-cpp",
                    msg(&format!("row{i} {payload}")),
                )
            })
            .collect();
        s.insert_batch(&rows).expect("insert");

        let bounds = PruneBounds {
            max_age_secs: 100_000_000,
            max_rows: 100_000,
            max_bytes: 5_000,
        };
        let deleted = s.prune(&bounds).expect("prune");
        assert!(deleted > 0, "bytes bound must delete something");

        let (entries, _) = s.query(&q()).expect("query");
        let kept_bytes: i64 = entries
            .iter()
            .map(|e| (e.msg.to_string().len() + e.source.as_str().len()) as i64)
            .sum();
        assert!(
            kept_bytes <= bounds.max_bytes,
            "kept estimated bytes {kept_bytes} must fit the budget"
        );
        assert_eq!(entries[0].id, 30, "newest row survives");
    }

    #[test]
    fn test_prune_within_bounds_deletes_zero() {
        let s = store();
        let now = now_ms();
        s.insert_batch(&[rec(now, LogstoreLevel::INFO, "proxy", msg("small"))])
            .expect("insert");
        let bounds = PruneBounds {
            max_age_secs: 100_000_000,
            max_rows: 1000,
            max_bytes: 1_000_000,
        };
        let deleted = s.prune(&bounds).expect("prune");
        assert_eq!(deleted, 0);
        let (entries, _) = s.query(&q()).expect("query");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_injected_insert_failure_fires_once_and_preserves_subsequent() {
        let s = store();
        s.fail_next_insert_for_tests(true);

        let first = s.insert_batch(&[rec(1, LogstoreLevel::INFO, "proxy", msg("lost"))]);
        assert!(first.is_err(), "fault-injected insert must fail");
        assert!(
            first.as_ref().unwrap_err().to_string().contains("injected"),
            "error carries the injected marker"
        );
        let (entries, _) = s.query(&q()).expect("query");
        assert!(entries.is_empty(), "fault did not persist anything");

        // fault fired exactly once: the same batch now succeeds
        let ids = s
            .insert_batch(&[rec(1, LogstoreLevel::INFO, "proxy", msg("kept"))])
            .expect("re-inserted");
        assert_eq!(ids, vec![1]);

        // subsequent batches keep working and stay in order
        let ids = s
            .insert_batch(&[rec(2, LogstoreLevel::WARN, "proxy", msg("next"))])
            .expect("second batch");
        assert_eq!(ids, vec![2]);
        let (entries, _) = s.query(&q()).expect("query");
        assert_eq!(entry_ids(&entries), vec![2, 1]);
    }

    #[test]
    fn test_count_matches_query_filters() {
        let s = store();
        insert_example_set(&s);

        // No filters: every row.
        assert_eq!(s.count(&q()).expect("count all"), 6);

        // min_level, since, until — same clauses as query().
        let mut query = q();
        query.min_level = Some(LogstoreLevel::WARN);
        assert_eq!(s.count(&query).expect("count min_level"), 3);

        let mut query = q();
        query.since = Some(300);
        query.until = Some(500);
        assert_eq!(s.count(&query).expect("count window"), 2);

        // Source exact + delimiter-aware prefix (same clause as query()).
        let mut query = q();
        query.source = Some(Source::parse("tamad:host1").unwrap());
        assert_eq!(s.count(&query).expect("count source"), 2);

        // FTS hit counts the FTS match set…
        let mut query = q();
        query.q = Some("proxy".into());
        assert_eq!(s.count(&query).expect("count fts"), 3);

        // …and a term no FTS token contains falls back to LIKE (zero rows,
        // not an error — the export cap check must never 500 on search text).
        let mut query = q();
        query.q = Some("[not fts (".into());
        assert_eq!(s.count(&query).expect("count fall back"), 0);
    }

    #[test]
    fn test_delete_all_chunks_10k_batches() {
        let s = store();
        let total = 25_000i64;
        let mut remaining = total;
        let mut ts = 0;
        while remaining > 0 {
            let n = remaining.min(10_000);
            ts += n;
            let rows: Vec<LogRecord> = (0..n)
                .map(|i| {
                    rec(
                        ts - n + i,
                        LogstoreLevel::INFO,
                        "proxy",
                        json!({"message": "bulk"}),
                    )
                })
                .collect();
            s.insert_batch(&rows).expect("bulk insert");
            remaining -= n;
        }
        assert_eq!(s.last_id().expect("last_id"), Some(total));

        let deleted = s.delete_all().expect("delete_all");
        assert_eq!(deleted, total, "multi-chunk delete counts all rows");
        assert_eq!(s.last_id().expect("last_id"), None);
        let (entries, _) = s.query(&q()).expect("query");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_open_creates_parent_dirs_and_resumes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("logs").join("tama-logs.db");
        assert!(!path.exists());

        let s = LogStore::open(&path).expect("open creates parents");
        assert_eq!(s.path, path);
        assert!(path.exists());
        s.insert_batch(&[rec(1, LogstoreLevel::INFO, "proxy", msg("persist"))])
            .expect("insert");
        drop(s);

        // resume: rows survive a reopen
        let s2 = LogStore::open(&path).expect("re-open");
        let (entries, _) = s2.query(&q()).expect("query");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].msg, msg("persist"));
        drop(s2);

        // a second live connection on the same path (WAL allows it)
        let s3 = LogStore::open(&path).expect("concurrent open");
        let ids = s3
            .insert_batch(&[rec(2, LogstoreLevel::INFO, "proxy", msg("from conn 2"))])
            .expect("insert on conn 2");
        assert_eq!(ids, vec![2]);
        let (entries, _) = s3.query(&LogQuery::default()).expect("query conn 2");
        assert_eq!(entry_ids(&entries), vec![2, 1]);
    }

    #[test]
    fn test_open_migrates_legacy_auto_vacuum_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("legacy.db");
        {
            // A non-empty database created BEFORE this feature exists:
            // default auto_vacuum = 0 (NONE), carrying unrelated data.
            let c = Connection::open(&path).expect("legacy open");
            c.execute_batch("CREATE TABLE other (id INTEGER); INSERT INTO other VALUES (1);")
                .expect("legacy schema");
        }

        let s = LogStore::open(&path).expect("open migrates");
        let mode: i64 = s
            .conn
            .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
            .expect("auto_vacuum read back");
        assert_eq!(mode, 2, "INCREMENTAL after migration VACUUM");

        // legacy data survived the one-time VACUUM
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM other", [], |r| r.get(0))
            .expect("legacy table intact");
        assert_eq!(n, 1);

        // and the store is functional
        s.insert_batch(&[rec(1, LogstoreLevel::INFO, "proxy", msg("post-migration"))])
            .expect("insert");
        let (entries, _) = s.query(&q()).expect("query");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_logstore_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<LogStore>();
    }

    #[test]
    fn test_source_constructors_and_parse() {
        assert_eq!(Source::proxy().as_str(), "proxy");
        assert_eq!(Source::backend("llama-cpp").as_str(), "backend:llama-cpp");
        assert_eq!(Source::tamad("gpu-box").as_str(), "tamad:gpu-box");
        assert_eq!(
            Source::tamad_model("gpu-box", "qwen3:8b").as_str(),
            "tamad:gpu-box:model:qwen3:8b"
        );
        assert_eq!(
            Source::tamad_model_tail("gpu-box", "qwen3:8b").as_str(),
            "tamad:gpu-box:model:qwen3:8b:tail"
        );
        assert!(Source::parse("").is_none());
        assert!(Source::parse("   ").is_none());
        assert_eq!(Source::parse(" proxy ").unwrap().as_str(), "proxy");
    }

    #[test]
    fn test_level_domain() {
        assert_eq!(LogstoreLevel::TRACE.as_u8(), 0);
        assert_eq!(LogstoreLevel::DEBUG.as_u8(), 1);
        assert_eq!(LogstoreLevel::INFO.as_u8(), 2);
        assert_eq!(LogstoreLevel::WARN.as_u8(), 3);
        assert_eq!(LogstoreLevel::ERROR.as_u8(), 4);
        assert_eq!(LogstoreLevel::from_u8(4), Some(LogstoreLevel::ERROR));
        assert_eq!(LogstoreLevel::from_u8(5), None);
        assert_eq!(LogstoreLevel::WARN.as_str(), "warn");
        assert!(LogstoreLevel::INFO < LogstoreLevel::ERROR);

        // serde round-trip as a plain number
        let json = serde_json::to_string(&LogstoreLevel::ERROR).expect("serialize");
        assert_eq!(json, "4");
        let back: LogstoreLevel = serde_json::from_str("2").expect("deserialize");
        assert_eq!(back, LogstoreLevel::INFO);
    }

    #[test]
    fn test_query_default_limit_is_200_via_effective_limit() {
        let query = |limit: Option<i64>| LogQuery {
            limit,
            ..LogQuery::default()
        };
        assert_eq!(query(None).effective_limit(), 200);
        assert_eq!(query(Some(10_000)).effective_limit(), 1000);
        assert_eq!(query(Some(0)).effective_limit(), 1);
    }
}
