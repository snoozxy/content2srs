use regex::Regex;

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
}
