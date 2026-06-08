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

pub fn media_filename(card_id: &str, start_ms: u64, end_ms: u64, ext: &str) -> String {
    sanitize_filename(&format!("{card_id}_{start_ms}-{end_ms}.{ext}"))
}

pub fn image_filename(card_id: &str, timestamp_ms: u64, ext: &str) -> String {
    sanitize_filename(&format!("{card_id}_{timestamp_ms}.{ext}"))
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
}
