//! Tokeniser for the *host shell fallback* path.
//!
//! This is deliberately **not** the Almanac path parser. It splits a raw
//! command line into a program name and argument vector so the arguments can
//! be handed to a real OS process API (`std::process::Command`) without ever
//! being concatenated back into a single string for `sh -c` / `cmd /C`.
//!
//! Shell metacharacters (`>`, `<`, `|`, `&&`, `;`) are treated as ordinary
//! argument text here: we never interpret redirection or pipelines because
//! that would require shell-string concatenation of untrusted input. The only
//! syntax understood is whitespace splitting and `"` / `'` quoting.

/// Split a host command line into whitespace-delimited tokens, honouring
/// single and double quotes. Returns an empty vector for blank input.
pub fn tokenize_host_line(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut quote: Option<char> = None;

    for character in input.chars() {
        match quote {
            Some(active) => {
                if character == active {
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

#[cfg(test)]
mod tests {
    use super::tokenize_host_line;

    #[test]
    fn splits_on_whitespace_and_keeps_redirection_as_literal_text() {
        assert_eq!(
            tokenize_host_line("echo hello > output.txt"),
            vec!["echo", "hello", ">", "output.txt"]
        );
    }

    #[test]
    fn honours_quotes() {
        assert_eq!(
            tokenize_host_line("git commit -m \"initial commit\""),
            vec!["git", "commit", "-m", "initial commit"]
        );
        assert_eq!(tokenize_host_line("   "), Vec::<String>::new());
    }
}
