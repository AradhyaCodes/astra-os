use crate::error::AstraError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub name: String,
    pub children: Vec<TreeNode>,
}

pub fn parse_tree(input: &str) -> Result<TreeNode, AstraError> {
    let mut parser = TreeParser::new(input);
    let node = parser.parse_node()?;
    parser.skip_whitespace();
    if parser.peek().is_some() {
        return parser.error("unexpected content after tree expression");
    }
    Ok(node)
}

struct TreeParser {
    chars: Vec<char>,
    position: usize,
}

impl TreeParser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            position: 0,
        }
    }

    fn parse_node(&mut self) -> Result<TreeNode, AstraError> {
        self.skip_whitespace();
        let name = self.parse_name()?;
        self.skip_whitespace();
        let mut children = Vec::new();

        if self.peek() == Some('>') {
            self.advance();
            self.skip_whitespace();
            self.expect('(')?;
            self.skip_whitespace();
            if self.peek() == Some(')') {
                return self.error("a generated directory cannot have an empty child list");
            }

            loop {
                children.push(self.parse_node()?);
                self.skip_whitespace();
                match self.peek() {
                    Some(',') => {
                        self.advance();
                        self.skip_whitespace();
                    }
                    Some(')') => {
                        self.advance();
                        break;
                    }
                    Some(_) => return self.error("expected ',' or ')' after child resource"),
                    None => return self.error("unterminated child list"),
                }
            }
        }

        Ok(TreeNode { name, children })
    }

    fn parse_name(&mut self) -> Result<String, AstraError> {
        let mut name = String::new();
        while let Some(character) = self.peek() {
            match character {
                '\\' => {
                    self.advance();
                    let escaped = self.peek().ok_or_else(|| {
                        AstraError::TreeParse("trailing escape character".to_string())
                    })?;
                    name.push(escaped);
                    self.advance();
                }
                '>' | '(' | ')' | ',' => break,
                character if character.is_whitespace() => break,
                _ => {
                    name.push(character);
                    self.advance();
                }
            }
        }

        if name.is_empty() {
            return self.error("expected a resource name");
        }
        Ok(name)
    }

    fn expect(&mut self, expected: char) -> Result<(), AstraError> {
        if self.peek() != Some(expected) {
            return self.error(&format!("expected '{expected}'"));
        }
        self.advance();
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.advance();
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn advance(&mut self) {
        self.position += 1;
    }

    fn error<T>(&self, message: &str) -> Result<T, AstraError> {
        Err(AstraError::TreeParse(format!(
            "{message} at character {}",
            self.position + 1
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_tree, TreeNode};

    #[test]
    fn parses_nested_tree_generation_syntax() {
        assert_eq!(
            parse_tree("file1>(file2>(file4),file3)").unwrap(),
            TreeNode {
                name: "file1".to_string(),
                children: vec![
                    TreeNode {
                        name: "file2".to_string(),
                        children: vec![TreeNode {
                            name: "file4".to_string(),
                            children: vec![],
                        }],
                    },
                    TreeNode {
                        name: "file3".to_string(),
                        children: vec![],
                    },
                ],
            }
        );
    }

    #[test]
    fn supports_escaped_parser_characters_in_names() {
        let parsed = parse_tree(r"Projects>(Docs\,Archive,Backend)").unwrap();
        assert_eq!(parsed.children[0].name, "Docs,Archive");
    }

    #[test]
    fn rejects_unterminated_trees() {
        assert!(parse_tree("Projects>(Frontend,Backend").is_err());
    }
}
