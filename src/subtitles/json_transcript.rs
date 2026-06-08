//! ElevenLabs Scribe JSON transcript parsing (aligned with subs2cia `json_transcript.py`).
//! Japanese SRS mode uses UniDic via Lindera for bunsetsu-based sentence cues.

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;
use serde::Deserialize;

use crate::config::BuildConfig;
use crate::errors::{AppError, Result};
use crate::subtitles::Cue;

const MERGE_GAP_LIMIT: f64 = 0.4;
const ANKI_COMMA_TOKEN_LIMIT: usize = 5;
const SENTENCE_ENDERS: &str = "。！？!?";

const BUNSETSU_STARTERS: &[&str] = &[
    "名詞", "動詞", "形容詞", "形状詞", "副詞", "連体詞", "接続詞", "感動詞", "代名詞",
];

#[derive(Debug, Deserialize)]
struct Transcript {
    #[serde(default)]
    language_code: String,
    words: Vec<WordEntry>,
}

#[derive(Debug, Deserialize)]
struct WordEntry {
    text: String,
    start: f64,
    end: f64,
    #[serde(rename = "type")]
    token_type: String,
    #[serde(default)]
    speaker_id: String,
}

#[derive(Debug, Clone)]
struct CharToken {
    text: String,
    start: f64,
    end: f64,
    speaker: String,
}

#[derive(Debug, Clone)]
struct MorphInfo {
    surface: String,
    pos1: String,
}

#[derive(Debug, Clone)]
struct Bunsetsu {
    text: String,
    start: f64,
    end: f64,
    speaker: String,
    morph_count: usize,
}

struct JapaneseTagger {
    tokenizer: Tokenizer,
}

impl JapaneseTagger {
    fn new() -> Result<Self> {
        let dictionary = load_dictionary("embedded://unidic").map_err(|e| {
            AppError::SubtitleParse(format!("failed to load UniDic dictionary: {e}"))
        })?;
        let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
        Ok(Self {
            tokenizer: Tokenizer::new(segmenter),
        })
    }

    fn analyze(&mut self, text: &str) -> Result<Vec<MorphInfo>> {
        let mut tokens = self
            .tokenizer
            .tokenize(text)
            .map_err(|e| AppError::SubtitleParse(format!("morphological analysis failed: {e}")))?;

        Ok(tokens
            .iter_mut()
            .map(|token| {
                let surface = token.surface.as_ref().to_string();
                let details = token.details();
                MorphInfo {
                    surface,
                    pos1: details.first().map(|s| s.to_string()).unwrap_or_default(),
                }
            })
            .collect())
    }
}

pub fn parse(config: &BuildConfig, bytes: &[u8]) -> Result<Vec<Cue>> {
    let data: Transcript = serde_json::from_slice(bytes).map_err(|e| {
        AppError::SubtitleParse(format!("invalid JSON transcript: {e}"))
    })?;

    if data.words.is_empty() {
        return Err(AppError::SubtitleParse(
            "JSON transcript contains no word entries".into(),
        ));
    }

    let lang = normalize_lang(&data.language_code, config.lang.as_deref());
    let raw = if is_japanese(&lang) {
        parse_japanese_srs(&data)?
    } else {
        parse_gap_based(&data, config.merge_threshold_ms)?
    };

    Ok(raw
        .into_iter()
        .enumerate()
        .map(|(index, (start_ms, end_ms, text))| Cue {
            index,
            start_ms,
            end_ms,
            text,
            context: String::new(),
        })
        .collect())
}

fn normalize_lang(json_lang: &str, config_lang: Option<&str>) -> String {
    if !json_lang.is_empty() {
        return json_lang.to_ascii_lowercase();
    }
    config_lang
        .map(|l| l.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_japanese(lang: &str) -> bool {
    matches!(lang, "jpn" | "ja" | "japanese")
}

fn parse_japanese_srs(data: &Transcript) -> Result<Vec<(u64, u64, String)>> {
    let mut tagger = JapaneseTagger::new()?;
    let chars = load_chars(data);
    if chars.is_empty() {
        return Err(AppError::SubtitleParse(
            "JSON transcript contains no word tokens".into(),
        ));
    }

    let mut all_bunsetsu = Vec::new();
    for run in split_by_speaker(&chars) {
        for sentence in split_by_sentence(&run) {
            all_bunsetsu.extend(chars_to_bunsetsu(&sentence, &mut tagger)?);
        }
    }

    Ok(bunsetsu_to_anki_cues(&all_bunsetsu))
}

fn parse_gap_based(data: &Transcript, threshold_ms: u64) -> Result<Vec<(u64, u64, String)>> {
    let threshold_s = threshold_ms as f64 / 1000.0;
    let words: Vec<&WordEntry> = data
        .words
        .iter()
        .filter(|w| w.token_type == "word")
        .collect();

    if words.is_empty() {
        return Err(AppError::SubtitleParse(
            "JSON transcript contains no word tokens".into(),
        ));
    }

    let mut segments: Vec<(f64, f64, String)> = Vec::new();
    let mut seg_start = words[0].start;
    let mut seg_end = words[0].end;
    let mut seg_text = words[0].text.clone();

    for word in words.iter().skip(1) {
        let gap = word.start - seg_end;
        if gap > threshold_s {
            segments.push((seg_start, seg_end, seg_text));
            seg_start = word.start;
            seg_text = word.text.clone();
        } else {
            seg_text.push_str(&word.text);
        }
        seg_end = word.end;
    }
    segments.push((seg_start, seg_end, seg_text));

    Ok(segments
        .into_iter()
        .map(|(start, end, text)| {
            (
                (start * 1000.0).round() as u64,
                (end * 1000.0).round() as u64,
                text,
            )
        })
        .collect())
}

fn load_chars(data: &Transcript) -> Vec<CharToken> {
    let mut chars = Vec::new();
    for word in &data.words {
        if word.token_type != "word" {
            continue;
        }

        let start = word.start;
        let mut end = word.end;
        if word.text.chars().count() == 1 && ends_sentence(&word.text) && end - start > 0.2 {
            end = start + 0.1;
        }

        let speaker = if word.speaker_id.is_empty() {
            "speaker_0".to_string()
        } else {
            word.speaker_id.clone()
        };

        let graphemes: Vec<char> = word.text.chars().collect();
        if graphemes.len() <= 1 {
            chars.push(CharToken {
                text: word.text.clone(),
                start,
                end,
                speaker,
            });
        } else {
            let n = graphemes.len() as f64;
            let duration = end - start;
            for (i, ch) in graphemes.into_iter().enumerate() {
                let ch_start = start + (duration * i as f64 / n);
                let ch_end = start + (duration * (i + 1) as f64 / n);
                chars.push(CharToken {
                    text: ch.to_string(),
                    start: ch_start,
                    end: ch_end,
                    speaker: speaker.clone(),
                });
            }
        }
    }
    chars
}

fn split_by_speaker(chars: &[CharToken]) -> Vec<Vec<CharToken>> {
    if chars.is_empty() {
        return Vec::new();
    }

    let mut runs = Vec::new();
    let mut current = vec![chars[0].clone()];
    for ch in chars.iter().skip(1) {
        if ch.speaker != current.last().map(|c| c.speaker.as_str()).unwrap_or("") {
            runs.push(current);
            current = vec![ch.clone()];
        } else {
            current.push(ch.clone());
        }
    }
    runs.push(current);
    runs
}

fn split_by_sentence(chars: &[CharToken]) -> Vec<Vec<CharToken>> {
    if chars.is_empty() {
        return Vec::new();
    }

    let mut sentences = Vec::new();
    let mut current = Vec::new();
    for ch in chars {
        current.push(ch.clone());
        if ends_sentence(&ch.text) {
            sentences.push(current);
            current = Vec::new();
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }
    sentences
}

fn chars_to_bunsetsu(chars: &[CharToken], tagger: &mut JapaneseTagger) -> Result<Vec<Bunsetsu>> {
    if chars.is_empty() {
        return Ok(Vec::new());
    }

    let text: String = chars.iter().map(|c| c.text.as_str()).collect();
    let speaker = chars[0].speaker.clone();
    let morphemes = tagger.analyze(&text)?;

    let mut char_offset = 0usize;
    let mut bunsetsu_list = Vec::new();
    let mut current_chars: Vec<CharToken> = Vec::new();
    let mut current_morphs: Vec<MorphInfo> = Vec::new();
    let mut prefix_active = false;

    let mut flush = |chars: &mut Vec<CharToken>, morphs: &mut Vec<MorphInfo>| {
        if chars.is_empty() {
            return;
        }
        bunsetsu_list.push(make_bunsetsu(chars, morphs, &speaker));
        chars.clear();
        morphs.clear();
    };

    for morph in morphemes {
        let morph_len = morph.surface.chars().count();
        let morph_chars: Vec<CharToken> = chars[char_offset..char_offset + morph_len]
            .iter()
            .cloned()
            .collect();
        char_offset += morph_len;

        let is_starter = BUNSETSU_STARTERS.contains(&morph.pos1.as_str());
        let is_prefix = morph.pos1 == "接頭辞";

        if is_prefix {
            flush(&mut current_chars, &mut current_morphs);
            current_chars = morph_chars;
            current_morphs = vec![morph];
            prefix_active = true;
        } else if is_starter {
            if prefix_active {
                current_chars.extend(morph_chars);
                current_morphs.push(morph);
                prefix_active = false;
            } else {
                flush(&mut current_chars, &mut current_morphs);
                current_chars = morph_chars;
                current_morphs = vec![morph];
            }
        } else if current_chars.is_empty() {
            current_chars = morph_chars;
            current_morphs = vec![morph];
        } else {
            current_chars.extend(morph_chars);
            current_morphs.push(morph);
            prefix_active = false;
        }
    }

    flush(&mut current_chars, &mut current_morphs);
    Ok(bunsetsu_list)
}

fn make_bunsetsu(chars: &[CharToken], morphs: &[MorphInfo], speaker: &str) -> Bunsetsu {
    let text: String = chars.iter().map(|c| c.text.as_str()).collect();
    Bunsetsu {
        text,
        start: chars[0].start,
        end: chars[chars.len() - 1].end,
        speaker: speaker.to_string(),
        morph_count: morphs.len(),
    }
}

fn bunsetsu_to_anki_cues(bunsetsu_list: &[Bunsetsu]) -> Vec<(u64, u64, String)> {
    if bunsetsu_list.is_empty() {
        return Vec::new();
    }

    let mut cues = Vec::new();
    let mut current: Vec<Bunsetsu> = Vec::new();

    let mut flush = |current: &mut Vec<Bunsetsu>| {
        if current.is_empty() {
            return;
        }
        let text: String = current.iter().map(|b| b.text.as_str()).collect();
        let start_ms = (current[0].start * 1000.0).round() as u64;
        let end_ms = (current.last().unwrap().end * 1000.0).round() as u64;
        cues.push((start_ms, end_ms, text));
        current.clear();
    };

    for (i, b) in bunsetsu_list.iter().enumerate() {
        if let Some(last) = current.last() {
            if b.speaker != last.speaker {
                flush(&mut current);
            } else if b.start - last.end >= MERGE_GAP_LIMIT {
                flush(&mut current);
            }
        }

        current.push(b.clone());

        if ends_with_sentence_ender(&b.text) {
            flush(&mut current);
        } else if b.text.ends_with('、') {
            let token_count: usize = current.iter().map(|bu| bu.morph_count).sum();
            if token_count >= ANKI_COMMA_TOKEN_LIMIT {
                let next_tokens = next_section_token_count(bunsetsu_list, i);
                if !(next_tokens <= 2 && token_count <= 7) {
                    flush(&mut current);
                }
            }
        }
    }

    flush(&mut current);
    cues
}

fn next_section_token_count(bunsetsu_list: &[Bunsetsu], idx: usize) -> usize {
    let speaker = &bunsetsu_list[idx].speaker;
    let mut count = 0usize;
    for b in bunsetsu_list.iter().skip(idx + 1) {
        if b.speaker != *speaker {
            break;
        }
        count += b.morph_count;
        if let Some(last) = b.text.chars().last() {
            if SENTENCE_ENDERS.contains(last) || last == '、' {
                break;
            }
        }
    }
    count
}

fn ends_sentence(text: &str) -> bool {
    text.chars()
        .last()
        .is_some_and(|c| SENTENCE_ENDERS.contains(c))
}

fn ends_with_sentence_ender(text: &str) -> bool {
    text.chars()
        .last()
        .is_some_and(|c| SENTENCE_ENDERS.contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
        "language_code": "jpn",
        "words": [
            {"text": "こん", "start": 1.0, "end": 1.2, "type": "word", "speaker_id": "speaker_0"},
            {"text": "ぴこ", "start": 1.2, "end": 1.5, "type": "word", "speaker_id": "speaker_0"},
            {"text": "。", "start": 1.5, "end": 1.6, "type": "word", "speaker_id": "speaker_0"},
            {"text": "どう", "start": 2.0, "end": 2.2, "type": "word", "speaker_id": "speaker_0"},
            {"text": "も", "start": 2.2, "end": 2.4, "type": "word", "speaker_id": "speaker_0"},
            {"text": "。", "start": 2.4, "end": 2.5, "type": "word", "speaker_id": "speaker_0"}
        ]
    }"#;

    fn test_config() -> BuildConfig {
        BuildConfig {
            inputs: vec![],
            batch: false,
            jobs: None,
            subtitle: None,
            output: None,
            apkg: None,
            audio_index: None,
            subtitle_index: None,
            video_index: None,
            lang: Some("jpn".into()),
            padding_ms: 0,
            merge_threshold_ms: 500,
            context_lines: 0,
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
            summaries: None,
        }
    }

    #[test]
    fn japanese_json_produces_sentence_cues() {
        let cues = parse(&test_config(), SAMPLE_JSON.as_bytes()).unwrap();
        assert_eq!(cues.len(), 2);
        assert!(cues[0].text.contains("こん"));
        assert!(cues[0].text.contains('。'));
        assert_eq!(cues[0].start_ms, 1000);
        assert!(cues[1].text.contains("どう"));
    }

    #[test]
    fn gap_based_for_non_japanese() {
        let json = r#"{
            "language_code": "eng",
            "words": [
                {"text": "Hello", "start": 0.0, "end": 0.5, "type": "word"},
                {"text": "world", "start": 0.5, "end": 1.0, "type": "word"},
                {"text": "!", "start": 1.0, "end": 1.1, "type": "word"},
                {"text": "Next", "start": 2.0, "end": 2.5, "type": "word"},
                {"text": "line", "start": 2.5, "end": 3.0, "type": "word"}
            ]
        }"#;
        let cues = parse(&test_config(), json.as_bytes()).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Helloworld!");
        assert_eq!(cues[1].text, "Nextline");
    }
}
