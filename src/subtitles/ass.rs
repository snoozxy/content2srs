use crate::errors::{AppError, Result};
use crate::subtitles::Cue;

pub fn parse(text: &str) -> Result<Vec<Cue>> {
    let mut cues = Vec::new();
    for line in text.lines() {
        if !line.starts_with("Dialogue:") {
            continue;
        }
        let payload = line.trim_start_matches("Dialogue:").trim();
        let parts = split_ass_fields(payload);
        if parts.len() < 10 {
            continue;
        }
        let start_ms = parse_ass_time(&parts[1])?;
        let end_ms = parse_ass_time(&parts[2])?;
        let raw_text = parts[9..].join(",");
        let text = strip_ass_tags(&raw_text);
        if text.trim().is_empty() {
            continue;
        }
        cues.push(Cue {
            index: cues.len(),
            start_ms,
            end_ms,
            text,
            context: String::new(),
        });
    }

    if cues.is_empty() {
        return Err(AppError::SubtitleParse("no Dialogue lines found in ASS".into()));
    }
    Ok(cues)
}

fn split_ass_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_brace = false;
    for ch in line.chars() {
        match ch {
            '{' => {
                in_brace = true;
                current.push(ch);
            }
            '}' => {
                in_brace = false;
                current.push(ch);
            }
            ',' if !in_brace => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

fn parse_ass_time(ts: &str) -> Result<u64> {
    let pieces: Vec<&str> = ts.split(':').collect();
    if pieces.len() != 3 {
        return Err(AppError::SubtitleParse(format!("invalid ASS time: {ts}")));
    }
    let hours: u64 = pieces[0].parse().map_err(|_| AppError::SubtitleParse(ts.to_string()))?;
    let minutes: u64 = pieces[1].parse().map_err(|_| AppError::SubtitleParse(ts.to_string()))?;
    let seconds_parts: Vec<&str> = pieces[2].split('.').collect();
    let seconds: u64 = seconds_parts[0]
        .parse()
        .map_err(|_| AppError::SubtitleParse(ts.to_string()))?;
    let centis: u64 = seconds_parts
        .get(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    Ok(hours * 3_600_000 + minutes * 60_000 + seconds * 1000 + centis * 10)
}

fn strip_ass_tags(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            while let Some(next) = chars.next() {
                if next == '}' {
                    break;
                }
            }
            continue;
        }
        if ch == '\\' {
            if chars.peek() == Some(&'N') {
                chars.next();
                out.push('\n');
                continue;
            }
            if chars.peek() == Some(&'n') {
                chars.next();
                out.push('\n');
                continue;
            }
        }
        out.push(ch);
    }
    out.replace("\\h", " ")
}
