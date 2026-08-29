use super::model::ROOT_NAME;
use crate::error::AstraError;

/// Resolve an Astra path against a current directory and return its canonical
/// form. A literal `>` or `\` inside a name is escaped with `\`.
pub fn normalize_path(cwd: &str, input: &str) -> Result<String, AstraError> {
    let cwd_components = parse_canonical_cwd(cwd)?;
    let normalized_input = input.trim();
    if normalized_input.is_empty() || normalized_input == "." {
        return Ok(canonicalize(&cwd_components));
    }

    let legacy_absolute = normalized_input.starts_with('/');
    let mut input_components = if normalized_input.contains('/') {
        normalized_input
            .trim_start_matches('/')
            .split('/')
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        parse_astra_components(normalized_input)?
    };
    let astra_absolute = input_components
        .first()
        .is_some_and(|part| part == ROOT_NAME);
    if astra_absolute {
        input_components.remove(0);
    }

    let mut resolved = if legacy_absolute || astra_absolute {
        Vec::new()
    } else {
        cwd_components
    };
    for component in input_components {
        if component.is_empty() {
            return Err(AstraError::InvalidPath(
                "path contains an empty component".to_string(),
            ));
        }
        match component.as_str() {
            "." => {}
            ".." => {
                resolved.pop();
            }
            ROOT_NAME => {
                return Err(AstraError::InvalidPath(format!(
                    "'{ROOT_NAME}' is only valid at the beginning of an absolute path"
                )));
            }
            _ => resolved.push(component),
        }
    }

    Ok(canonicalize(&resolved))
}

/// Split a canonical or resolvable path into its canonical parent and raw name.
pub fn split_path(path: &str) -> Result<(String, String), AstraError> {
    let canonical = normalize_path(ROOT_NAME, path)?;
    let mut raw_components = parse_astra_components(&canonical)?;
    if raw_components == [ROOT_NAME] {
        return Ok((ROOT_NAME.to_string(), String::new()));
    }

    let name = raw_components.pop().unwrap_or_default();
    raw_components.remove(0);
    Ok((canonicalize(&raw_components), name))
}

pub fn parent_path(path: &str) -> Result<String, AstraError> {
    Ok(split_path(path)?.0)
}

pub fn depth(path: &str) -> Result<usize, AstraError> {
    Ok(components(path)?.len())
}

/// Return raw, unescaped resource names for traversal.
pub fn components(path: &str) -> Result<Vec<String>, AstraError> {
    let canonical = normalize_path(ROOT_NAME, path)?;
    let mut raw_components = parse_astra_components(&canonical)?;
    if raw_components.first().is_some_and(|part| part == ROOT_NAME) {
        raw_components.remove(0);
    }
    Ok(raw_components)
}

pub fn join(parent: &str, name: &str) -> String {
    format!("{parent}>{}", escape_component(name))
}

fn parse_canonical_cwd(cwd: &str) -> Result<Vec<String>, AstraError> {
    if cwd == "/" {
        return Ok(Vec::new());
    }

    let mut parsed = if cwd.starts_with('/') {
        cwd.trim_start_matches('/')
            .split('/')
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        parse_astra_components(cwd)?
    };
    if parsed.first().is_some_and(|part| part == ROOT_NAME) {
        parsed.remove(0);
    }
    if parsed.iter().any(String::is_empty) {
        return Err(AstraError::InvalidPath(format!(
            "invalid current directory: {cwd}"
        )));
    }
    Ok(parsed)
}

fn parse_astra_components(path: &str) -> Result<Vec<String>, AstraError> {
    let mut components = Vec::new();
    let mut component = String::new();
    let mut escaped = false;

    for character in path.chars() {
        if escaped {
            component.push(character);
            escaped = false;
        } else {
            match character {
                '\\' => escaped = true,
                '>' => {
                    components.push(component);
                    component = String::new();
                }
                _ => component.push(character),
            }
        }
    }
    if escaped {
        return Err(AstraError::InvalidPath(
            "path ends with an incomplete escape".to_string(),
        ));
    }
    components.push(component);
    Ok(components)
}

fn escape_component(component: &str) -> String {
    let mut escaped = String::new();
    for character in component.chars() {
        if matches!(character, '\\' | '>') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn canonicalize(components: &[String]) -> String {
    if components.is_empty() {
        ROOT_NAME.to_string()
    } else {
        let encoded = components
            .iter()
            .map(|component| escape_component(component))
            .collect::<Vec<_>>()
            .join(">");
        format!("{ROOT_NAME}>{encoded}")
    }
}

#[cfg(test)]
mod tests {
    use super::{components, depth, join, normalize_path, parent_path, split_path};

    #[test]
    fn resolves_absolute_and_relative_astra_paths() {
        assert_eq!(
            normalize_path("ROOT>Documents", "Projects>AstraOS").unwrap(),
            "ROOT>Documents>Projects>AstraOS"
        );
        assert_eq!(
            normalize_path("ROOT>Documents", "ROOT>Downloads").unwrap(),
            "ROOT>Downloads"
        );
        assert_eq!(
            normalize_path("ROOT>Documents>Projects", "..>Pictures").unwrap(),
            "ROOT>Documents>Pictures"
        );
    }

    #[test]
    fn escapes_literal_path_separators_in_resource_names() {
        let path = join("ROOT>Projects", "Release>2026");
        assert_eq!(path, r"ROOT>Projects>Release\>2026");
        assert_eq!(components(&path).unwrap(), vec!["Projects", "Release>2026"]);
        assert_eq!(split_path(&path).unwrap().1, "Release>2026");
    }

    #[test]
    fn accepts_legacy_slash_paths_for_almanac_compatibility() {
        assert_eq!(normalize_path("/", "/Documents").unwrap(), "ROOT>Documents");
        assert_eq!(
            normalize_path("/Documents", "Projects").unwrap(),
            "ROOT>Documents>Projects"
        );
    }

    #[test]
    fn returns_parent_root_and_depth() {
        assert_eq!(
            parent_path("ROOT>Documents>Projects").unwrap(),
            "ROOT>Documents"
        );
        assert_eq!(split_path("ROOT>Documents").unwrap().1, "Documents");
        assert_eq!(depth("ROOT").unwrap(), 0);
        assert_eq!(depth("ROOT>Documents>Projects").unwrap(), 2);
    }
}
