use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::errors::{AppError, Result};
use crate::util::sanitize_filename;

const VIDEO_EXTENSIONS: &[&str] = &["mkv", "mp4", "webm", "avi", "mov", "m4v", "ts"];
const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt"];

#[derive(Debug, Clone)]
pub struct Episode {
    /// Sanitized stem used as card-id prefix (batch mode).
    pub prefix: String,
    pub video: PathBuf,
    pub subtitle: Option<PathBuf>,
}

pub fn expand_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut expanded = Vec::new();
    for input in inputs {
        let pattern = input.to_string_lossy();
        if pattern.contains('*') || pattern.contains('?') {
            let entries = glob::glob(&pattern).map_err(|e| AppError::Config(e.to_string()))?;
            let mut matched = false;
            for entry in entries {
                let path = entry.map_err(|e| AppError::Config(e.to_string()))?;
                expanded.push(path);
                matched = true;
            }
            if !matched {
                return Err(AppError::Config(format!(
                    "glob matched no files: {pattern}"
                )));
            }
        } else if input.is_file() {
            expanded.push(input.clone());
        } else {
            return Err(AppError::Config(format!(
                "input not found: {}",
                input.display()
            )));
        }
    }
    expanded.sort();
    expanded.dedup();
    Ok(expanded)
}

pub fn discover_episodes(paths: &[PathBuf]) -> Result<Vec<Episode>> {
    let mut videos: HashMap<String, PathBuf> = HashMap::new();
    let mut subtitles: HashMap<String, PathBuf> = HashMap::new();

    for path in paths {
        if is_video(path) {
            let key = grouping_key(path);
            if videos.contains_key(&key) {
                return Err(AppError::Config(format!(
                    "duplicate video for episode '{key}': {}",
                    path.display()
                )));
            }
            videos.insert(key, path.clone());
        } else if is_subtitle(path) {
            let key = grouping_key(path);
            subtitles
                .entry(key.clone())
                .and_modify(|existing| {
                    tracing::warn!(
                        "multiple subtitles for '{key}', keeping {}",
                        existing.display()
                    );
                })
                .or_insert(path.clone());
        } else {
            return Err(AppError::Config(format!(
                "unsupported input type: {}",
                path.display()
            )));
        }
    }

    if videos.is_empty() {
        return Err(AppError::Config(
            "no video files found in inputs".into(),
        ));
    }

    let mut episodes: Vec<Episode> = videos
        .into_iter()
        .map(|(key, video)| Episode {
            prefix: episode_prefix(&key),
            subtitle: subtitles.remove(&key),
            video,
        })
        .collect();
    episodes.sort_by(|a, b| a.video.cmp(&b.video));
    Ok(episodes)
}

pub fn episode_prefix(stem: &str) -> String {
    sanitize_filename(stem)
}

fn grouping_key(path: &Path) -> String {
    let mut name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("episode")
        .to_string();

    loop {
        let p = Path::new(&name);
        let ext = match p.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_ascii_lowercase(),
            None => break,
        };
        if VIDEO_EXTENSIONS.contains(&ext.as_str())
            || SUBTITLE_EXTENSIONS.contains(&ext.as_str())
            || ext == "forced"
            || is_lang_tag(&ext)
        {
            name = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
        } else {
            break;
        }
    }
    name
}

fn is_lang_tag(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 5
        && s.chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-')
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn is_subtitle(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SUBTITLE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping_key_strips_lang_subtitle_suffix() {
        let path = Path::new("Show - 01.ja.srt");
        assert_eq!(grouping_key(path), "Show - 01");
    }

    #[test]
    fn grouping_key_strips_video_extension() {
        let path = Path::new("Show - 01.mkv");
        assert_eq!(grouping_key(path), "Show - 01");
    }

    #[test]
    fn discover_pairs_video_and_subtitle() {
        let episodes = discover_episodes(&[
            PathBuf::from("ep01.mkv"),
            PathBuf::from("ep01.srt"),
            PathBuf::from("ep02.mkv"),
            PathBuf::from("ep02.ass"),
        ])
        .unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].prefix, "ep01");
        assert_eq!(episodes[0].subtitle.as_deref(), Some(Path::new("ep01.srt")));
        assert_eq!(episodes[1].subtitle.as_deref(), Some(Path::new("ep02.ass")));
    }
}
