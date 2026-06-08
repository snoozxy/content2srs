mod ass;
mod context;
mod filter;
mod srt;

pub use context::build_context;
use std::path::Path;

use crate::config::BuildConfig;
use crate::errors::{AppError, Result};
use crate::probe::SubtitleSource;

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub context: String,
}

pub fn load_and_prepare(config: &BuildConfig, source: &SubtitleSource) -> Result<Vec<Cue>> {
    let (bytes, ext_hint) = match source {
        SubtitleSource::External { path, bytes } => {
            (bytes.clone(), path.extension().and_then(|e| e.to_str()).map(String::from))
        }
        SubtitleSource::Embedded { .. } => {
            return Err(AppError::SubtitleParse(
                "embedded subtitles must be demuxed before parsing".into(),
            ));
        }
    };

    let text = decode_text(&bytes, ext_hint.as_deref())?;
    let mut cues = if looks_like_ass(&text) {
        ass::parse(&text)?
    } else {
        srt::parse(&text)?
    };

    if !config.no_filter {
        let regex = config
            .regex_filter
            .as_ref()
            .map(|r| regex::Regex::new(r))
            .transpose()
            .map_err(|e| AppError::SubtitleParse(format!("invalid regex: {e}")))?;
        cues.retain(|c| filter::is_dialogue(&c.text, regex.as_ref()));
    }

    apply_padding(&mut cues, config.padding_ms);

    if config.merge_threshold_ms > 0 {
        cues = merge_cues(cues, config.merge_threshold_ms);
    }

    assign_indices_and_context(&mut cues, config.context_lines);
    Ok(cues)
}

pub fn load_from_bytes(
    config: &BuildConfig,
    bytes: &[u8],
    ext_hint: Option<&str>,
) -> Result<Vec<Cue>> {
    let text = decode_text(bytes, ext_hint)?;
    let mut cues = if looks_like_ass(&text) {
        ass::parse(&text)?
    } else {
        srt::parse(&text)?
    };

    if !config.no_filter {
        let regex = config
            .regex_filter
            .as_ref()
            .map(|r| regex::Regex::new(r))
            .transpose()
            .map_err(|e| AppError::SubtitleParse(format!("invalid regex: {e}")))?;
        cues.retain(|c| filter::is_dialogue(&c.text, regex.as_ref()));
    }

    apply_padding(&mut cues, config.padding_ms);
    if config.merge_threshold_ms > 0 {
        cues = merge_cues(cues, config.merge_threshold_ms);
    }
    assign_indices_and_context(&mut cues, config.context_lines);
    Ok(cues)
}

fn assign_indices_and_context(cues: &mut [Cue], context_lines: usize) {
    for (i, cue) in cues.iter_mut().enumerate() {
        cue.index = i;
    }
    let contexts: Vec<String> = (0..cues.len())
        .map(|i| build_context(cues, i, context_lines))
        .collect();
    for (cue, context) in cues.iter_mut().zip(contexts) {
        cue.context = context;
    }
}

pub fn subtitle_filename(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "subtitles".into())
}

fn decode_text(bytes: &[u8], ext_hint: Option<&str>) -> Result<String> {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(s.to_string());
    }

    let encodings: &[&encoding_rs::Encoding] = &[
        encoding_rs::SHIFT_JIS,
        encoding_rs::EUC_JP,
        encoding_rs::ISO_2022_JP,
        encoding_rs::WINDOWS_1252,
        encoding_rs::GB18030,
    ];

    for enc in encodings {
        let (cow, _, had_errors) = enc.decode(bytes);
        if !had_errors {
            return Ok(cow.into_owned());
        }
    }

    let (cow, _, _) = encoding_rs::UTF_8.decode(bytes);
    if ext_hint.is_some() {
        Ok(cow.into_owned())
    } else {
        Err(AppError::SubtitleParse(
            "could not decode subtitle file as UTF-8 or common legacy encodings".into(),
        ))
    }
}

fn looks_like_ass(text: &str) -> bool {
    text.lines()
        .take(20)
        .any(|l| l.trim_start().starts_with("[Script Info]") || l.starts_with("Dialogue:"))
}

fn apply_padding(cues: &mut [Cue], padding_ms: u64) {
    for cue in cues.iter_mut() {
        cue.start_ms = cue.start_ms.saturating_sub(padding_ms);
        cue.end_ms = cue.end_ms.saturating_add(padding_ms);
    }
}

fn merge_cues(mut cues: Vec<Cue>, threshold_ms: u64) -> Vec<Cue> {
    if cues.is_empty() {
        return cues;
    }
    let mut merged = Vec::new();
    let mut current = cues.remove(0);
    for next in cues {
        if next.start_ms <= current.end_ms + threshold_ms {
            current.end_ms = current.end_ms.max(next.end_ms);
            if !current.text.contains(&next.text) {
                current.text.push('\n');
                current.text.push_str(&next.text);
            }
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_srt() {
        let srt_data = b"1\n00:00:01,000 --> 00:00:03,000\nHello world\n\n";
        let config = BuildConfig {
            inputs: vec![],
            subtitle: None,
            output: None,
            apkg: None,
            audio_index: None,
            subtitle_index: None,
            video_index: None,
            lang: None,
            padding_ms: 0,
            merge_threshold_ms: 0,
            context_lines: 1,
            regex_filter: None,
            no_filter: true,
            audio_format: crate::config::AudioFormat::Mp3,
            audio_bitrate_kbps: 128,
            loudnorm: false,
            image_format: crate::config::ImageFormat::Jpg,
            screenshot_offset_ms: 0,
            exact_frame: false,
            hwaccel: "auto".into(),
            workers: 4,
            resume: true,
            dry_run: false,
            no_audio: false,
            no_screenshot: false,
            deck_name: "test".into(),
        };
        let cues = load_from_bytes(&config, srt_data, Some("srt")).unwrap();
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "Hello world");
        assert_eq!(cues[0].start_ms, 1000);
        assert_eq!(cues[0].end_ms, 3000);
    }
}
