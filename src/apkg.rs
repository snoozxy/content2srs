use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use rusqlite::Connection;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::bundle::{Bundle, CardRecord};
use crate::errors::{AppError, Result};

/// Default deck config JSON matching genanki exports.
const GENANKI_DCONF: &str = r#"{
    "1": {
        "autoplay": true,
        "id": 1,
        "lapse": {
            "delays": [10],
            "leechAction": 0,
            "leechFails": 8,
            "minInt": 1,
            "mult": 0
        },
        "maxTaken": 60,
        "mod": 0,
        "name": "Default",
        "new": {
            "bury": true,
            "delays": [1, 10],
            "initialFactor": 2500,
            "ints": [1, 4, 7],
            "order": 1,
            "perDay": 20,
            "separate": true
        },
        "replayq": true,
        "rev": {
            "bury": true,
            "ease4": 1.3,
            "fuzz": 0.05,
            "ivlFct": 1,
            "maxIvl": 36500,
            "minSpace": 1,
            "perDay": 100
        },
        "timer": 0,
        "usn": 0
    }
}"#;

pub fn export_apkg(bundle: &Bundle, output: &Path, deck_name: &str) -> Result<()> {
    let cards = bundle.list_complete_cards()?;
    if cards.is_empty() {
        return Err(AppError::Apkg("bundle contains no complete cards".into()));
    }

    let tmp_dir = tempfile::tempdir().map_err(|e| AppError::Apkg(e.to_string()))?;
    let collection_path = tmp_dir.path().join("collection.anki2");
    build_collection_db(&collection_path, &cards, deck_name)?;

    // Modern Anki expects index -> filename (see anki/importing/apkg.py), not filename -> index.
    let mut seen_names: HashMap<String, usize> = HashMap::new();
    let mut media_files: Vec<(usize, String, Vec<u8>)> = Vec::new();
    let mut next_id = 0usize;

    for card in &cards {
        if let (Some(name), Some(blob)) = (&card.audio_filename, &card.audio_blob) {
            if !seen_names.contains_key(name) {
                seen_names.insert(name.clone(), next_id);
                media_files.push((next_id, name.clone(), blob.clone()));
                next_id += 1;
            }
        }
        if let (Some(name), Some(blob)) = (&card.image_filename, &card.image_blob) {
            if !seen_names.contains_key(name) {
                seen_names.insert(name.clone(), next_id);
                media_files.push((next_id, name.clone(), blob.clone()));
                next_id += 1;
            }
        }
    }

    let media_json_map: HashMap<String, String> = media_files
        .iter()
        .map(|(id, name, _)| (id.to_string(), name.clone()))
        .collect();
    let media_json =
        serde_json::to_string(&media_json_map).map_err(|e| AppError::Apkg(e.to_string()))?;

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

/// Deterministic IDs matching `apkg_export.py` / genanki.
fn deterministic_id(seed: &str) -> i64 {
    let digest = md5::compute(seed.as_bytes());
    let hex = format!("{digest:x}");
    i64::from_str_radix(&hex[..8], 16).unwrap_or(1)
}

fn deck_id(deck_name: &str) -> i64 {
    deterministic_id(deck_name)
}

fn model_id(deck_name: &str) -> i64 {
    deterministic_id(&format!("{deck_name}_model"))
}

fn note_guid(card_id: &str) -> String {
    let digest = md5::compute(card_id.as_bytes());
    let mut out = String::with_capacity(10);
    for byte in digest.0 {
        out.push(char::from(b'0' + (byte % 10)));
        if out.len() == 10 {
            break;
        }
    }
    out
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
    let did = deck_id(deck_name);
    let mid = model_id(deck_name);
    let mid_key = mid.to_string();

    // Match dojo-prompts/scripts/apkg_export.py (genanki Listening Card model).
    let models = serde_json::json!({
        &mid_key: {
            "id": &mid_key,
            "name": format!("{deck_name} Model"),
            "type": 0,
            "mod": now,
            "usn": -1,
            "sortf": 0,
            "did": did,
            "latexPre": "\\documentclass[12pt]{article}\n\\special{papersize=3in,5in}\n\\usepackage[utf8]{inputenc}\n\\usepackage{amssymb,amsmath}\n\\pagestyle{empty}\n\\setlength{\\parindent}{0in}\n\\begin{document}\n",
            "latexPost": "\\end{document}",
            "latexsvg": false,
            "tags": [],
            "vers": [],
            "tmpls": [{
                "name": "Listening Card",
                "ord": 0,
                "qfmt": "{{Audio}}",
                "afmt": "{{FrontSide}}<hr id=answer>{{Image}}<br>{{Text}}<br><br><small>{{Context}}</small>",
                "bqfmt": "",
                "bafmt": "",
                "bfont": "",
                "bsize": 0,
                "did": null
            }],
            "flds": [
                {"name": "Audio", "ord": 0, "sticky": false, "rtl": false, "font": "Liberation Sans", "size": 20, "media": []},
                {"name": "Image", "ord": 1, "sticky": false, "rtl": false, "font": "Liberation Sans", "size": 20, "media": []},
                {"name": "Text", "ord": 2, "sticky": false, "rtl": false, "font": "Liberation Sans", "size": 20, "media": []},
                {"name": "Context", "ord": 3, "sticky": false, "rtl": false, "font": "Liberation Sans", "size": 20, "media": []}
            ],
            "css": ".card { font-family: sans-serif; font-size: 24px; text-align: center; }",
            "req": [[0, "all", [0]]]
        }
    });

    let decks = serde_json::json!({
        "1": {
            "id": 1,
            "mod": now,
            "name": "Default",
            "usn": 0,
            "desc": "",
            "dyn": 0,
            "conf": 1,
            "collapsed": false,
            "extendNew": 10,
            "extendRev": 50,
            "newToday": [0, 0],
            "revToday": [0, 0],
            "lrnToday": [0, 0],
            "timeToday": [0, 0]
        },
        did.to_string(): {
            "id": did,
            "mod": now,
            "name": deck_name,
            "usn": -1,
            "desc": "",
            "dyn": 0,
            "conf": 1,
            "collapsed": false,
            "extendNew": 0,
            "extendRev": 50,
            "newToday": [0, 0],
            "revToday": [0, 0],
            "lrnToday": [0, 0],
            "timeToday": [0, 0]
        }
    });

    let conf = serde_json::json!({
        "activeDecks": [1],
        "addToCur": true,
        "collapseTime": 1200,
        "curDeck": 1,
        "curModel": mid.to_string(),
        "dueCounts": true,
        "estTimes": true,
        "newBury": true,
        "newSpread": 0,
        "nextPos": 1,
        "sortBackwards": false,
        "sortType": "noteFld",
        "timeLim": 0
    });

    conn.execute(
        "INSERT INTO col VALUES (1, ?1, ?1, ?1, 11, 0, 0, 0, ?2, ?3, ?4, ?5, '')",
        rusqlite::params![
            now,
            conf.to_string(),
            models.to_string(),
            decks.to_string(),
            GENANKI_DCONF,
        ],
    )
    .map_err(|e| AppError::Apkg(e.to_string()))?;

    let base_note_id = chrono::Utc::now().timestamp_millis();

    for (i, card) in cards.iter().enumerate() {
        let audio_filename = match (&card.audio_filename, &card.audio_blob) {
            (Some(name), Some(_)) => name.clone(),
            _ => continue,
        };

        let audio_field = format!("[sound:{audio_filename}]");
        let image_field = match (&card.image_filename, &card.image_blob) {
            (Some(name), Some(_)) => format!("<img src=\"{name}\">"),
            _ => String::new(),
        };

        let flds = [
            audio_field.as_str(),
            image_field.as_str(),
            card.text.as_str(),
            card.context.as_str(),
        ]
        .join("\x1f");

        let note_id = base_note_id + i as i64;
        let card_row_id = note_id + 1;
        let guid = note_guid(&card.id);

        conn.execute(
            "INSERT INTO notes VALUES (?1, ?2, ?3, ?4, -1, '', ?5, ?6, 0, 0, '')",
            rusqlite::params![note_id, guid, mid, now, flds, audio_field],
        )
        .map_err(|e| AppError::Apkg(e.to_string()))?;

        conn.execute(
            "INSERT INTO cards VALUES (?1, ?2, ?3, 0, ?4, 0, 0, 0, 0, 0, 2500, 0, 0, 0, 0, 0, 0, '')",
            rusqlite::params![card_row_id, note_id, did, now],
        )
        .map_err(|e| AppError::Apkg(e.to_string()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{Bundle, CardRecord};
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn card_record() -> CardRecord {
        CardRecord {
            id: "card_00000".into(),
            cue_index: 0,
            text: "こんにちは".into(),
            start_ms: 1000,
            end_ms: 2000,
            context: "prev\nnext".into(),
            source: "test.mkv".into(),
            audio_filename: Some("a.mp3".into()),
            image_filename: Some("b.jpg".into()),
            audio_blob: Some(vec![0x01, 0x02]),
            image_blob: Some(vec![0x03, 0x04]),
            audio_sha256: None,
            image_sha256: None,
        }
    }

    #[test]
    fn deterministic_ids_match_python() {
        assert_eq!(deck_id("test"), 160_394_189);
        assert_eq!(model_id("test"), 2_491_705_758);
    }

    #[test]
    fn exports_listening_card_note_fields() {
        let dir = tempdir().unwrap();
        let bundle_path = dir.path().join("test.db");
        let bundle = Bundle::open(&bundle_path, true).unwrap();
        bundle.commit_card(&card_record()).unwrap();

        let apkg_path = dir.path().join("deck.apkg");
        export_apkg(&bundle, &apkg_path, "test").unwrap();

        let file = std::fs::File::open(&apkg_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut db_bytes = Vec::new();
        std::io::copy(
            &mut archive.by_name("collection.anki2").unwrap(),
            &mut db_bytes,
        )
        .unwrap();

        let db_file = dir.path().join("col.anki2");
        std::fs::write(&db_file, &db_bytes).unwrap();
        let conn = Connection::open(&db_file).unwrap();

        let models: String = conn
            .query_row("SELECT models FROM col", [], |r| r.get(0))
            .unwrap();
        assert!(models.contains("Listening Card"));
        assert!(models.contains("{{Audio}}"));
        assert!(models.contains(r#"<hr id=answer>"#));

        let flds: String = conn
            .query_row("SELECT flds FROM notes", [], |r| r.get(0))
            .unwrap();
        assert!(flds.contains("[sound:a.mp3]"));
        assert!(flds.contains(r#"<img src="b.jpg">"#));
        assert!(flds.contains("こんにちは"));

        let due: i64 = conn
            .query_row("SELECT due FROM cards", [], |r| r.get(0))
            .unwrap();
        assert_eq!(due, 0);
    }

    #[test]
    fn media_map_uses_index_to_filename() {
        let dir = tempdir().unwrap();
        let bundle_path = dir.path().join("test.db");
        let bundle = Bundle::open(&bundle_path, true).unwrap();
        bundle.commit_card(&card_record()).unwrap();

        let apkg_path = dir.path().join("deck.apkg");
        export_apkg(&bundle, &apkg_path, "test").unwrap();

        let file = File::open(&apkg_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let media: HashMap<String, String> =
            serde_json::from_reader(archive.by_name("media").unwrap()).unwrap();
        assert_eq!(media.get("0"), Some(&"a.mp3".to_string()));
        assert_eq!(media.get("1"), Some(&"b.jpg".to_string()));
    }

    #[test]
    fn exports_apkg_zip_structure() {
        let dir = tempdir().unwrap();
        let bundle_path = dir.path().join("test.db");
        let bundle = Bundle::open(&bundle_path, true).unwrap();
        bundle.commit_card(&card_record()).unwrap();

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
