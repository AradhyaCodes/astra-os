//! Pure path router: given the current working location and a user-supplied
//! path, decide whether it addresses the virtual Astra filesystem or a mounted
//! host folder. No filesystem access happens here.

use crate::almanac::lexer::split_unescaped_gt;
use crate::error::AstraError;
use crate::filesystem::path::normalize_path;

use super::host::HOST_LABEL;

/// A fully-routed target location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstraLocation {
    /// Canonical virtual path, e.g. `ROOT>Documents>Projects`.
    Virtual(String),
    /// `HOST` on its own — the list of mounts.
    HostRoot,
    /// A path inside a mounted host folder. `relative` has already had `.` and
    /// `..` resolved *logically*; the host provider still re-checks the real
    /// canonical path against the mount root.
    Host {
        mount: String,
        relative: Vec<String>,
    },
}

/// Optional scheme prefix seen in documentation (`ASTRA::HOST>…`). Accepted and
/// stripped so both spellings work.
const SCHEME_PREFIX: &str = "ASTRA::";
const LEGACY_SCHEME_PREFIX: &str = "AARU::";

/// Alias for the virtual root: `ASTRA>Documents` means the same as
/// `ROOT>Documents` (the desktop labels the virtual filesystem "ASTRA").
const VIRTUAL_ROOT_ALIAS: &str = "ASTRA";
const LEGACY_VIRTUAL_ROOT_ALIAS: &str = "AARU";

/// Route `path`, resolved against `cwd`, to a provider location.
pub fn route(cwd: &str, path: &str) -> Result<AstraLocation, AstraError> {
    let path = path.trim();
    let path = path
        .strip_prefix(SCHEME_PREFIX)
        .or_else(|| path.strip_prefix(LEGACY_SCHEME_PREFIX))
        .unwrap_or(path);
    let cwd = cwd.trim();
    let cwd = cwd
        .strip_prefix(SCHEME_PREFIX)
        .or_else(|| cwd.strip_prefix(LEGACY_SCHEME_PREFIX))
        .unwrap_or(cwd);

    let path_segments = split_unescaped_gt(path);
    let first = path_segments
        .first()
        .map(String::as_str)
        .unwrap_or_default();

    // An absolute path names its own root; a relative one inherits `cwd`'s.
    let absolute = matches!(
        first,
        HOST_LABEL | "ROOT" | VIRTUAL_ROOT_ALIAS | LEGACY_VIRTUAL_ROOT_ALIAS
    );
    let mut combined: Vec<String> = if absolute || path.is_empty() {
        if path.is_empty() {
            split_unescaped_gt(cwd)
        } else {
            path_segments
        }
    } else {
        let mut base = split_unescaped_gt(cwd);
        base.extend(path_segments);
        base
    };

    // `ASTRA` is a user-facing alias for the virtual root `ROOT`.
    if matches!(
        combined.first().map(String::as_str),
        Some(VIRTUAL_ROOT_ALIAS | LEGACY_VIRTUAL_ROOT_ALIAS)
    ) {
        combined[0] = "ROOT".to_string();
    }

    if combined.first().map(String::as_str) == Some(HOST_LABEL) {
        route_host(&combined[1..])
    } else {
        // Hand the rest to the existing virtual path normaliser, which already
        // understands `ROOT`, `.`/`..`, escapes and legacy `/` paths.
        let joined = combined.join(">");
        let canonical = normalize_path("ROOT", if joined.is_empty() { "." } else { &joined })?;
        Ok(AstraLocation::Virtual(canonical))
    }
}

fn route_host(rest: &[String]) -> Result<AstraLocation, AstraError> {
    let mut rest = rest.iter().filter(|segment| !segment.is_empty());
    let Some(mount) = rest.next() else {
        return Ok(AstraLocation::HostRoot);
    };

    let mut relative: Vec<String> = Vec::new();
    for segment in rest {
        match segment.as_str() {
            "." => {}
            ".." => {
                if relative.pop().is_none() {
                    return Err(AstraError::PermissionDenied(format!(
                        "path escapes the mount root {HOST_LABEL}>{mount}"
                    )));
                }
            }
            name => relative.push(name.to_string()),
        }
    }

    Ok(AstraLocation::Host {
        mount: mount.to_string(),
        relative,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_paths_route_to_the_virtual_provider() {
        assert_eq!(
            route("ROOT", "Documents>Projects").unwrap(),
            AstraLocation::Virtual("ROOT>Documents>Projects".to_string())
        );
        assert_eq!(
            route("ROOT>Documents", "Projects").unwrap(),
            AstraLocation::Virtual("ROOT>Documents>Projects".to_string())
        );
    }

    #[test]
    fn astra_is_an_absolute_alias_for_the_virtual_root() {
        assert_eq!(
            route("HOST>Dev", "ASTRA>Documents>notes.txt").unwrap(),
            AstraLocation::Virtual("ROOT>Documents>notes.txt".to_string())
        );
        assert_eq!(
            route("ROOT", "ASTRA").unwrap(),
            AstraLocation::Virtual("ROOT".to_string())
        );
    }

    #[test]
    fn legacy_aaru_paths_continue_to_route_after_the_rename() {
        assert_eq!(
            route("HOST>Dev", "AARU>Documents>notes.txt").unwrap(),
            AstraLocation::Virtual("ROOT>Documents>notes.txt".to_string())
        );
        assert_eq!(
            route("ROOT", "AARU::HOST>Downloads").unwrap(),
            AstraLocation::Host {
                mount: "Downloads".to_string(),
                relative: vec![],
            }
        );
    }

    #[test]
    fn host_paths_route_to_the_host_provider() {
        assert_eq!(
            route("ROOT", "HOST>Documents>Report.docx").unwrap(),
            AstraLocation::Host {
                mount: "Documents".to_string(),
                relative: vec!["Report.docx".to_string()],
            }
        );
        assert_eq!(route("ROOT", "HOST").unwrap(), AstraLocation::HostRoot);
        assert_eq!(
            route("ROOT", "ASTRA::HOST>Downloads").unwrap(),
            AstraLocation::Host {
                mount: "Downloads".to_string(),
                relative: vec![],
            }
        );
    }

    #[test]
    fn relative_paths_inherit_a_host_cwd() {
        assert_eq!(
            route("HOST>Documents", "University>notes.txt").unwrap(),
            AstraLocation::Host {
                mount: "Documents".to_string(),
                relative: vec!["University".to_string(), "notes.txt".to_string()],
            }
        );
    }

    #[test]
    fn dot_dot_cannot_escape_the_mount() {
        assert!(matches!(
            route("ROOT", "HOST>Documents>..>..>Windows"),
            Err(AstraError::PermissionDenied(_))
        ));
        // ..-within-bounds is fine
        assert_eq!(
            route("ROOT", "HOST>Documents>a>..>b").unwrap(),
            AstraLocation::Host {
                mount: "Documents".to_string(),
                relative: vec!["b".to_string()],
            }
        );
    }

    #[test]
    fn an_absolute_virtual_path_overrides_a_host_cwd() {
        assert_eq!(
            route("HOST>Documents", "ROOT>Downloads").unwrap(),
            AstraLocation::Virtual("ROOT>Downloads".to_string())
        );
    }
}
