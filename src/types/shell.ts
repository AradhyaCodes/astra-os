/**
 * Almanac command-engine types. These mirror the Rust structs returned by the
 * `almanac_*` Tauri commands.
 */

export type StatusTag =
  "OK" | "INFO" | "ERROR" | "DENIED" | "LOCKED" | "AUTH" | "PROCESS" | "SYSTEM";

export interface OutputLine {
  tag: StatusTag;
  text: string;
}

export interface PromptRequest {
  id: string;
  /** e.g. "destroy_confirm", "lock_password", "lock_confirm", "unlock_password", "logout_password" */
  kind: string;
  message: string;
  /** Masked inputs (passwords, confirmations) must never be echoed or stored in history. */
  masked: boolean;
}

export interface AppLaunch {
  app: string;
  path: string | null;
  args: string[];
}

export type SystemAction =
  | { kind: "shutdown" }
  | { kind: "restart" }
  | { kind: "hibernate" }
  | { kind: "logged_out" };

export interface ProcessView {
  id: string;
  program: string;
}

export interface AlmanacOutcome {
  lines: OutputLine[];
  new_cwd: string | null;
  clear: boolean;
  prompt: PromptRequest | null;
  process: ProcessView | null;
  launch: AppLaunch | null;
  system_action: SystemAction | null;
  /** UI should open the native folder picker, then re-run `almanac mount "<path>"`. */
  request_mount: boolean;
  /** AppId of a Tauri window the UI should open (built-in app launch). */
  open_window: string | null;
  /** Title for the opened window when it is the generic app shell. */
  open_window_title: string | null;
}

/** A host mount as shown by `almanac mounts` / the mount picker flow. */
export interface MountView {
  alias: string;
  source: string;
  is_default: boolean;
  available: boolean;
}

/** A real Windows application Aaru can launch, from the `host_apps` command. */
export interface HostAppInfo {
  /** Alias / display name — also the `almanac run <name>` argument. */
  name: string;
  /** Resolved to a real executable or an installed Store package. */
  installed: boolean;
  /** Installed as a Microsoft Store / MSIX package rather than a plain exe. */
  store_app: boolean;
}

export interface CompletionResult {
  replacement: string | null;
  candidates: string[];
  locked: boolean;
}

/** Streamed host-process event on channel `almanac://proc/<id>`. */
export type StreamEvent =
  | { type: "started"; pid: number }
  | { type: "stdout"; line: string }
  | { type: "stderr"; line: string }
  | { type: "exit"; code: number | null; success: boolean }
  | { type: "error"; message: string; not_found: boolean };
