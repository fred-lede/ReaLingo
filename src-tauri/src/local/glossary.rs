use crate::config::GlossaryEntry;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use std::collections::HashSet;

struct Pattern {
    source: String,
    target: String,
}

pub struct GlossaryMatcher {
    patterns: Vec<Pattern>,
    automaton: Option<AhoCorasick>,
}

pub struct MatchResult {
    pub text: String,
    pub hints: Vec<(String, String)>,
}

fn cjk(character: char) -> bool {
    matches!(character as u32, 0x3040..=0x30FF | 0x3400..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

fn word_neighbor(character: char) -> bool {
    let code = character as u32;
    (character.is_alphanumeric() && !cjk(character))
        || character == '_'
        || matches!(code, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn valid_boundary(text: &str, start: usize, end: usize, needle: &str) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    let begins_word = needle.chars().next().is_some_and(word_neighbor);
    let ends_word = needle.chars().next_back().is_some_and(word_neighbor);
    (!begins_word || !before.is_some_and(word_neighbor))
        && (!ends_word || !after.is_some_and(word_neighbor))
}

impl GlossaryMatcher {
    pub fn new(entries: &[GlossaryEntry], target_lang: &str) -> Result<Self, String> {
        let mut patterns = Vec::new();
        let mut needles = Vec::new();
        let specific_sources: HashSet<_> = entries
            .iter()
            .filter(|entry| entry.target_lang == target_lang)
            .map(|entry| entry.source.to_ascii_lowercase())
            .collect();
        for entry in entries
            .iter()
            .filter(|entry| entry.target_lang == "*" || entry.target_lang == target_lang)
        {
            if entry.target_lang == "*"
                && specific_sources.contains(&entry.source.to_ascii_lowercase())
            {
                continue;
            }
            let source = entry.source.trim();
            let target = entry.target.trim();
            if source.is_empty()
                || target.is_empty()
                || source.contains(['\r', '\n'])
                || target.contains(['\r', '\n'])
            {
                continue;
            }
            for needle in std::iter::once(&entry.source).chain(entry.aliases.iter()) {
                let needle = needle.trim();
                if needle.is_empty() || needle.contains(['\r', '\n']) {
                    continue;
                }
                needles.push(needle.to_owned());
                patterns.push(Pattern {
                    source: source.to_owned(),
                    target: target.to_owned(),
                });
            }
        }
        let automaton = if needles.is_empty() {
            None
        } else {
            Some(
                AhoCorasickBuilder::new()
                    .ascii_case_insensitive(true)
                    .build(&needles)
                    .map_err(|error| error.to_string())?,
            )
        };
        Ok(Self {
            patterns,
            automaton,
        })
    }

    pub fn apply(&self, text: &str) -> MatchResult {
        let Some(automaton) = &self.automaton else {
            return MatchResult {
                text: text.to_owned(),
                hints: Vec::new(),
            };
        };
        let mut matches: Vec<_> = automaton
            .find_overlapping_iter(text)
            .filter(|found| {
                valid_boundary(
                    text,
                    found.start(),
                    found.end(),
                    &text[found.start()..found.end()],
                )
            })
            .collect();
        matches.sort_by_key(|found| {
            (
                std::cmp::Reverse(found.end() - found.start()),
                found.start(),
                found.pattern().as_usize(),
            )
        });
        let mut chosen = Vec::new();
        for found in matches {
            if chosen.iter().any(|selected: &aho_corasick::Match| {
                found.start() < selected.end() && selected.start() < found.end()
            }) {
                continue;
            }
            chosen.push(found);
        }
        chosen.sort_by_key(|found| found.start());
        let mut output = String::with_capacity(text.len());
        let mut hints = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = 0;
        for found in chosen {
            let pattern = &self.patterns[found.pattern().as_usize()];
            output.push_str(&text[cursor..found.start()]);
            output.push_str(&pattern.source);
            cursor = found.end();
            let pair = (pattern.source.clone(), pattern.target.clone());
            if hints.len() < 8 && seen.insert(pair.clone()) {
                hints.push(pair);
            }
        }
        output.push_str(&text[cursor..]);
        MatchResult {
            text: output,
            hints,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GlossaryMatcher;
    use crate::config::GlossaryEntry;

    fn term(source: &str, aliases: &[&str], target: &str, language: &str) -> GlossaryEntry {
        GlossaryEntry {
            source: source.into(),
            aliases: aliases.iter().map(|s| (*s).into()).collect(),
            target: target.into(),
            target_lang: language.into(),
        }
    }

    #[test]
    fn latin_aliases_correct_spelling_but_not_inside_other_words() {
        let matcher = GlossaryMatcher::new(
            &[term("LoRA", &["Loari", "Low-Ri"], "低秩适配", "zh")],
            "zh",
        )
        .unwrap();
        let result = matcher.apply("LoRa, Loari and Low-Ri, not Flora");
        assert_eq!(result.text, "LoRA, LoRA and LoRA, not Flora");
        assert_eq!(result.hints, vec![("LoRA".into(), "低秩适配".into())]);
    }

    #[test]
    fn mixed_chinese_latin_and_chinese_phrase_survive_utf8_slicing() {
        let matcher = GlossaryMatcher::new(
            &[
                term("LoRA", &["Loari"], "低秩适配", "zh"),
                term("人工智能", &[], "AI", "zh"),
            ],
            "zh",
        )
        .unwrap();
        let result = matcher.apply("我们讲人工智能Loari技术");
        assert_eq!(result.text, "我们讲人工智能LoRA技术");
        assert_eq!(result.hints.len(), 2);
    }

    #[test]
    fn longer_phrase_wins_and_other_language_is_ignored() {
        let matcher = GlossaryMatcher::new(
            &[
                term("New York", &[], "纽约", "zh"),
                term("New York City", &[], "纽约市", "zh"),
                term("GGUF", &[], "model", "en"),
            ],
            "zh",
        )
        .unwrap();
        let result = matcher.apply("New York City uses GGUF");
        assert_eq!(result.text, "New York City uses GGUF");
        assert_eq!(
            result.hints,
            vec![("New York City".into(), "纽约市".into())]
        );
        assert!(matcher.apply("nothing here").hints.is_empty());
    }

    #[test]
    fn longer_staggered_overlap_wins_even_when_it_starts_later() {
        let matcher = GlossaryMatcher::new(
            &[
                term("New York", &[], "纽约", "zh"),
                term("York City Hall", &[], "纽约市政厅", "zh"),
            ],
            "zh",
        )
        .unwrap();
        let result = matcher.apply("New York City Hall");
        assert_eq!(result.text, "New York City Hall");
        assert_eq!(
            result.hints,
            vec![("York City Hall".into(), "纽约市政厅".into())]
        );
    }

    #[test]
    fn accented_latin_letters_and_combining_marks_are_not_boundaries() {
        let matcher = GlossaryMatcher::new(
            &[
                term("CAF", &[], "organization", "en"),
                term("cafe", &[], "coffee shop", "en"),
                term("é", &[], "accent", "en"),
            ],
            "en",
        )
        .unwrap();
        let result = matcher.apply("café cafe\u{301} CAF é");
        assert_eq!(result.text, "café cafe\u{301} CAF é");
        assert_eq!(
            result.hints,
            vec![
                ("CAF".into(), "organization".into()),
                ("é".into(), "accent".into())
            ]
        );
    }

    #[test]
    fn target_specific_term_overrides_migrated_all_target_term() {
        let matcher = GlossaryMatcher::new(
            &[
                term("LoRA", &[], "old", "*"),
                term("LoRA", &["Loari"], "低秩适配", "zh"),
            ],
            "zh",
        )
        .unwrap();
        let result = matcher.apply("Loari and LoRA");
        assert_eq!(result.text, "LoRA and LoRA");
        assert_eq!(result.hints, vec![("LoRA".into(), "低秩适配".into())]);
    }

    #[test]
    fn prompt_hints_are_limited_and_inactive_entries_are_not_compiled() {
        let mut entries: Vec<_> = (0..5_000)
            .map(|n| term(&format!("other{n}"), &[], "x", "en"))
            .collect();
        for n in 0..20 {
            entries.push(term(&format!("term{n}"), &[], "x", "zh"));
        }
        let matcher = GlossaryMatcher::new(&entries, "zh").unwrap();
        assert_eq!(matcher.patterns.len(), 20);
        let text = (0..20)
            .map(|n| format!("term{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(matcher.apply(&text).hints.len(), 8);
    }
}
