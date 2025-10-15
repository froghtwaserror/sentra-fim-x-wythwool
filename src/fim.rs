
use crate::config::Config;
use crate::metrics::Metrics;
use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher, EventKind, event::{ModifyKind, RenameMode}};
use rusqlite::{params, Connection, TransactionBehavior};
use std::{fs, path::{Path, PathBuf}, sync::mpsc, collections::HashMap};
use walkdir::WalkDir;
use globset::{Glob, GlobSetBuilder};
use serde::Serialize;
use tracing::{info, warn, debug};
use time::OffsetDateTime;

#[derive(Debug, Serialize)]
struct AuditEvent<'a> {
    ts: i128,
    kind: &'a str,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
}

pub fn build_baseline(cfg: &Config) -> Result<()> {
    let conn = Connection::open(&cfg.baseline_db)?;
    init_schema(&conn)?;
    let (globset, _) = build_excluder(&cfg.exclude)?;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM files", [])?;
    let mut count = 0usize;
    for root in &cfg.watch_paths {
        for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            let p = entry.path();
            if !p.is_file() { continue; }
            if is_excluded(p, &globset) { continue; }
            if let Ok(meta) = p.metadata() {
                let (hash, size, mtime) = hash_meta(p, cfg)?;
                let norm = normalize_path(p);
                tx.execute(
                    "INSERT OR REPLACE INTO files(path, hash, size, mtime) VALUES(?1, ?2, ?3, ?4)",
                    params![norm, hash, size as i64, mtime as i64]
                )?;
                count += 1;
            }
        }
    }
    tx.commit()?;
    info!("Baseline: {} files indexed (transactional)", count);
    Ok(())
}

pub async fn watch_loop(cfg: Config, jsonl_path: String, metrics: Metrics) -> Result<()> {
    let conn = Connection::open(&cfg.baseline_db)?;
    init_schema(&conn)?;

    // count tracked_files
    let tracked: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    metrics.tracked_files.set(tracked as i64);

    let (globset, _raw) = build_excluder(&cfg.exclude)?;

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default())
        .context("create watcher")?;

    for p in &cfg.watch_paths {
        watcher.watch(Path::new(p), RecursiveMode::Recursive)?;
    }
    info!("Watching {} roots", cfg.watch_paths.len());

    let mut jsonl = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jsonl_path)
        .context("open jsonl")?;

    // simple debounce map
    let mut last_evt: HashMap<String, i128> = HashMap::new();
    let window = cfg.debounce_ms as i128;

    loop {
        let event = rx.recv().expect("watcher channel closed");
        // Debug log kind
        debug!("event: kind={:?} paths={:?}", event.kind, event.paths);

        match &event.kind {
            EventKind::Modify(ModifyKind::Name(_mode)) => {
                // Try to handle rename with two paths (from, to)
                if event.paths.len() >= 2 {
                    let from = &event.paths[0];
                    let to = &event.paths[1];
                    if is_excluded(from, &globset) || is_excluded(to, &globset) {
                        continue;
                    }
                    if debounce_hit(&mut last_evt, from, window) && debounce_hit(&mut last_evt, to, window) {
                        continue;
                    }
                    if let Err(e) = handle_rename(&conn, from, to, &mut jsonl, &metrics, &cfg) {
                        warn!("rename handle error: {e}");
                    }
                } else {
                    // Fallback: treat as delete/create if we don't have both paths
                    for p in &event.paths {
                        if is_excluded(p, &globset) { continue; }
                        if debounce_hit(&mut last_evt, p, window) { continue; }
                    }
                }
            }
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                for p in event.paths {
                    if is_excluded(&p, &globset) { continue; }
                    if debounce_hit(&mut last_evt, &p, window) { continue; }
                    if let EventKind::Remove(_) = &event.kind {
                        if let Err(e) = handle_delete(&conn, &p, &mut jsonl, &metrics) {
                            warn!("delete handle error: {e}");
                        }
                    } else {
                        if let Err(e) = handle_upsert(&conn, &p, &mut jsonl, &metrics, &cfg) {
                            warn!("upsert handle error: {e}");
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn scan_diff(cfg: &Config, jsonl_out: Option<String>) -> Result<()> {
    let conn = Connection::open(&cfg.baseline_db)?;
    init_schema(&conn)?;

    let (globset, _) = build_excluder(&cfg.exclude)?;
    let mut added = 0usize;
    let mut changed = 0usize;
    let mut missing = 0usize;

    let mut known = std::collections::HashSet::new();
    let mut out = if let Some(p) = jsonl_out {
        Some(fs::OpenOptions::new().create(true).truncate(true).write(true).open(p)
            .context("open diff jsonl")?)
    } else { None };

    // check current FS for create/modify
    for root in &cfg.watch_paths {
        for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            let p = entry.path();
            if !p.is_file() { continue; }
            if is_excluded(p, &globset) { continue; }
            let meta = match p.metadata() { Ok(m) => m, Err(_) => continue };
            let (hash, size, mtime) = hash_meta(p, cfg)?;
            let norm = normalize_path(p);
            known.insert(norm.clone());

            let mut stmt = conn.prepare("SELECT hash, size, mtime FROM files WHERE path=?1")?;
            let row = stmt.query_row(params![norm.clone()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)));
            match row {
                Ok((old_hash, old_size, old_mtime)) => {
                    if old_hash != hash || old_size as u64 != size || old_mtime as u64 != mtime {
                        changed += 1;
                        if let Some(f) = &mut out {
                            write_jsonl(f, AuditEvent {
                                ts: now_ms(), kind: "changed", path: norm.clone(),
                                old_path: None, old_hash: Some(old_hash), new_hash: Some(hash), size: Some(size)
                            })?;
                        } else {
                            println!("CHANGED: {}", norm);
                        }
                    }
                }
                Err(_) => {
                    added += 1;
                    if let Some(f) = &mut out {
                        write_jsonl(f, AuditEvent {
                            ts: now_ms(), kind: "added", path: norm.clone(),
                            old_path: None, old_hash: None, new_hash: Some(hash), size: Some(size)
                        })?;
                    } else {
                        println!("ADDED: {}", norm);
                    }
                }
            }
        }
    }

    // find removed
    let mut stmt = conn.prepare("SELECT path FROM files")?;
    let paths = stmt.query_map([], |r| r.get::<_, String>(0))?;
    for r in paths {
        let path: String = r?;
        if !known.contains(&path) {
            missing += 1;
            if let Some(f) = &mut out {
                write_jsonl(f, AuditEvent {
                    ts: now_ms(), kind: "missing", path: path.clone(),
                    old_path: None, old_hash: None, new_hash: None, size: None
                })?;
            } else {
                println!("MISSING: {}", path);
            }
        }
    }

    println!("Summary -> added: {added}, changed: {changed}, missing: {missing}");
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(r#"
    PRAGMA journal_mode=WAL;
    CREATE TABLE IF NOT EXISTS files (
      path TEXT PRIMARY KEY,
      hash TEXT NOT NULL,
      size INTEGER NOT NULL,
      mtime INTEGER NOT NULL
    );
    "#)?;
    Ok(())
}

fn build_excluder(patterns: &[String]) -> Result<(globset::GlobSet, Vec<Glob>)> {
    let mut b = GlobSetBuilder::new();
    let mut raws = Vec::new();
    for p in patterns {
        let g = Glob::new(p)?;
        b.add(g.clone());
        raws.push(g);
    }
    Ok((b.build()?, raws))
}

fn is_excluded(p: &Path, set: &globset::GlobSet) -> bool {
    set.is_match(p)
}

fn hash_meta(p: &Path, cfg: &Config) -> Result<(String, u64, u64)> {
    // choose hasher
    let alg = cfg.hash_alg.to_lowercase();
    let mut f = fs::File::open(p)?;
    let size = f.metadata()?.len();
    let mtime = f.metadata()?.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();

    if alg == "sha256" {
        use sha2::{Sha256, Digest};
        use std::io::Read;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 { break; }
            hasher.update(&buf[..n]);
        }
        let res = hasher.finalize();
        let hash = format!("{:x}", res);
        Ok((hash, size, mtime))
    } else {
        // default blake3
        use std::io::{Read};
        let mut hasher = blake3::Hasher::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 { break; }
            hasher.update(&buf[..n]);
        }
        let res = hasher.finalize();
        Ok((res.to_hex().to_string(), size, mtime))
    }
}

fn handle_upsert(conn: &rusqlite::Connection, p: &Path, jsonl: &mut fs::File, metrics: &Metrics, cfg: &Config) -> Result<()> {
    if !p.is_file() { return Ok(()); }
    let (new_hash, size, mtime) = hash_meta(p, cfg)?;
    let norm = normalize_path(p);

    let mut stmt = conn.prepare("SELECT hash FROM files WHERE path=?1")?;
    let exists = stmt.exists(rusqlite::params![norm.clone()])?;

    let ts = now_ms();
    if exists {
        let old_hash: String = conn.query_row("SELECT hash FROM files WHERE path=?1", params![norm.clone()], |r| r.get(0))?;
        conn.execute("UPDATE files SET hash=?1, size=?2, mtime=?3 WHERE path=?4",
            params![new_hash.clone(), size as i64, mtime as i64, norm.clone()])?;

        if old_hash != new_hash {
            metrics.modified.inc();
            write_jsonl(jsonl, AuditEvent {
                ts, kind: "modify", path: norm, old_path: None,
                old_hash: Some(old_hash), new_hash: Some(new_hash), size: Some(size)
            })?;
        }
    } else {
        conn.execute("INSERT INTO files(path, hash, size, mtime) VALUES(?1, ?2, ?3, ?4)",
            params![norm.clone(), new_hash.clone(), size as i64, mtime as i64])?;
        metrics.created.inc();
        metrics.tracked_files.inc();
        write_jsonl(jsonl, AuditEvent {
            ts, kind: "create", path: norm, old_path: None,
            old_hash: None, new_hash: Some(new_hash), size: Some(size)
        })?;
    }
    Ok(())
}

fn handle_delete(conn: &rusqlite::Connection, p: &Path, jsonl: &mut fs::File, metrics: &Metrics) -> Result<()> {
    let norm = normalize_path(p);
    let ts = now_ms();
    let existed = conn.execute("DELETE FROM files WHERE path=?1", params![norm.clone()])?;
    if existed > 0 {
        metrics.deleted.inc();
        metrics.tracked_files.dec();
        write_jsonl(jsonl, AuditEvent {
            ts, kind: "delete", path: norm, old_path: None,
            old_hash: None, new_hash: None, size: None
        })?;
    }
    Ok(())
}

fn handle_rename(conn: &rusqlite::Connection, from: &Path, to: &Path, jsonl: &mut fs::File, _metrics: &Metrics, cfg: &Config) -> Result<()> {
    let from_n = normalize_path(from);
    let to_n = normalize_path(to);
    let ts = now_ms();

    let affected = conn.execute("UPDATE files SET path=?1 WHERE path=?2", params![to_n.clone(), from_n.clone()])?;
    if affected == 0 {
        // if row doesn't exist (e.g., watcher started after), insert fresh
        if to.is_file() {
            let (hash, size, mtime) = hash_meta(to, cfg)?;
            conn.execute("INSERT OR REPLACE INTO files(path, hash, size, mtime) VALUES(?1, ?2, ?3, ?4)",
                params![to_n.clone(), hash.clone(), size as i64, mtime as i64])?;
        }
    }
    let mut f = jsonl;
    write_jsonl(&mut f, AuditEvent {
        ts, kind: "rename", path: to_n, old_path: Some(from_n),
        old_hash: None, new_hash: None, size: None
    })?;
    Ok(())
}

fn write_jsonl(f: &mut fs::File, evt: AuditEvent<'_>) -> Result<()> {
    let line = serde_json::to_string(&evt)? + "\n";
    use std::io::Write;
    f.write_all(line.as_bytes())?;
    Ok(())
}

fn normalize_path(p: &Path) -> String {
    match dunce::canonicalize(p) {
        Ok(pp) => pp.to_string_lossy().to_string(),
        Err(_) => p.to_string_lossy().to_string(),
    }
}

fn debounce_hit(last: &mut HashMap<String, i128>, p: &Path, window_ms: i128) -> bool {
    let key = normalize_path(p);
    let now = now_ms();
    if let Some(prev) = last.get(&key) {
        if now - *prev <= window_ms {
            return true;
        }
    }
    last.insert(key, now);
    false
}

fn now_ms() -> i128 {
    let now = OffsetDateTime::now_utc();
    now.unix_timestamp_nanos() / 1_000_000
}
