use crate::subtitles::Cue;

pub fn build_context(cues: &[Cue], index: usize, radius: usize) -> String {
    if radius == 0 || cues.is_empty() {
        return String::new();
    }
    let start = index.saturating_sub(radius);
    let end = (index + radius).min(cues.len() - 1);
    cues[start..=end]
        .iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
