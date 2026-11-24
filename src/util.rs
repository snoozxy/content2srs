use std::path::Path;

use unicode_normalization::UnicodeNormalization;

const ANKI_ILLEGAL: &[char] = &['[', ']', '<', '>', ':', '"', '/', '?', '*', '^', '|', '\\'];

pub fn sanitize_filename(name: &str) -> String {
    let nfc: String = name.nfc().collect();
    let mut out: String = nfc
        .chars()
        .map(|c| if ANKI_ILLEGAL.contains(&c) { '_' } else { c })
        .collect();
    if out.len() > 120 {
        out.truncate(120);
    }
    out
}

pub fn ms_to_ffmpeg_time(ms: u64) -> String {
    let total_seconds = ms as f64 / 1000.0;
    format!("{total_seconds:.3}")
}

pub fn format_timestamp_range(start_ms: u64, end_ms: u64) -> String {
    format!("{}-{}", ms_to_hms(start_ms), ms_to_hms(end_ms))
}

fn ms_to_hms(ms: u64) -> String {
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    let millis = ms % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

/// Max length for the source stem portion of a media filename. Kept well under
/// the final 120-char cap in `sanitize_filename` so the per-card id/timestamp
/// suffix appended after this never gets truncated away, which would collide
/// distinct cards from the same episode onto the same filename. This is
/// purely cosmetic (readability) now — uniqueness across episodes is
/// guaranteed by `batch::episode_prefix` appending a sequential index, not by
/// anything in this truncation.
const SOURCE_STEM_MAX_LEN: usize = 60;

pub fn source_stem(source: &str) -> String {
    let mut stem = Path::new(source)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(sanitize_filename)
        .unwrap_or_else(|| "clip".into());
    if stem.len() > SOURCE_STEM_MAX_LEN {
        stem.truncate(SOURCE_STEM_MAX_LEN);
    }
    stem
}

/// Card id prefixed with the source video stem when not already present.
///
/// In batch mode, `card_id` already begins with the episode's globally
/// unique prefix (see `batch::episode_prefix`, assigned by sequential index)
/// — bare `card_id`s only look like `"card_00042"` in single-file mode where
/// no episode prefix has been assigned yet. Distinguishing on that shape
/// (rather than re-deriving `source_stem` here and string-matching it
/// against `card_id`) avoids a bug where this function's truncation could
/// disagree with the one that produced `card_id`, causing the stem to be
/// prepended a second time instead of recognized as already present.
fn media_id_part(source: &str, card_id: &str) -> String {
    if !card_id.starts_with("card_") {
        // Already carries a batch-assigned episode prefix.
        return card_id.to_string();
    }
    // Single-file mode: card_id is a bare "card_NNNNN" with no episode
    // context — prepend the source stem for a readable filename.
    format!("{}_{card_id}", source_stem(source))
}

pub fn media_filename(source: &str, card_id: &str, start_ms: u64, end_ms: u64, ext: &str) -> String {
    let id_part = media_id_part(source, card_id);
    sanitize_filename(&format!("{id_part}_{start_ms}-{end_ms}.{ext}"))
}

pub fn image_filename(source: &str, card_id: &str, timestamp_ms: u64, ext: &str) -> String {
    let id_part = media_id_part(source, card_id);
    sanitize_filename(&format!("{id_part}_{timestamp_ms}.{ext}"))
}

pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_anki_illegal_chars() {
        assert_eq!(sanitize_filename("bad[name].mp3"), "bad_name_.mp3");
    }

    #[test]
    fn media_filename_uses_source_stem() {
        assert_eq!(
            media_filename("show_01.mkv", "card_00042", 1000, 2000, "mp3"),
            "show_01_card_00042_1000-2000.mp3"
        );
    }

    #[test]
    fn media_filename_avoids_duplicate_batch_prefix() {
        assert_eq!(
            media_filename("ep01.mkv", "ep01_card_00042", 1000, 2000, "mp3"),
            "ep01_card_00042_1000-2000.mp3"
        );
    }
}
