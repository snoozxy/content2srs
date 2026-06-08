use crate::errors::{AppError, Result};
use crate::subtitles::Cue;

pub fn parse(text: &str) -> Result<Vec<Cue>> {
    let normalized = text.replace("\r\n", "\n");
    let blocks: Vec<&str> = normalized.split("\n\n").collect();
    let mut cues = Vec::new();

    for block in blocks {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.len() < 2 {
            continue;
        }

        let time_line = if lines[0].contains("-->") {
            lines[0]
        } else if lines.len() >= 3 && lines[1].contains("-->") {
            lines[1]
        } else {
            continue;
        };

        let (start_ms, end_ms) = parse_timestamp_line(time_line)?;
        let text_start = if lines[0].contains("-->") { 1 } else { 2 };
        let cue_text: String = lines[text_start..]
            .join("\n")
            .replace("<i>", "")
            .replace("</i>", "")
            .replace("<b>", "")
            .replace("</b>", "")
            .trim()
            .to_string();

        if cue_text.is_empty() {
            continue;
        }

        cues.push(Cue {
            index: cues.len(),
            start_ms,
            end_ms,
            text: cue_text,
            context: String::new(),
        });
    }

    if cues.is_empty() {
        return Err(AppError::SubtitleParse("no cues found in SRT".into()));
    }
    Ok(cues)
}

fn parse_timestamp_line(line: &str) -> Result<(u64, u64)> {
    let parts: Vec<&str> = line.split("-->").collect();
    if parts.len() != 2 {
        return Err(AppError::SubtitleParse(format!("invalid timestamp: {line}")));
    }
    let start = parse_timestamp(parts[0].trim())?;
    let end = parse_timestamp(parts[1].trim().split_whitespace().next().unwrap_or(""))?;
    Ok((start, end))
}

fn parse_timestamp(ts: &str) -> Result<u64> {
    let ts = ts.replace(',', ".");
    let pieces: Vec<&str> = ts.split(':').collect();
    if pieces.len() != 3 {
        return Err(AppError::SubtitleParse(format!("invalid timestamp: {ts}")));
    }
    let hours: u64 = pieces[0].parse().map_err(|_| AppError::SubtitleParse(ts.to_string()))?;
    let minutes: u64 = pieces[1].parse().map_err(|_| AppError::SubtitleParse(ts.to_string()))?;
    let seconds: f64 = pieces[2].parse().map_err(|_| AppError::SubtitleParse(ts.to_string()))?;
    Ok(hours * 3_600_000 + minutes * 60_000 + (seconds * 1000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_timestamp("00:01:02,500").unwrap(), 62_500);
    }
}
