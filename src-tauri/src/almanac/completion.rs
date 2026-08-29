//! Case-insensitive tab completion.
//!
//! Completion matching is case-insensitive (`doc` → `Documents`), but the
//! value it returns is the real, case-sensitive resource name, so the command
//! the user finally runs still executes case-sensitively.
//!
//! Completion must not leak the contents of a locked directory the session has
//! not authenticated: when the parent directory sits behind an un-cleared lock
//! boundary, the result is `locked = true` with no candidates.

use super::engine::{NATIVE_VERBS, PATH_VERBS};
use super::lexer::{is_almanac_line, lex, split_unescaped_gt, ALMANAC_KEYWORD};
use crate::error::AstraError;
use crate::fs_provider::AstraLocation;
use crate::state::SystemState;
use serde::Serialize;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct CompletionResult {
    /// The fully completed final token, when exactly one candidate matches.
    pub replacement: Option<String>,
    /// All matching names, when the prefix is ambiguous.
    pub candidates: Vec<String>,
    /// The parent directory is behind an un-cleared lock boundary.
    pub locked: bool,
}

impl CompletionResult {
    fn candidates(mut names: Vec<String>) -> Self {
        names.sort();
        Self {
            replacement: None,
            candidates: names,
            locked: false,
        }
    }

    fn one(replacement: String) -> Self {
        Self {
            replacement: Some(replacement),
            candidates: Vec::new(),
            locked: false,
        }
    }

    fn locked() -> Self {
        Self {
            replacement: None,
            candidates: Vec::new(),
            locked: true,
        }
    }
}

/// Compute completions for a raw terminal line and cursor-at-end.
pub fn complete(state: &SystemState, cwd: &str, line: &str) -> CompletionResult {
    if !is_almanac_line(line) {
        // Host commands are completed by the host shell, not Almanac.
        return CompletionResult::default();
    }
    // A trailing space means the previous token is finished; nothing to extend.
    if line.ends_with(char::is_whitespace) {
        return CompletionResult::default();
    }

    let mut tokens = lex(line);
    // Drop the leading `almanac` keyword.
    if tokens.first().map(String::as_str) == Some(ALMANAC_KEYWORD) {
        tokens.remove(0);
    }

    match tokens.as_slice() {
        [] => CompletionResult::default(),
        [partial_verb] => complete_verb(partial_verb),
        [verb, rest @ ..] if PATH_VERBS.contains(&verb.as_str()) => match rest.last() {
            Some(partial_path) => complete_path(state, cwd, partial_path),
            None => CompletionResult::default(),
        },
        _ => CompletionResult::default(),
    }
}

fn complete_verb(partial: &str) -> CompletionResult {
    let lower = partial.to_ascii_lowercase();
    let matches: Vec<String> = NATIVE_VERBS
        .iter()
        .filter(|verb| verb.to_ascii_lowercase().starts_with(&lower))
        .map(|verb| (*verb).to_string())
        .collect();
    match matches.as_slice() {
        [] => CompletionResult::default(),
        [only] => CompletionResult::one(only.clone()),
        _ => CompletionResult::candidates(matches),
    }
}

fn complete_path(state: &SystemState, cwd: &str, partial: &str) -> CompletionResult {
    let segments = split_unescaped_gt(partial);
    let (leaf, parent_segments) = segments
        .split_last()
        .map(|(leaf, rest)| (leaf.clone(), rest.to_vec()))
        .unwrap_or_default();

    let parent_relative = if parent_segments.is_empty() {
        ".".to_string()
    } else {
        parent_segments.join(">")
    };

    let children = match child_names(state, cwd, &parent_relative) {
        Ok(children) => children,
        Err(AstraError::ResourceAuthenticationRequired(_)) => return CompletionResult::locked(),
        Err(_) => return CompletionResult::default(),
    };

    let lower_leaf = leaf.to_ascii_lowercase();
    let matches: Vec<String> = children
        .into_iter()
        .filter(|name| name.to_ascii_lowercase().starts_with(&lower_leaf))
        .collect();

    let prefix = if parent_segments.is_empty() {
        String::new()
    } else {
        format!("{}>", parent_segments.join(">"))
    };

    match matches.as_slice() {
        [] => CompletionResult::default(),
        [only] => CompletionResult::one(format!("{prefix}{only}")),
        _ => CompletionResult::candidates(matches),
    }
}

/// Names of the entries directly under `parent_relative` (resolved against
/// `cwd`), whichever provider it lands on:
///
/// * a virtual directory → its children;
/// * `HOST` on its own → the mount aliases (so `HOST>Publ⇥` → `HOST>PublicDesktop`);
/// * a path inside a mount → that host directory's entries.
///
/// Lock boundaries are honoured on both sides: a directory the session has not
/// authenticated surfaces as [`AstraError::ResourceAuthenticationRequired`].
fn child_names(
    state: &SystemState,
    cwd: &str,
    parent_relative: &str,
) -> Result<Vec<String>, AstraError> {
    match state.route(cwd, parent_relative)? {
        AstraLocation::Virtual(_) => {
            let mut names: Vec<String> = state
                .completion_children(cwd, parent_relative)?
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            // `HOST` is an absolute root alongside `ROOT`; offer it when the
            // user is completing the first path segment.
            if parent_relative == "." {
                names.push("HOST".to_string());
            }
            Ok(names)
        }
        AstraLocation::HostRoot => Ok(state
            .host_mount_list()?
            .into_iter()
            .map(|mount| mount.alias)
            .collect()),
        AstraLocation::Host { mount, relative } => Ok(state
            .host_list(&mount, &relative)?
            .into_iter()
            .map(|entry| entry.name)
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::JsonPersistence;

    fn state(directory: &tempfile::TempDir) -> SystemState {
        let mut state =
            SystemState::fresh(JsonPersistence::new(directory.path().join("state.json")));
        state.configure_login("login-password").unwrap();
        state
    }

    #[test]
    fn case_insensitive_prefix_completes_to_the_real_name() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(&dir);
        // ROOT already contains Documents, Downloads, Desktop, … — "docu" is unique.
        let result = complete(&state, "ROOT", "almanac open docu");
        assert_eq!(result.replacement.as_deref(), Some("Documents"));
    }

    #[test]
    fn ambiguous_prefix_lists_candidates_instead_of_guessing() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(&dir);
        // "d" matches Desktop, Documents, Downloads.
        let result = complete(&state, "ROOT", "almanac open D");
        assert!(result.replacement.is_none());
        assert!(result.candidates.contains(&"Documents".to_string()));
        assert!(result.candidates.contains(&"Downloads".to_string()));
        assert!(result.candidates.contains(&"Desktop".to_string()));
    }

    #[test]
    fn completes_nested_path_segments() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(&dir);
        state
            .create_tree("ROOT>Projects", "AstraOS>(Frontend,Backend)")
            .unwrap();
        let result = complete(&state, "ROOT", "almanac open Projects>AstraOS>Fro");
        assert_eq!(
            result.replacement.as_deref(),
            Some("Projects>AstraOS>Frontend")
        );
    }

    #[test]
    fn does_not_expose_contents_of_an_unauthenticated_locked_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(&dir);
        state
            .create_tree("ROOT>Projects", "Vault>(TopSecret)")
            .unwrap();
        state
            .lock_resource("ROOT", "Projects>Vault", "vault-pass")
            .unwrap();
        state.logout();
        state.login("login-password").unwrap();

        let result = complete(&state, "ROOT", "almanac open Projects>Vault>Top");
        assert!(result.locked);
        assert!(result.candidates.is_empty());
        assert!(result.replacement.is_none());
    }

    #[test]
    fn completes_a_host_mount_alias_after_the_host_root() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = state(&dir);
        state
            .host_mount(work.path(), Some("PublicDesktop"))
            .unwrap();

        let result = complete(&state, "ROOT", "almanac open HOST>Publ");
        assert_eq!(result.replacement.as_deref(), Some("HOST>PublicDesktop"));
    }

    #[test]
    fn ambiguous_host_mount_prefix_lists_the_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut state = state(&dir);
        state.host_mount(a.path(), Some("Docs")).unwrap();
        state.host_mount(b.path(), Some("Downloads")).unwrap();

        let result = complete(&state, "ROOT", "almanac open HOST>Do");
        assert!(result.replacement.is_none());
        assert!(result.candidates.contains(&"Docs".to_string()));
        assert!(result.candidates.contains(&"Downloads".to_string()));
    }

    #[test]
    fn completes_entries_inside_a_host_mount() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::create_dir(work.path().join("Reports")).unwrap();
        let mut state = state(&dir);
        state
            .host_mount(work.path(), Some("PublicDesktop"))
            .unwrap();

        let result = complete(&state, "ROOT", "almanac open HOST>PublicDesktop>Rep");
        assert_eq!(
            result.replacement.as_deref(),
            Some("HOST>PublicDesktop>Reports")
        );
    }

    #[test]
    fn completes_a_bare_name_against_a_host_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::create_dir(work.path().join("Archive")).unwrap();
        let mut state = state(&dir);
        state.host_mount(work.path(), Some("Dev")).unwrap();

        let result = complete(&state, "HOST>Dev", "almanac open Arch");
        assert_eq!(result.replacement.as_deref(), Some("Archive"));
    }

    #[test]
    fn completes_the_host_root_token_itself() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(&dir);
        // No virtual ROOT child starts with "HO", so it is unambiguously HOST.
        let result = complete(&state, "ROOT", "almanac open HO");
        assert_eq!(result.replacement.as_deref(), Some("HOST"));
    }

    #[test]
    fn completes_verbs_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(&dir);
        assert_eq!(
            complete(&state, "ROOT", "almanac SCA")
                .replacement
                .as_deref(),
            Some("scan")
        );
        let ambiguous = complete(&state, "ROOT", "almanac r");
        assert!(ambiguous.candidates.contains(&"rename".to_string()));
        assert!(ambiguous.candidates.contains(&"rewrite".to_string()));
        assert!(ambiguous.candidates.contains(&"run".to_string()));
        assert!(ambiguous.candidates.contains(&"restart".to_string()));
    }
}
