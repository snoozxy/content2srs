use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use rusqlite::Connection;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::bundle::{Bundle, CardRecord};
use crate::errors::{AppError, Result};

const MODEL_ID: i64 = 1_601_739_408_287_017_736;
const DECK_ID: i64 = 1;
const NOTE_TYPE_ID: i64 = 1;

pub fn export_apkg(bundle: &Bundle, output: &Path, deck_name: &str) -> Result<()> {
    let cards = bundle.list_complete_cards()?;
    if cards.is_empty() {
        return Err(AppError::Apkg("bundle contains no complete cards".into()));
    }

    let tmp_dir = tempfile::tempdir().map_err(|e| AppError::Apkg(e.to_string()))?;
    let collection_path = tmp_dir.path().join("collection.anki2");
    build_collection_db(&collection_path, &cards, deck_name)?;

    let mut media_map: HashMap<String, usize> = HashMap::new();
    let mut media_files: Vec<(usize, String, Vec<u8>)> = Vec::new();
    let mut next_id = 0usize;

    for card in &cards {
        if let (Some(name), Some(blob)) = (&card.audio_filename, &card.audio_blob) {
            if !media_map.contains_key(name) {
                media_map.insert(name.clone(), next_id);
                media_files.push((next_id, name.clone(), blob.clone()));
                next_id += 1;
            }
        }
        if let (Some(name), Some(blob)) = (&card.image_filename, &card.image_blob) {
            if !media_map.contains_key(name) {
                media_map.insert(name.clone(), next_id);
                media_files.push((next_id, name.clone(), blob.clone()));
                next_id += 1;
            }
        }
    }

    let media_json = serde_json::to_string(&media_map).map_err(|e| AppError::Apkg(e.to_string()))?;

    let staging = output.with_extension("apkg.staging");
    {
        let file = File::create(&staging).map_err(|e| AppError::Apkg(e.to_string()))?;
        let mut zip = ZipWriter::new(file);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        zip.start_file("collection.anki2", stored)
            .map_err(|e| AppError::Apkg(e.to_string()))?;
        let db_bytes =
            std::fs::read(&collection_path).map_err(|e| AppError::Apkg(e.to_string()))?;
        zip.write_all(&db_bytes)
            .map_err(|e| AppError::Apkg(e.to_string()))?;

        zip.start_file("media", stored)
            .map_err(|e| AppError::Apkg(e.to_string()))?;
        zip.write_all(media_json.as_bytes())
            .map_err(|e| AppError::Apkg(e.to_string()))?;

        for (id, _name, blob) in &media_files {
            let entry = id.to_string();
            zip.start_file(&entry, stored)
                .map_err(|e| AppError::Apkg(e.to_string()))?;
            zip.write_all(blob)
                .map_err(|e| AppError::Apkg(e.to_string()))?;
        }

        zip.finish().map_err(|e| AppError::Apkg(e.to_string()))?;
    }

    if output.exists() {
        std::fs::remove_file(output).map_err(|e| AppError::Apkg(e.to_string()))?;
    }
    std::fs::rename(&staging, output).map_err(|e| AppError::Apkg(e.to_string()))?;
    Ok(())
}

fn build_collection_db(path: &Path, cards: &[CardRecord], deck_name: &str) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path).ok();
    }
    let conn = Connection::open(path).map_err(|e| AppError::Apkg(e.to_string()))?;
    conn.execute_batch(
        r#"
        CREATE TABLE col (
            id INTEGER PRIMARY KEY,
            crt INTEGER NOT NULL,
            mod INTEGER NOT NULL,
            scm INTEGER NOT NULL,
            ver INTEGER NOT NULL,
            dty INTEGER NOT NULL,
            usn INTEGER NOT NULL,
            ls INTEGER NOT NULL,
            conf TEXT NOT NULL,
            models TEXT NOT NULL,
            decks TEXT NOT NULL,
            dconf TEXT NOT NULL,
            tags TEXT NOT NULL
        );
        CREATE TABLE notes (
            id INTEGER PRIMARY KEY,
            guid TEXT NOT NULL,
            mid INTEGER NOT NULL,
            mod INTEGER NOT NULL,
            usn INTEGER NOT NULL,
            tags TEXT NOT NULL,
            flds TEXT NOT NULL,
            sfld TEXT NOT NULL,
            csum INTEGER NOT NULL,
            flags INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE cards (
            id INTEGER PRIMARY KEY,
            nid INTEGER NOT NULL,
            did INTEGER NOT NULL,
            ord INTEGER NOT NULL,
            mod INTEGER NOT NULL,
            usn INTEGER NOT NULL,
            type INTEGER NOT NULL,
            queue INTEGER NOT NULL,
            due INTEGER NOT NULL,
            ivl INTEGER NOT NULL,
            factor INTEGER NOT NULL,
            reps INTEGER NOT NULL,
            lapses INTEGER NOT NULL,
            left INTEGER NOT NULL,
            odue INTEGER NOT NULL,
            odid INTEGER NOT NULL,
            flags INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE graves (usn INTEGER, oid INTEGER, type INTEGER);
        CREATE TABLE revlog (
            id INTEGER PRIMARY KEY,
            cid INTEGER NOT NULL,
            usn INTEGER NOT NULL,
            ease INTEGER NOT NULL,
            ivl INTEGER NOT NULL,
            lastIvl INTEGER NOT NULL,
            factor INTEGER NOT NULL,
            time INTEGER NOT NULL,
            type INTEGER NOT NULL
        );
        "#,
    )
    .map_err(|e| AppError::Apkg(e.to_string()))?;

    let now = chrono::Utc::now().timestamp();
    let models = serde_json::json!({
        MODEL_ID.to_string(): {
            "id": MODEL_ID,
            "name": "content2srs",
            "type": 0,
            "mod": now,
            "usn": 0,
            "sortf": 0,
            "did": DECK_ID,
            "tmpls": [{
                "name": "Card 1",
                "ord": 0,
                "qfmt": "{{Expression}}<br>{{#Image}}<img src=\"{{Image}}\">{{/Image}}<br>{{Audio}}",
                "afmt": "{{Context}}<hr>{{Timestamps}}<br><small>{{Source}}</small>"
            }],
            "flds": [
                {"name": "Expression", "ord": 0, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": []},
                {"name": "Audio", "ord": 1, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": ["audio"]},
                {"name": "Image", "ord": 2, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": ["image"]},
                {"name": "Context", "ord": 3, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": []},
                {"name": "Timestamps", "ord": 4, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": []},
                {"name": "Source", "ord": 5, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": []}
            ],
            "css": ".card { font-family: arial; font-size: 20px; text-align: center; }",
            "latexPre": "",
            "latexPost": "",
            "latexsvg": false,
            "req": [[0, "any", [0,1,2]]]
        }
    });
    let decks = serde_json::json!({
        "1": {
            "id": 1,
            "mod": now,
            "name": deck_name,
            "usn": 0,
            "desc": "Generated by content2srs",
            "dyn": 0,
            "conf": 1,
            "collapsed": false,
            "browserCollapsed": false,
            "extendNew": 0,
            "extendRev": 0,
            "newToday": [0, 0],
            "revToday": [0, 0],
            "lrnToday": [0, 0],
            "timeToday": [0, 0]
        }
    });

    conn.execute(
        "INSERT INTO col VALUES (1, ?1, ?1, ?1, 11, 0, 0, 0, '{}', ?2, ?3, '{}', '')",
        rusqlite::params![now, models.to_string(), decks.to_string()],
    )
    .map_err(|e| AppError::Apkg(e.to_string()))?;

    for (i, card) in cards.iter().enumerate() {
        let note_id = (i as i64) + 1;
        let card_id = note_id;
        let guid = format!("c2s-{:08x}", note_id);

        let audio_field = card
            .audio_filename
            .as_ref()
            .map(|f| format!("[sound:{f}]"))
            .unwrap_or_default();
        let image_field = card.image_filename.clone().unwrap_or_default();
        let timestamps = format!("{}-{}", card.start_ms, card.end_ms);

        let flds = vec![
            card.text.clone(),
            audio_field,
            image_field,
            card.context.clone(),
            timestamps,
            card.source.clone(),
        ]
        .join("\x1f");

        conn.execute(
            "INSERT INTO notes VALUES (?1, ?2, ?3, ?4, 0, '', ?5, ?6, 0, 0, '')",
            rusqlite::params![note_id, guid, NOTE_TYPE_ID, now, flds, card.text],
        )
        .map_err(|e| AppError::Apkg(e.to_string()))?;

        conn.execute(
            "INSERT INTO cards VALUES (?1, ?2, ?3, 0, ?4, 0, 0, 0, ?2, 0, 2500, 0, 0, 0, 0, 0, 0, '')",
            rusqlite::params![card_id, note_id, DECK_ID, now],
        )
        .map_err(|e| AppError::Apkg(e.to_string()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{Bundle, CardRecord};
    use tempfile::tempdir;

    #[test]
    fn exports_apkg_zip_structure() {
        let dir = tempdir().unwrap();
        let bundle_path = dir.path().join("test.db");
        let bundle = Bundle::open(&bundle_path, true).unwrap();
        bundle
            .commit_card(&CardRecord {
                id: "card_00000".into(),
                cue_index: 0,
                text: "hello".into(),
                start_ms: 1000,
                end_ms: 2000,
                context: "ctx".into(),
                source: "test.mkv".into(),
                audio_filename: Some("a.mp3".into()),
                image_filename: Some("b.jpg".into()),
                audio_blob: Some(vec![0x01, 0x02]),
                image_blob: Some(vec![0x03, 0x04]),
                audio_sha256: None,
                image_sha256: None,
            })
            .unwrap();

        let apkg_path = dir.path().join("deck.apkg");
        export_apkg(&bundle, &apkg_path, "test").unwrap();
        assert!(apkg_path.exists());

        let file = File::open(&apkg_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert!(archive.by_name("collection.anki2").is_ok());
        assert!(archive.by_name("media").is_ok());
        assert!(archive.by_name("0").is_ok());
    }
}
