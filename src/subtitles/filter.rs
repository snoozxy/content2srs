use regex::Regex;

/// Paired bracket/quote characters. `OPEN_BRACKETS[i]` closes with `CLOSE_BRACKETS[i]`.
/// Used to trim dangling quote marks left when a cue boundary (speaker change,
/// sentence end) cuts across a quotation — e.g. `つらかったよね「` or `」って。`.
const OPEN_BRACKETS: &[char] = &['「', '『', '（', '(', '【', '〈', '《', '〔', '｛', '{', '“', '‘'];
const CLOSE_BRACKETS: &[char] = &['」', '』', '）', ')', '】', '〉', '》', '〕', '｝', '}', '”', '’'];

fn matching_open(close: char) -> Option<char> {
    CLOSE_BRACKETS
        .iter()
        .position(|&c| c == close)
        .map(|i| OPEN_BRACKETS[i])
}

/// True if the text contains at least one "language" character — kana, kanji,
/// latin letters, or a digit. Pure punctuation/symbol cues (`？`, `。`, `…。`)
/// carry no learning value and are dropped before card creation.
pub fn has_linguistic_content(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// Strip *unmatched* quote brackets from a cue, wherever they sit.
///
/// When segmentation splits on a speaker change or sentence end mid-quotation, a cue
/// is left with a lone bracket: a dangling opener at the edge (`つらかったよね「`), a
/// dangling closer at the edge (`」って。`), or an interior closer whose opener was in
/// the previous card (`…いいんだよ」プハー。`). A simple stack pairs each closer with
/// the nearest matching opener; every bracket left unpaired is removed. Balanced
/// quotations (`彼は「やめろ」と言った`) are kept intact.
pub fn normalize_cue_text(text: &str) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    let mut matched = vec![false; chars.len()];
    let mut stack: Vec<usize> = Vec::new();

    for (i, &c) in chars.iter().enumerate() {
        if OPEN_BRACKETS.contains(&c) {
            stack.push(i);
        } else if let Some(open) = matching_open(c) {
            // Pair with the nearest open bracket of the same type still on the stack.
            if let Some(&top) = stack.last() {
                if chars[top] == open {
                    matched[top] = true;
                    matched[i] = true;
                    stack.pop();
                }
                // Mismatched nesting: leave this closer unmatched (it will be removed).
            }
        }
    }

    let cleaned: String = chars
        .iter()
        .enumerate()
        .filter(|&(i, &c)| {
            let is_bracket = OPEN_BRACKETS.contains(&c) || CLOSE_BRACKETS.contains(&c);
            !(is_bracket && !matched[i])
        })
        .map(|(_, &c)| c)
        .collect();

    cleaned.trim().to_string()
}

pub fn is_dialogue(text: &str, custom_exclude: Option<&Regex>) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    if let Some(re) = custom_exclude {
        if re.is_match(trimmed) {
            return false;
        }
    }

    if trimmed.contains('♪') || trimmed.contains('♫') {
        return false;
    }

    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return false;
    }

    if trimmed.starts_with('[') {
        if let Some(end) = trimmed.find(']') {
            let tag = &trimmed[1..end];
            if tag.len() <= 20 && !tag.contains(' ') {
                return false;
            }
        }
    }

    if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() < 80 {
        return false;
    }

    if trimmed.contains("{\\an") || trimmed.contains("{\\pos") {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_songs_and_signs() {
        assert!(!is_dialogue("♪ opening theme ♪", None));
        assert!(!is_dialogue("[Sign] Exit", None));
        assert!(is_dialogue("Hello, how are you?", None));
    }

    #[test]
    fn rejects_punctuation_only_cues() {
        assert!(!has_linguistic_content("？"));
        assert!(!has_linguistic_content("…。"));
        assert!(!has_linguistic_content("。"));
        assert!(!has_linguistic_content("「」"));
        assert!(has_linguistic_content("あ"));
        assert!(has_linguistic_content("えっ？"));
        assert!(has_linguistic_content("１年"));
    }

    #[test]
    fn trims_dangling_brackets() {
        // trailing dangling opener
        assert_eq!(normalize_cue_text("つらかったよね「"), "つらかったよね");
        // leading dangling closer
        assert_eq!(normalize_cue_text("」って。"), "って。");
        assert_eq!(normalize_cue_text("」みたいな空気が。"), "みたいな空気が。");
        // leading opener with no closer in the cue
        assert_eq!(normalize_cue_text("「あれ？"), "あれ？");
        assert_eq!(
            normalize_cue_text("「草介と連絡取れないけど一緒にいるの？"),
            "草介と連絡取れないけど一緒にいるの？"
        );
        // interior closer whose opener was in the previous card
        assert_eq!(
            normalize_cue_text("お兄ちゃんが頑張ってたのはわかるよ」プハー。"),
            "お兄ちゃんが頑張ってたのはわかるよプハー。"
        );
        assert_eq!(normalize_cue_text("君は無邪気な闇の女王』は外せません。"), "君は無邪気な闇の女王は外せません。");
        // balanced quotation is preserved
        assert_eq!(normalize_cue_text("「やめろ」"), "「やめろ」");
        assert_eq!(normalize_cue_text("彼は「やめろ」と言った"), "彼は「やめろ」と言った");
        assert_eq!(normalize_cue_text("『君は無邪気な闇の女王』"), "『君は無邪気な闇の女王』");
        // lone bracket collapses to empty (then dropped by content filter)
        assert_eq!(normalize_cue_text("「"), "");
    }
}
