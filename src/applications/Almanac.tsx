import { useState, useRef, useEffect, type KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useWindowStore, type AppId } from "../stores";
import type {
  AlmanacOutcome,
  AuthenticationStatus,
  CompletionResult,
  OutputLine,
  PromptRequest,
  StatusTag,
  StreamEvent,
  ResumeSession,
} from "../types";

const WINDOW_SIZES: Record<string, [number, number]> = {
  taskmanager: [920, 640],
  "app-shell": [420, 380],
  almanac: [900, 620],
  terminal: [760, 500],
  settings: [780, 560],
  calculator: [360, 530],
  texteditor: [760, 560],
  imageviewer: [780, 560],
  "system-info": [700, 600],
  security: [720, 620],
};

interface HistoryEntry {
  kind: "input" | "line";
  tag?: StatusTag;
  content: string;
}

interface AlmanacProps {
  authenticated: boolean;
  acceptExternalCommands?: boolean;
  onAuthenticated: (status: AuthenticationStatus) => void;
  onLogout: () => Promise<void>;
  resumeSession: ResumeSession | null;
}

const MAX_MASK_DOTS = 10;
const MAX_SCROLLBACK = 2_000;

export function Almanac({
  authenticated,
  acceptExternalCommands = false,
  onAuthenticated,
  onLogout,
  resumeSession,
}: AlmanacProps) {
  const restoredAlmanac = resumeSession?.almanac_session;
  const [history, setHistory] = useState<HistoryEntry[]>(() =>
    validHistory(restoredAlmanac?.history),
  );
  const [cwd, setCwd] = useState(
    () => restoredAlmanac?.cwd ?? resumeSession?.cwd ?? "ROOT",
  );
  const [input, setInput] = useState("");
  const [cmdHistory, setCmdHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [isExecuting, setIsExecuting] = useState(false);
  const [prompt, setPrompt] = useState<PromptRequest | null>(null);
  const [halted, setHalted] = useState(false);
  const [authRequired, setAuthRequired] = useState(!authenticated);

  const openWindow = useWindowStore((state) => state.openWindow);

  const historyRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const unlistenersRef = useRef<UnlistenFn[]>([]);

  useEffect(() => {
    invoke<string[]>("almanac_history")
      .then(setCmdHistory)
      .catch(() => {
        /* history is best-effort; a fresh session simply starts empty */
      });
  }, []);

  useEffect(() => {
    setAuthRequired(!authenticated);
    if (!authenticated) {
      setPrompt(null);
      setHalted(false);
    }
  }, [authenticated]);

  useEffect(() => {
    const scrollback = historyRef.current;
    if (scrollback) scrollback.scrollTop = scrollback.scrollHeight;
  }, [history]);

  useEffect(() => {
    if (!isExecuting && !halted) inputRef.current?.focus({ preventScroll: true });
  }, [authRequired, isExecuting, halted, prompt]);

  useEffect(() => {
    const unlisteners = unlistenersRef.current;
    return () => {
      unlisteners.forEach((off) => off());
      unlisteners.length = 0;
    };
  }, []);

  const pushLine = (tag: StatusTag, content: string) =>
    setHistory((prev) =>
      [...prev, { kind: "line" as const, tag, content }].slice(-MAX_SCROLLBACK),
    );

  const pushLines = (lines: OutputLine[]) =>
    setHistory((prev) =>
      [
        ...prev,
        ...lines.map((line) => ({
          kind: "line" as const,
          tag: line.tag,
          content: line.text,
        })),
      ].slice(-MAX_SCROLLBACK),
    );

  const subscribeProcess = (id: string, program: string) => {
    const channel = `almanac://proc/${id}`;
    listen<StreamEvent>(channel, (event) => {
      const payload = event.payload;
      switch (payload.type) {
        case "started":
          pushLine("PROCESS", `${program} running — host PID ${payload.pid}`);
          break;
        case "stdout":
          pushLine("PROCESS", payload.line);
          break;
        case "stderr":
          pushLine("ERROR", payload.line);
          break;
        case "exit":
          pushLine(
            "PROCESS",
            `${program} exited — code ${payload.code ?? "?"} (${
              payload.success ? "success" : "failure"
            })`,
          );
          break;
        case "error":
          pushLine(
            "ERROR",
            payload.not_found
              ? `command not found: ${payload.message}`
              : `process error: ${payload.message}`,
          );
          break;
      }
    })
      .then((off) => unlistenersRef.current.push(off))
      .catch(() => {
        pushLine("ERROR", "unable to stream host process output in this environment");
      });
  };

  const applyOutcome = (outcome: AlmanacOutcome) => {
    if (outcome.clear) {
      setHistory([]);
    } else if (outcome.lines.length > 0) {
      pushLines(outcome.lines);
    }

    if (outcome.new_cwd) setCwd(outcome.new_cwd);
    setPrompt(outcome.prompt);

    if (outcome.process) subscribeProcess(outcome.process.id, outcome.process.program);

    if (outcome.request_mount) void pickAndMount();

    if (outcome.open_window) {
      const appId = outcome.open_window as AppId;
      const [width, height] = WINDOW_SIZES[outcome.open_window] ?? [480, 420];
      openWindow(appId, outcome.open_window_title ?? outcome.open_window, width, height);
    }

    if (outcome.launch && outcome.launch.app !== "$default") {
      const target = outcome.launch.path ?? outcome.launch.args.join(" ");
      pushLine("INFO", `launch → ${outcome.launch.app}${target ? ` (${target})` : ""}`);
    }

    if (outcome.system_action) {
      switch (outcome.system_action.kind) {
        case "logged_out":
          setAuthRequired(true);
          void onLogout();
          break;
        case "shutdown":
          localStorage.removeItem("astra-hibernate-ui");
          setHalted(true);
          pushLine("SYSTEM", "Astra OS is shutting down…");
          break;
        case "restart":
          localStorage.removeItem("astra-hibernate-ui");
          setHalted(true);
          pushLine("SYSTEM", "Astra OS is restarting…");
          break;
        case "hibernate":
          void persistHibernateSession();
          break;
      }
    }
  };

  const pickAndMount = async () => {
    setIsExecuting(true);
    try {
      const picked = await invoke<string | null>("host_pick_directory");
      if (!picked) {
        pushLine("INFO", "mount cancelled");
        return;
      }
      // The backend canonicalises + containment-checks this path before it
      // becomes a mount.
      const outcome = await invoke<AlmanacOutcome>("almanac_eval", {
        cwd,
        line: `almanac mount "${picked}"`,
      });
      applyOutcome(outcome);
    } catch (error) {
      pushLine("ERROR", readError(error));
    } finally {
      setIsExecuting(false);
    }
  };

  const runLine = async (raw: string) => {
    const line = raw.trim();
    if (!line || isExecuting || halted || authRequired) return;

    setHistory((prev) => [...prev, { kind: "input", content: `${cwd} > ${line}` }]);
    setCmdHistory((prev) => (prev[prev.length - 1] === line ? prev : [...prev, line]));
    setHistoryIndex(-1);
    setIsExecuting(true);
    try {
      const outcome = await invoke<AlmanacOutcome>("almanac_eval", { cwd, line });
      applyOutcome(outcome);
    } catch (error) {
      pushLine("ERROR", readError(error));
    } finally {
      setIsExecuting(false);
    }
  };

  const submitLogin = async (value: string) => {
    if (!value || isExecuting) return;
    setHistory((previous) => [
      ...previous,
      { kind: "input", content: "Password: ••••••••" },
    ]);
    setIsExecuting(true);
    try {
      const next = await invoke<AuthenticationStatus>("login", { password: value });
      onAuthenticated(next);
      setAuthRequired(false);
      pushLine("AUTH", "authentication accepted — LapSession unlocked");
    } catch (error) {
      pushLine("DENIED", readError(error));
    } finally {
      setIsExecuting(false);
    }
  };

  const persistHibernateSession = async () => {
    const windowState = useWindowStore.getState();
    try {
      await invoke("lifecycle_hibernate", {
        cwd,
        uiSession: {
          windows: windowState.windows,
          activeWindowId: windowState.activeWindowId,
        },
        almanacSession: { cwd, history },
      });
      localStorage.setItem("astra-hibernate-ui", "saved");
      pushLine("SYSTEM", "hibernate image committed — click the sleep screen to resume");
      window.dispatchEvent(new Event("astra:hibernate"));
    } catch (error) {
      pushLine("ERROR", `hibernate failed: ${readError(error)}`);
    }
  };

  const submitPrompt = async (value: string) => {
    const active = prompt;
    if (!active || isExecuting) return;

    const echo = active.masked
      ? `${active.message} ${"•".repeat(Math.min(value.length, MAX_MASK_DOTS))}`
      : `${active.message} ${value}`;
    setHistory((prev) => [...prev, { kind: "input", content: echo }]);
    // Prompt responses (passwords, confirmations) never enter command history.

    setIsExecuting(true);
    try {
      const outcome = await invoke<AlmanacOutcome>("almanac_respond", {
        response: value,
      });
      applyOutcome(outcome);
    } catch (error) {
      pushLine("ERROR", readError(error));
      setPrompt(null);
    } finally {
      setIsExecuting(false);
    }
  };

  const cancelPrompt = async () => {
    setPrompt(null);
    try {
      const outcome = await invoke<AlmanacOutcome>("almanac_cancel_prompt");
      applyOutcome(outcome);
    } catch {
      /* nothing to cancel in a non-Tauri context */
    }
  };

  const runCompletion = async () => {
    if (authRequired || prompt || !input.trim()) return;
    try {
      const result = await invoke<CompletionResult>("almanac_complete", {
        cwd,
        line: input,
      });
      if (result.locked) {
        pushLine("LOCKED", "completion hidden — that directory is locked");
        return;
      }
      if (result.replacement) {
        setInput(replaceLastToken(input, result.replacement));
        return;
      }
      if (result.candidates.length > 0) {
        pushLine("INFO", result.candidates.join("   "));
      }
    } catch {
      /* completion is unavailable outside the Tauri runtime */
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key.toLowerCase() === "c" && event.ctrlKey) {
      if (prompt) {
        event.preventDefault();
        setInput("");
        void cancelPrompt();
      }
      return;
    }
    if (event.key === "Enter") {
      const value = input;
      setInput("");
      if (authRequired) void submitLogin(value);
      else if (prompt) void submitPrompt(value);
      else void runLine(value);
      return;
    }
    if (event.key === "Escape" && prompt) {
      event.preventDefault();
      setInput("");
      void cancelPrompt();
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault();
      void runCompletion();
      return;
    }
    if (authRequired || prompt) return;

    if (event.key === "ArrowUp") {
      event.preventDefault();
      if (cmdHistory.length === 0) return;
      const nextIndex =
        historyIndex + 1 < cmdHistory.length ? historyIndex + 1 : historyIndex;
      if (nextIndex >= 0) {
        setHistoryIndex(nextIndex);
        setInput(cmdHistory[cmdHistory.length - 1 - nextIndex]);
      }
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      if (historyIndex > 0) {
        const nextIndex = historyIndex - 1;
        setHistoryIndex(nextIndex);
        setInput(cmdHistory[cmdHistory.length - 1 - nextIndex]);
      } else if (historyIndex === 0) {
        setHistoryIndex(-1);
        setInput("");
      }
    } else if (event.key === "l" && event.ctrlKey) {
      event.preventDefault();
      setHistory([]);
    }
  };

  useEffect(() => {
    if (!acceptExternalCommands) return;
    const runExternal = (event: Event) => {
      const command = (event as CustomEvent<string>).detail;
      if (command) void runLine(command);
    };
    window.addEventListener("astra:command", runExternal);
    return () => window.removeEventListener("astra:command", runExternal);
  }, [acceptExternalCommands, runLine]);

  return (
    <div
      className="almanac-container"
      onClick={() => inputRef.current?.focus()}
      aria-busy={isExecuting}
    >
      <header className="almanac-header">
        <span className="almanac-led" />
        <strong>ALMANAC CONSOLE</strong>
        <span>{cwd.startsWith("HOST") ? "HOST BRIDGE" : "VIRTUAL CORE"}</span>
        <small>
          {history.length} / {MAX_SCROLLBACK} lines
        </small>
      </header>
      <div
        ref={historyRef}
        className="almanac-history"
        role="log"
        aria-live="polite"
        aria-relevant="additions"
      >
        {history.length === 0 && (
          <div className="almanac-empty">
            {authRequired
              ? "LapSession locked. Enter the login password below; plaintext is never echoed or stored."
              : "Astra command interface ready. Type “almanac” for native commands; other input falls back to the host shell."}
          </div>
        )}
        {history.map((entry, index) =>
          entry.kind === "input" ? (
            <div key={index} className="almanac-entry input">
              {entry.content}
            </div>
          ) : (
            <div key={index} className={`almanac-entry line tag-${entry.tag}`}>
              <span className="almanac-tag">[{entry.tag}]</span> {entry.content}
            </div>
          ),
        )}
      </div>
      <div className="almanac-input-row">
        {prompt?.kind === "destroy_host_confirm" && (
          <span className="almanac-host-warning">PHYSICAL FILE OPERATION</span>
        )}
        <span className="almanac-prompt">
          {authRequired ? "Password:" : prompt ? prompt.message : formatPrompt(cwd)}
        </span>
        <input
          ref={inputRef}
          type={authRequired || prompt?.masked ? "password" : "text"}
          value={input}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={handleKeyDown}
          className="almanac-input"
          autoComplete="off"
          spellCheck="false"
          aria-label={
            authRequired ? "Password" : prompt ? prompt.message : "Almanac command"
          }
          disabled={isExecuting || halted}
        />
      </div>
    </div>
  );
}

function formatPrompt(cwd: string): string {
  if (cwd.startsWith("HOST")) {
    const tail = cwd.slice(4).replace(/^>/, "");
    return `ASTRA::HOST>${tail ? `${tail}>` : ""}`;
  }
  return `ASTRA::${cwd}>`;
}

function validHistory(value: unknown): HistoryEntry[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter(
      (entry): entry is HistoryEntry =>
        typeof entry === "object" &&
        entry !== null &&
        ((entry as HistoryEntry).kind === "input" ||
          (entry as HistoryEntry).kind === "line") &&
        typeof (entry as HistoryEntry).content === "string",
    )
    .slice(-MAX_SCROLLBACK);
}

function replaceLastToken(line: string, replacement: string): string {
  const trimmedEnd = line.replace(/\s+$/, "");
  const lastSpace = trimmedEnd.lastIndexOf(" ");
  if (lastSpace === -1) return replacement;
  return `${trimmedEnd.slice(0, lastSpace + 1)}${replacement}`;
}

function readError(reason: unknown): string {
  const message =
    typeof reason === "string"
      ? reason
      : reason instanceof Error
        ? reason.message
        : "The Rust backend could not complete this request.";
  return message.includes("invoke")
    ? "The Almanac engine requires the Tauri runtime. Run npm run tauri dev."
    : message;
}
