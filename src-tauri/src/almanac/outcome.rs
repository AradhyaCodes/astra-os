//! Structured result of evaluating an Almanac line.

use crate::error::AaruError;
use crate::shell::HostCommand;
use serde::Serialize;

/// Consistent status prefixes for every Almanac line of output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum StatusTag {
    Ok,
    Info,
    Error,
    Denied,
    Locked,
    Auth,
    Process,
    System,
}

impl std::fmt::Display for StatusTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            StatusTag::Ok => "OK",
            StatusTag::Info => "INFO",
            StatusTag::Error => "ERROR",
            StatusTag::Denied => "DENIED",
            StatusTag::Locked => "LOCKED",
            StatusTag::Auth => "AUTH",
            StatusTag::Process => "PROCESS",
            StatusTag::System => "SYSTEM",
        };
        f.write_str(text)
    }
}

/// One line of terminal output plus its status tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputLine {
    pub tag: StatusTag,
    pub text: String,
}

impl OutputLine {
    pub fn new(tag: StatusTag, text: impl Into<String>) -> Self {
        Self {
            tag,
            text: text.into(),
        }
    }

    /// `[TAG] text` — used by tests and by any non-structured renderer.
    pub fn rendered(&self) -> String {
        format!("[{}] {}", self.tag, self.text)
    }

    /// Map a backend error onto the most appropriate status tag.
    pub fn from_error(error: &AaruError) -> Self {
        let tag = match error {
            AaruError::PermissionDenied(_) => StatusTag::Denied,
            AaruError::ResourceAuthenticationRequired(_) => StatusTag::Locked,
            AaruError::AuthenticationRequired
            | AaruError::AuthenticationFailed
            | AaruError::AccountLocked { .. }
            | AaruError::CredentialsNotConfigured
            | AaruError::CredentialsAlreadyConfigured => StatusTag::Auth,
            _ => StatusTag::Error,
        };
        OutputLine::new(tag, error.to_string())
    }
}

/// A request for the terminal UI to collect one line of input and return it via
/// `almanac_respond`. `masked` inputs (passwords, confirmations) must never be
/// echoed and must never be written to command history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptRequest {
    pub id: String,
    pub kind: String,
    pub message: String,
    pub masked: bool,
}

/// Application-launch abstraction. Real process management arrives in a later
/// phase; for now this is the stable boundary the UI can react to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppLaunch {
    pub app: String,
    pub path: Option<String>,
    pub args: Vec<String>,
}

/// A whole-session action the command layer must carry out after replying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemAction {
    Shutdown,
    Restart,
    Hibernate,
    LoggedOut,
}

/// Info about a spawned host process (streaming happens over Tauri events).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessView {
    pub id: String,
    pub program: String,
}

/// Everything the terminal needs to render one evaluation.
#[derive(Debug, Default, Serialize)]
pub struct AlmanacOutcome {
    pub lines: Vec<OutputLine>,
    pub new_cwd: Option<String>,
    pub clear: bool,
    pub prompt: Option<PromptRequest>,
    pub process: Option<ProcessView>,
    pub launch: Option<AppLaunch>,
    pub system_action: Option<SystemAction>,
    /// The UI should open the native directory picker and re-issue
    /// `almanac mount "<picked path>"`.
    pub request_mount: bool,
    /// AppId of a Tauri window the UI should open (built-in app launch).
    pub open_window: Option<String>,
    /// Window title to use when `open_window` is a generic app shell.
    pub open_window_title: Option<String>,
    /// Internal: a host process the command layer should spawn + stream.
    /// Never serialised to the frontend.
    #[serde(skip)]
    pub shell: Option<HostCommand>,
}

impl AlmanacOutcome {
    pub fn line(tag: StatusTag, text: impl Into<String>) -> Self {
        Self {
            lines: vec![OutputLine::new(tag, text)],
            ..Self::default()
        }
    }

    pub fn push(&mut self, tag: StatusTag, text: impl Into<String>) {
        self.lines.push(OutputLine::new(tag, text));
    }

    pub fn from_error(error: &AaruError) -> Self {
        Self {
            lines: vec![OutputLine::from_error(error)],
            ..Self::default()
        }
    }

    /// Convenience for tests: all lines rendered as `[TAG] text`.
    pub fn rendered(&self) -> Vec<String> {
        self.lines.iter().map(OutputLine::rendered).collect()
    }

    pub fn first_tag(&self) -> Option<StatusTag> {
        self.lines.first().map(|line| line.tag)
    }
}
