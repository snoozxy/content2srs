use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use crate::config::BuildConfig;
use crate::errors::{AppError, Result};
use crate::jobplan::CardJob;
use crate::util::{format_timestamp_range, image_filename, media_filename, sanitize_filename};

const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Clone)]
pub struct CardRecord {
    pub id: String,
    pub cue_index: usize,
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub context: String,
    pub source: String,
    pub audio_filename: Option<String>,
    pub image_filename: Option<String>,
    pub audio_blob: Option<Vec<u8>>,
    pub image_blob: Option<Vec<u8>>,
    pub audio_sha256: Option<String>,
    pub image_sha256: Option<String>,
}

pub struct Bundle {
    conn: Connection,
    path: PathBuf,
}

impl Bundle {
    pub fn open(path: &Path, create: bool) -> Result<Self> {
        if create {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| AppError::Bundle(e.to_string()))?;
            }
        }
        let conn = Connection::open(path).map_err(|e| AppError::Bundle(e.to_string()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        let bundle = Self {
            conn,
            path: path.to_path_buf(),
        };
        if create {
            bundle.init_schema()?;
        }
        Ok(bundle)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS assets (
                    name TEXT PRIMARY KEY,
                    mime TEXT NOT NULL,
                    data BLOB NOT NULL
                );
                CREATE TABLE IF NOT EXISTS cards (
                    id TEXT PRIMARY KEY,
                    cue_index INTEGER NOT NULL,
                    text TEXT NOT NULL,
                    start_ms INTEGER NOT NULL,
                    end_ms INTEGER NOT NULL,
                    context TEXT NOT NULL DEFAULT '',
                    source TEXT NOT NULL DEFAULT '',
                    audio_filename TEXT,
                    image_filename TEXT,
                    audio_blob BLOB,
                    image_blob BLOB,
                    audio_sha256 TEXT,
                    image_sha256 TEXT,
                    status TEXT NOT NULL DEFAULT 'complete',
                    error TEXT,
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE INDEX IF NOT EXISTS idx_cards_cue ON cards(cue_index);
                "#,
            )
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM meta WHERE key = ?1")
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        let mut rows = stmt
            .query(params![key])
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        if let Some(row) = rows.next().map_err(|e| AppError::Bundle(e.to_string()))? {
            Ok(Some(row.get(0).map_err(|e| AppError::Bundle(e.to_string()))?))
        } else {
            Ok(None)
        }
    }

    pub fn store_subtitle_asset(&self, name: &str, mime: &str, data: &[u8]) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO assets(name, mime, data) VALUES(?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET mime=excluded.mime, data=excluded.data",
                params![name, mime, data],
            )
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        Ok(())
    }

    pub fn has_complete_card(&self, card_id: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT status FROM cards WHERE id = ?1")
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        let mut rows = stmt
            .query(params![card_id])
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        if let Some(row) = rows.next().map_err(|e| AppError::Bundle(e.to_string()))? {
            let status: String = row.get(0).map_err(|e| AppError::Bundle(e.to_string()))?;
            Ok(status == "complete")
        } else {
            Ok(false)
        }
    }

    pub fn commit_card(&self, record: &CardRecord) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        tx.execute(
            "INSERT INTO cards(
                id, cue_index, text, start_ms, end_ms, context, source,
                audio_filename, image_filename, audio_blob, image_blob,
                audio_sha256, image_sha256, status, error
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'complete',NULL)
            ON CONFLICT(id) DO UPDATE SET
                text=excluded.text, start_ms=excluded.start_ms, end_ms=excluded.end_ms,
                context=excluded.context, source=excluded.source,
                audio_filename=excluded.audio_filename, image_filename=excluded.image_filename,
                audio_blob=excluded.audio_blob, image_blob=excluded.image_blob,
                audio_sha256=excluded.audio_sha256, image_sha256=excluded.image_sha256,
                status='complete', error=NULL",
            params![
                record.id,
                record.cue_index as i64,
                record.text,
                record.start_ms as i64,
                record.end_ms as i64,
                record.context,
                record.source,
                record.audio_filename,
                record.image_filename,
                record.audio_blob,
                record.image_blob,
                record.audio_sha256,
                record.image_sha256,
            ],
        )
        .map_err(|e| AppError::Bundle(e.to_string()))?;
        tx.commit().map_err(|e| AppError::Bundle(e.to_string()))?;
        Ok(())
    }

    pub fn mark_card_failed(&self, job: &CardJob, source: &str, error: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO cards(id, cue_index, text, start_ms, end_ms, context, source, status, error)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'failed',?8)
                 ON CONFLICT(id) DO UPDATE SET status='failed', error=excluded.error",
                params![
                    job.card_id,
                    job.cue_index as i64,
                    job.text,
                    job.start_ms as i64,
                    job.end_ms as i64,
                    job.context,
                    source,
                    error,
                ],
            )
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        Ok(())
    }

    pub fn list_complete_cards(&self) -> Result<Vec<CardRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, cue_index, text, start_ms, end_ms, context, source,
                        audio_filename, image_filename, audio_blob, image_blob,
                        audio_sha256, image_sha256
                 FROM cards WHERE status = 'complete' ORDER BY cue_index",
            )
            .map_err(|e| AppError::Bundle(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(CardRecord {
                    id: row.get(0)?,
                    cue_index: row.get::<_, i64>(1)? as usize,
                    text: row.get(2)?,
                    start_ms: row.get::<_, i64>(3)? as u64,
                    end_ms: row.get::<_, i64>(4)? as u64,
                    context: row.get(5)?,
                    source: row.get(6)?,
                    audio_filename: row.get(7)?,
                    image_filename: row.get(8)?,
                    audio_blob: row.get(9)?,
                    image_blob: row.get(10)?,
                    audio_sha256: row.get(11)?,
                    image_sha256: row.get(12)?,
                })
            })
            .map_err(|e| AppError::Bundle(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| AppError::Bundle(e.to_string()))
    }

    pub fn init_build_meta(&self, config: &BuildConfig, source: &str) -> Result<()> {
        self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        self.set_meta("source", source)?;
        self.set_meta("deck_name", &config.deck_name)?;
        self.set_meta(
            "audio_format",
            &format!("{:?}", config.audio_format).to_lowercase(),
        )?;
        self.set_meta(
            "image_format",
            &format!("{:?}", config.image_format).to_lowercase(),
        )?;
        Ok(())
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

pub fn build_card_record(
    job: &CardJob,
    source: &str,
    audio_ext: &str,
    image_ext: &str,
    audio: Option<Vec<u8>>,
    image: Option<Vec<u8>>,
) -> CardRecord {
    let audio_filename = audio.as_ref().map(|_| {
        sanitize_filename(&media_filename(&job.card_id, job.start_ms, job.end_ms, audio_ext))
    });
    let image_filename = image.as_ref().map(|_| {
        sanitize_filename(&image_filename(&job.card_id, job.screenshot_ms, image_ext))
    });
    CardRecord {
        id: job.card_id.clone(),
        cue_index: job.cue_index,
        text: job.text.clone(),
        start_ms: job.start_ms,
        end_ms: job.end_ms,
        context: job.context.clone(),
        source: source.to_string(),
        audio_sha256: audio.as_ref().map(|b| sha256_hex(b)),
        image_sha256: image.as_ref().map(|b| sha256_hex(b)),
        audio_filename,
        image_filename,
        audio_blob: audio,
        image_blob: image,
    }
}

pub fn card_timestamps(job: &CardJob) -> String {
    format_timestamp_range(job.start_ms, job.end_ms)
}
