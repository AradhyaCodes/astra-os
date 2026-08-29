//! Controlled host-process execution layer.
//!
//! Responsibilities:
//! - spawn a real OS process from an already-parsed `program` + `args` vector
//!   (never a shell-concatenated string),
//! - stream stdout and stderr line-by-line as they are produced,
//! - expose the final exit status,
//! - support long-running processes (the reader loop blocks until the pipes
//!   close, so `npm run dev` streams indefinitely).
//!
//! `command not found` is reported as [`HostError::NotFound`] so callers can
//! distinguish it from an Almanac parser error or a non-zero exit code.

use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// A fully parsed request to run a host program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Real filesystem directory to run in, when one can be resolved.
    pub cwd: Option<PathBuf>,
}

impl HostCommand {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: None,
        }
    }
}

/// An event streamed back from a running host process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Emitted once, immediately after a successful spawn, carrying the OS PID
    /// so the process manager can track (and later terminate) it.
    Started {
        pid: u32,
    },
    Stdout {
        line: String,
    },
    Stderr {
        line: String,
    },
    Exit {
        code: Option<i32>,
        success: bool,
    },
    /// The process could not be started at all.
    Error {
        message: String,
        not_found: bool,
    },
}

/// Failure to *start* a host process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The executable was not found on `PATH` — distinct from a parser error.
    NotFound(String),
    /// The process existed but could not be spawned (permissions, etc.).
    Spawn(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::NotFound(program) => write!(f, "command not found: {program}"),
            HostError::Spawn(message) => write!(f, "could not start process: {message}"),
        }
    }
}

/// Abstraction over "run a process". The real implementation shells out to the
/// OS; tests substitute a recording double.
pub trait ProcessRunner: Send + Sync {
    /// Run `command`, invoking `on_event` for every streamed line and once more
    /// with [`StreamEvent::Exit`]. Returns the exit code (or `-1` when the
    /// platform reports none, e.g. killed by signal).
    fn run(
        &self,
        command: &HostCommand,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<i32, HostError>;
}

/// Production [`ProcessRunner`] backed by `std::process::Command`.
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(
        &self,
        command: &HostCommand,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<i32, HostError> {
        let mut builder = Command::new(&command.program);
        builder
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = &command.cwd {
            builder.current_dir(directory);
        }

        let mut child = builder.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                HostError::NotFound(command.program.clone())
            } else {
                HostError::Spawn(error.to_string())
            }
        })?;

        on_event(StreamEvent::Started { pid: child.id() });

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        std::thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel::<StreamEvent>();
            if let Some(stdout) = stdout {
                let sender = sender.clone();
                scope.spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if sender.send(StreamEvent::Stdout { line }).is_err() {
                            break;
                        }
                    }
                });
            }
            if let Some(stderr) = stderr {
                let sender = sender.clone();
                scope.spawn(move || {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        if sender.send(StreamEvent::Stderr { line }).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(sender);
            for event in receiver {
                on_event(event);
            }
        });

        let status = child
            .wait()
            .map_err(|error| HostError::Spawn(error.to_string()))?;
        let code = status.code();
        on_event(StreamEvent::Exit {
            code,
            success: status.success(),
        });
        Ok(code.unwrap_or(-1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_command() -> HostCommand {
        if cfg!(windows) {
            HostCommand::new("cmd", vec!["/C".into(), "echo".into(), "aaru".into()])
        } else {
            HostCommand::new("printf", vec!["aaru".into()])
        }
    }

    fn failing_command() -> HostCommand {
        if cfg!(windows) {
            HostCommand::new("cmd", vec!["/C".into(), "exit".into(), "1".into()])
        } else {
            HostCommand::new("sh", vec!["-c".into(), "exit 1".into()])
        }
    }

    #[test]
    fn streams_stdout_and_reports_success() {
        let mut events = Vec::new();
        let code = SystemProcessRunner
            .run(&echo_command(), &mut |event| events.push(event))
            .unwrap();
        assert_eq!(code, 0);
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::Stdout { line } if line.contains("aaru")
        )));
        assert!(events
            .iter()
            .any(|event| matches!(event, StreamEvent::Exit { success: true, .. })));
    }

    #[test]
    fn non_zero_exit_is_reported_without_being_an_error() {
        let mut events = Vec::new();
        let code = SystemProcessRunner
            .run(&failing_command(), &mut |event| events.push(event))
            .unwrap();
        assert_eq!(code, 1);
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::Exit {
                success: false,
                code: Some(1)
            }
        )));
    }

    #[test]
    fn missing_executable_is_command_not_found() {
        let result = SystemProcessRunner.run(
            &HostCommand::new("aaru-nonexistent-binary-xyz", vec![]),
            &mut |_| {},
        );
        assert!(matches!(result, Err(HostError::NotFound(_))));
    }
}
