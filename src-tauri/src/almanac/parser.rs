//! Token stream → [`AlmanacCommand`].
//!
//! Parser errors are always [`AstraError::AlmanacParse`] so the engine can label
//! them `[ERROR] almanac: …` and callers can tell them apart from a host
//! `command not found`.

use super::ast::{AlmanacCommand, EditorTarget};
use super::lexer::{lex, split_unescaped_gt};
use crate::error::AstraError;
use crate::memory::parse_replacement_policy;
use crate::scheduler::parse_algorithm;

/// Parse a full Almanac line *including* the leading `almanac` keyword.
pub fn parse_line(line: &str) -> Result<AlmanacCommand, AstraError> {
    let tokens = lex(line);
    let mut tokens = tokens.as_slice();
    match tokens.split_first() {
        Some((first, rest)) if first == super::lexer::ALMANAC_KEYWORD => tokens = rest,
        _ => {
            return Err(AstraError::AlmanacParse(
                "expected an 'almanac …' command".to_string(),
            ))
        }
    }
    parse_tokens(tokens)
}

/// Parse the tokens that follow the `almanac` keyword.
pub fn parse_tokens(tokens: &[String]) -> Result<AlmanacCommand, AstraError> {
    let Some((verb, args)) = tokens.split_first() else {
        return Ok(AlmanacCommand::Help);
    };

    match verb.as_str() {
        "open" => {
            let (path, editor) = editor_form(verb, args)?;
            Ok(AlmanacCommand::Open { path, editor })
        }
        "back" => {
            no_args(verb, args)?;
            Ok(AlmanacCommand::Back)
        }
        "root" => {
            no_args(verb, args)?;
            Ok(AlmanacCommand::Root)
        }
        "scan" => {
            no_args(verb, args)?;
            Ok(AlmanacCommand::Scan)
        }
        "gen" => Ok(AlmanacCommand::Gen {
            path: single_path(verb, args)?,
        }),
        "mgen" => {
            if args.is_empty() {
                return Err(missing(verb, "a tree expression such as Projects>(A,B,C)"));
            }
            Ok(AlmanacCommand::Mgen {
                expression: args.join(" "),
            })
        }
        "write" => {
            let (path, editor) = editor_form(verb, args)?;
            Ok(AlmanacCommand::Write { path, editor })
        }
        "rewrite" => {
            let (path, editor) = editor_form(verb, args)?;
            Ok(AlmanacCommand::Rewrite { path, editor })
        }
        "destroy" => Ok(AlmanacCommand::Destroy {
            path: single_path(verb, args)?,
        }),
        "rename" => parse_rename(args),
        "transfer" => {
            let (from, to) = two_paths(verb, args)?;
            Ok(AlmanacCommand::Transfer { from, to })
        }
        "copy" => {
            let (from, to) = two_paths(verb, args)?;
            Ok(AlmanacCommand::Copy { from, to })
        }
        "lookout" => {
            if args.is_empty() {
                return Err(missing(verb, "a search term"));
            }
            Ok(AlmanacCommand::Lookout {
                query: args.join(" "),
            })
        }
        "inspect" => Ok(AlmanacCommand::Inspect {
            path: single_path(verb, args)?,
        }),
        "lock" => Ok(AlmanacCommand::Lock {
            path: single_path(verb, args)?,
        }),
        "unlock" => Ok(AlmanacCommand::Unlock {
            path: single_path(verb, args)?,
        }),
        "mount" => match args {
            [] => Ok(AlmanacCommand::Mount { path: None }),
            [path] => Ok(AlmanacCommand::Mount {
                path: Some(path.clone()),
            }),
            _ => Err(AstraError::AlmanacParse(
                "mount takes no argument (opens a picker) or one Windows path".to_string(),
            )),
        },
        "unmount" => match args {
            [alias] => Ok(AlmanacCommand::Unmount {
                alias: alias.clone(),
            }),
            _ => Err(missing(verb, "a mount alias")),
        },
        "mounts" => {
            no_args(verb, args)?;
            Ok(AlmanacCommand::Mounts)
        }
        "run" => {
            let Some((application, rest)) = args.split_first() else {
                return Err(missing(verb, "an application name"));
            };
            Ok(AlmanacCommand::Run {
                application: application.clone(),
                args: rest.to_vec(),
            })
        }
        "reveal" => Ok(AlmanacCommand::Reveal {
            path: single_path(verb, args)?,
        }),
        "process" | "processes" => {
            no_args("process", args)?;
            Ok(AlmanacCommand::Process)
        }
        "terminate" => Ok(AlmanacCommand::Terminate {
            pid: pid_arg(verb, args)?,
        }),
        "suspend" => Ok(AlmanacCommand::Suspend {
            pid: pid_arg(verb, args)?,
        }),
        "resume" => Ok(AlmanacCommand::Resume {
            pid: pid_arg(verb, args)?,
        }),
        "scheduler" | "sched" => parse_scheduler(args),
        "memory" | "mem" => parse_memory(args),
        "logout" => {
            no_args(verb, args)?;
            Ok(AlmanacCommand::Logout)
        }
        "kill" => match args {
            [target] if target == "lapsession" => Ok(AlmanacCommand::KillLapsession),
            _ => Err(AstraError::AlmanacParse(
                "did you mean 'almanac kill lapsession'? (this shuts Astra OS down)".to_string(),
            )),
        },
        "hibernate" => {
            no_args(verb, args)?;
            Ok(AlmanacCommand::Hibernate)
        }
        "restart" => {
            no_args(verb, args)?;
            Ok(AlmanacCommand::Restart)
        }
        other => Err(AstraError::AlmanacParse(format!(
            "unknown command '{other}'. Type 'almanac' for the command reference"
        ))),
    }
}

fn parse_scheduler(args: &[String]) -> Result<AlmanacCommand, AstraError> {
    match args {
        [] => Ok(AlmanacCommand::Scheduler),
        [keyword, algorithm]
            if keyword.eq_ignore_ascii_case("change") || keyword.eq_ignore_ascii_case("set") =>
        {
            Ok(AlmanacCommand::SchedulerChange {
                algorithm: parse_algorithm(algorithm)?,
            })
        }
        [keyword] if keyword.eq_ignore_ascii_case("tick") => {
            Ok(AlmanacCommand::SchedulerTick { ticks: 1 })
        }
        [keyword, count] if keyword.eq_ignore_ascii_case("tick") => {
            let ticks = count
                .parse::<u64>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| {
                    AstraError::AlmanacParse(format!(
                        "scheduler tick expects a positive whole number, got '{count}'"
                    ))
                })?;
            Ok(AlmanacCommand::SchedulerTick {
                ticks: ticks.min(100_000),
            })
        }
        _ => Err(AstraError::AlmanacParse(
            "usage: almanac scheduler [change <RR|FCFS|Priority>] | [tick <n>]".to_string(),
        )),
    }
}

fn parse_memory(args: &[String]) -> Result<AlmanacCommand, AstraError> {
    match args {
        [] => Ok(AlmanacCommand::Memory),
        [keyword, policy]
            if keyword.eq_ignore_ascii_case("policy")
                || keyword.eq_ignore_ascii_case("change")
                || keyword.eq_ignore_ascii_case("replacement") =>
        {
            parse_replacement_policy(policy)
                .map(|policy| AlmanacCommand::MemorySetPolicy { policy })
                .ok_or_else(|| {
                    AstraError::AlmanacParse(format!(
                        "unknown replacement policy '{policy}' — use FIFO or LRU"
                    ))
                })
        }
        _ => Err(AstraError::AlmanacParse(
            "usage: almanac memory [policy <FIFO|LRU>]".to_string(),
        )),
    }
}

fn parse_rename(args: &[String]) -> Result<AlmanacCommand, AstraError> {
    match args {
        // `almanac rename originalPath>newName`
        [combined] => {
            let mut segments = split_unescaped_gt(combined);
            if segments.len() < 2 {
                return Err(AstraError::AlmanacParse(
                    "rename expects originalName>newName (for nested resources: \
                     Parent>Child>newName)"
                        .to_string(),
                ));
            }
            let new_name = segments.pop().unwrap();
            let path = segments.join(">");
            if path.trim().is_empty() || new_name.trim().is_empty() {
                return Err(AstraError::AlmanacParse(
                    "rename expects a non-empty original path and new name".to_string(),
                ));
            }
            Ok(AlmanacCommand::Rename { path, new_name })
        }
        // convenience: `almanac rename <path> <newName>`
        [path, new_name] => Ok(AlmanacCommand::Rename {
            path: path.clone(),
            new_name: new_name.clone(),
        }),
        _ => Err(AstraError::AlmanacParse(
            "rename expects originalName>newName".to_string(),
        )),
    }
}

fn editor_form(verb: &str, args: &[String]) -> Result<(String, EditorTarget), AstraError> {
    match args {
        [path] => Ok((path.clone(), EditorTarget::None)),
        [path, keyword, app] if keyword == "in" => {
            Ok((path.clone(), EditorTarget::App(app.clone())))
        }
        [] => Err(missing(verb, "a file path")),
        _ => Err(AstraError::AlmanacParse(format!(
            "{verb} expects '<file>' or '<file> in <Editor>'"
        ))),
    }
}

fn single_path(verb: &str, args: &[String]) -> Result<String, AstraError> {
    match args {
        [path] => Ok(path.clone()),
        [] => Err(missing(verb, "a path")),
        _ => Err(AstraError::AlmanacParse(format!(
            "{verb} expects exactly one path (Astra paths use '>', not spaces)"
        ))),
    }
}

fn two_paths(verb: &str, args: &[String]) -> Result<(String, String), AstraError> {
    match args {
        [from, to] => Ok((from.clone(), to.clone())),
        // A natural-language "to" separator is also accepted: `copy X to Y`.
        [from, separator, to] if separator.eq_ignore_ascii_case("to") => {
            Ok((from.clone(), to.clone()))
        }
        _ => Err(AstraError::AlmanacParse(format!(
            "{verb} expects '<from> <to>' or '<from> to <to>' (Astra paths use '>', not spaces)"
        ))),
    }
}

fn no_args(verb: &str, args: &[String]) -> Result<(), AstraError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(AstraError::AlmanacParse(format!(
            "{verb} does not take any arguments"
        )))
    }
}

fn missing(verb: &str, what: &str) -> AstraError {
    AstraError::AlmanacParse(format!("{verb} requires {what}"))
}

fn pid_arg(verb: &str, args: &[String]) -> Result<u32, AstraError> {
    match args {
        [pid] => pid.parse::<u32>().map_err(|_| {
            AstraError::AlmanacParse(format!("{verb} expects a numeric Astra PID, got '{pid}'"))
        }),
        _ => Err(missing(verb, "an Astra PID (see 'almanac process')")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<AlmanacCommand, AstraError> {
        parse_line(line)
    }

    #[test]
    fn parses_every_native_command() {
        use AlmanacCommand::*;
        assert_eq!(parse("almanac").unwrap(), Help);
        assert_eq!(
            parse("almanac open Documents>Projects").unwrap(),
            Open {
                path: "Documents>Projects".into(),
                editor: EditorTarget::None
            }
        );
        assert_eq!(
            parse("almanac open app.js in vsc").unwrap(),
            Open {
                path: "app.js".into(),
                editor: EditorTarget::App("vsc".into())
            }
        );
        assert_eq!(parse("almanac back").unwrap(), Back);
        assert_eq!(parse("almanac root").unwrap(), Root);
        assert_eq!(parse("almanac scan").unwrap(), Scan);
        assert_eq!(
            parse("almanac gen Projects>New").unwrap(),
            Gen {
                path: "Projects>New".into()
            }
        );
        assert_eq!(
            parse("almanac mgen Projects>(Frontend,Backend,Docs)").unwrap(),
            Mgen {
                expression: "Projects>(Frontend,Backend,Docs)".into()
            }
        );
        assert_eq!(
            parse("almanac write notes.txt").unwrap(),
            Write {
                path: "notes.txt".into(),
                editor: EditorTarget::None
            }
        );
        assert_eq!(
            parse("almanac write notes.txt in VSCode").unwrap(),
            Write {
                path: "notes.txt".into(),
                editor: EditorTarget::App("VSCode".into())
            }
        );
        assert_eq!(
            parse("almanac rewrite notes.txt in VSCode").unwrap(),
            Rewrite {
                path: "notes.txt".into(),
                editor: EditorTarget::App("VSCode".into())
            }
        );
        assert_eq!(
            parse("almanac destroy Projects>Old").unwrap(),
            Destroy {
                path: "Projects>Old".into()
            }
        );
        assert_eq!(
            parse("almanac rename Reports>Q1>draft.txt>final.txt").unwrap(),
            Rename {
                path: "Reports>Q1>draft.txt".into(),
                new_name: "final.txt".into()
            }
        );
        assert_eq!(
            parse("almanac transfer Projects>a Documents").unwrap(),
            Transfer {
                from: "Projects>a".into(),
                to: "Documents".into()
            }
        );
        assert_eq!(
            parse("almanac copy Projects>a Documents").unwrap(),
            Copy {
                from: "Projects>a".into(),
                to: "Documents".into()
            }
        );
        // The natural-language "to" separator is accepted for copy/transfer.
        assert_eq!(
            parse("almanac copy HOST>Desktop>uniq-app to Projects").unwrap(),
            Copy {
                from: "HOST>Desktop>uniq-app".into(),
                to: "Projects".into()
            }
        );
        assert_eq!(
            parse("almanac transfer Projects>a TO HOST>Dev").unwrap(),
            Transfer {
                from: "Projects>a".into(),
                to: "HOST>Dev".into()
            }
        );
        assert_eq!(
            parse("almanac lookout server.rs").unwrap(),
            Lookout {
                query: "server.rs".into()
            }
        );
        assert_eq!(
            parse("almanac inspect Projects").unwrap(),
            Inspect {
                path: "Projects".into()
            }
        );
        assert_eq!(
            parse("almanac lock Projects").unwrap(),
            Lock {
                path: "Projects".into()
            }
        );
        assert_eq!(
            parse("almanac unlock Projects").unwrap(),
            Unlock {
                path: "Projects".into()
            }
        );
        assert_eq!(
            parse("almanac run Notepad README.txt").unwrap(),
            Run {
                application: "Notepad".into(),
                args: vec!["README.txt".into()]
            }
        );
        assert_eq!(parse("almanac mount").unwrap(), Mount { path: None });
        assert_eq!(
            parse("almanac mount \"D:\\Dev Work\"").unwrap(),
            Mount {
                path: Some("D:\\Dev Work".into())
            }
        );
        assert_eq!(
            parse("almanac unmount Development").unwrap(),
            Unmount {
                alias: "Development".into()
            }
        );
        assert_eq!(parse("almanac mounts").unwrap(), Mounts);
        assert_eq!(parse("almanac logout").unwrap(), Logout);
        assert_eq!(parse("almanac kill lapsession").unwrap(), KillLapsession);
        assert_eq!(parse("almanac hibernate").unwrap(), Hibernate);
        assert_eq!(parse("almanac restart").unwrap(), Restart);
    }

    #[test]
    fn parses_the_scheduler_grammar() {
        use crate::kernel::SchedulerAlgorithm;
        assert_eq!(
            parse("almanac scheduler").unwrap(),
            AlmanacCommand::Scheduler
        );
        assert_eq!(
            parse("almanac scheduler change RR").unwrap(),
            AlmanacCommand::SchedulerChange {
                algorithm: SchedulerAlgorithm::RoundRobin
            }
        );
        assert_eq!(
            parse("almanac scheduler change fcfs").unwrap(),
            AlmanacCommand::SchedulerChange {
                algorithm: SchedulerAlgorithm::Fcfs
            }
        );
        assert_eq!(
            parse("almanac scheduler set Priority").unwrap(),
            AlmanacCommand::SchedulerChange {
                algorithm: SchedulerAlgorithm::Priority
            }
        );
        assert_eq!(
            parse("almanac scheduler tick").unwrap(),
            AlmanacCommand::SchedulerTick { ticks: 1 }
        );
        assert_eq!(
            parse("almanac scheduler tick 12").unwrap(),
            AlmanacCommand::SchedulerTick { ticks: 12 }
        );
        assert!(parse("almanac scheduler change nope").is_err());
        assert!(parse("almanac scheduler tick 0").is_err());
        assert!(parse("almanac scheduler wat").is_err());
    }

    #[test]
    fn parses_the_memory_grammar() {
        use crate::memory::ReplacementPolicy;
        assert_eq!(parse("almanac memory").unwrap(), AlmanacCommand::Memory);
        assert_eq!(parse("almanac mem").unwrap(), AlmanacCommand::Memory);
        assert_eq!(
            parse("almanac memory policy FIFO").unwrap(),
            AlmanacCommand::MemorySetPolicy {
                policy: ReplacementPolicy::Fifo
            }
        );
        assert_eq!(
            parse("almanac memory policy lru").unwrap(),
            AlmanacCommand::MemorySetPolicy {
                policy: ReplacementPolicy::Lru
            }
        );
        assert!(parse("almanac memory policy swap").is_err());
        assert!(parse("almanac memory wat").is_err());
    }

    #[test]
    fn rejects_invalid_arguments() {
        assert!(parse("almanac open").is_err());
        assert!(parse("almanac open a b").is_err());
        assert!(parse("almanac back now").is_err());
        assert!(parse("almanac rename noseparator").is_err());
        assert!(parse("almanac transfer only-one").is_err());
        assert!(parse("almanac kill everything").is_err());
        assert!(parse("almanac teleport somewhere").is_err());
        assert!(matches!(
            parse("almanac teleport x"),
            Err(AstraError::AlmanacParse(_))
        ));
    }

    #[test]
    fn open_argument_keeps_the_path_arrow_out_of_the_shell() {
        // The lexer must not split on '>', so this is a single-path Open,
        // never a redirection.
        assert_eq!(
            parse("almanac open Documents>Projects").unwrap(),
            AlmanacCommand::Open {
                path: "Documents>Projects".into(),
                editor: EditorTarget::None
            }
        );
    }
}
