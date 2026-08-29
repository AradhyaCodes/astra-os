//! Almanac line lexer.
//!
//! This lexer is intentionally separate from both the host-shell tokeniser
//! ([`crate::shell::tokenize`]) and the Aaru path parser
//! ([`crate::filesystem::path`]).
//!
//! It performs exactly one job: split an Almanac command line into
//! whitespace-delimited tokens while honouring `"` and `'` quoting (needed for
//! host paths containing spaces: `almanac open "HOST>Desktop>My App.lnk"`).
//! Crucially it does **not** treat `>` as anything special — inside Almanac,
//! `>` is always a path separator and stays attached to its token
//! (`open Documents>Projects` → `["open", "Documents>Projects"]`). Backslash
//! escapes such as `\>` and `\,` are passed through untouched so the
//! downstream path / tree parsers can interpret them.

/// The leading keyword that routes a line to the Almanac engine instead of the
/// host shell.
pub const ALMANAC_KEYWORD: &str = "almanac";

/// Split an Almanac command line (already stripped of the leading `almanac`
/// keyword or not — the caller decides) into tokens.
pub fn lex(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    // Which quote character (if any) is currently open. `'` inside `"…"` (and
    // vice versa) is a literal.
    let mut quote: Option<char> = None;

    for character in input.chars() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                } else {
                    current.push(character);
                }
            }
            None => match character {
                '"' | '\'' => {
                    has_token = true;
                    quote = Some(character);
                }
                c if c.is_whitespace() => {
                    if has_token {
                        tokens.push(std::mem::take(&mut current));
                        has_token = false;
                    }
                }
                c => {
                    has_token = true;
                    current.push(c);
                }
            },
        }
    }

    if has_token {
        tokens.push(current);
    }
    tokens
}

/// Does this raw line address the Almanac engine? True when the first
/// whitespace-delimited word is exactly `almanac` (case-sensitive, matching
/// the case-sensitive execution rule).
pub fn is_almanac_line(input: &str) -> bool {
    input
        .split_whitespace()
        .next()
        .is_some_and(|word| word == ALMANAC_KEYWORD)
}

/// Split a single token on unescaped `>` characters, returning the raw
/// segments with their escapes preserved. Used by `rename` to separate an
/// existing resource path from the new leaf name without a naive `split('>')`.
pub fn split_unescaped_gt(token: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for character in token.chars() {
        if escaped {
            current.push('\\');
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '>' => segments.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    if escaped {
        current.push('\\');
    }
    segments.push(current);
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_path_separators_inside_tokens() {
        assert_eq!(
            lex("open Documents>Projects>AaruOS"),
            vec!["open", "Documents>Projects>AaruOS"]
        );
    }

    #[test]
    fn honours_quotes_and_collapses_whitespace() {
        assert_eq!(
            lex("  write   notes.txt  in   VSCode "),
            vec!["write", "notes.txt", "in", "VSCode"]
        );
        assert_eq!(
            lex("lookout \"quarterly report\""),
            vec!["lookout", "quarterly report"]
        );
    }

    #[test]
    fn single_and_double_quotes_both_group_spaces() {
        assert_eq!(
            lex("open 'HOST>Desktop>My App.lnk'"),
            vec!["open", "HOST>Desktop>My App.lnk"]
        );
        assert_eq!(
            lex("rewrite \"HOST>Desktop>年度 report.txt\" in VSCode"),
            vec!["rewrite", "HOST>Desktop>年度 report.txt", "in", "VSCode"]
        );
        // The other quote char is literal inside a quoted span.
        assert_eq!(
            lex("write \"it's here.txt\""),
            vec!["write", "it's here.txt"]
        );
    }

    #[test]
    fn routes_only_exact_keyword() {
        assert!(is_almanac_line("almanac open Documents"));
        assert!(!is_almanac_line("Almanac open Documents"));
        assert!(!is_almanac_line("almanacX"));
        assert!(!is_almanac_line("git status"));
    }

    #[test]
    fn splits_rename_target_on_unescaped_separator_only() {
        assert_eq!(
            split_unescaped_gt("old.txt>new.txt"),
            vec!["old.txt", "new.txt"]
        );
        assert_eq!(
            split_unescaped_gt(r"Projects>Reports>Q1\>2026>Q2\>2026"),
            vec!["Projects", "Reports", r"Q1\>2026", r"Q2\>2026"]
        );
    }
}
