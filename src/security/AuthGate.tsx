import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import type { AuthenticationStatus } from "../types";

export interface SessionControls {
  authenticated: boolean;
  logout: () => Promise<void>;
  onAuthenticated: (status: AuthenticationStatus) => void;
}

interface AuthGateProps {
  children: (session: SessionControls) => ReactNode;
}

const EMPTY_STATUS: AuthenticationStatus = {
  configured: false,
  authenticated: false,
  failed_attempts: 0,
  remaining_attempts: 3,
  locked_out: false,
};

export function AuthGate({ children }: AuthGateProps) {
  const [status, setStatus] = useState<AuthenticationStatus | null>(null);
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const passwordRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    invoke<AuthenticationStatus>("auth_status")
      .then(setStatus)
      .catch((reason) => {
        setStatus(EMPTY_STATUS);
        setError(readError(reason));
      });
  }, []);
  useEffect(() => {
    if (status && !status.configured) passwordRef.current?.focus({ preventScroll: true });
  }, [status]);

  const submitSetup = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!status || busy || status.configured) return;
    if (password !== confirmation) {
      setError("The passwords do not match. Re-enter both fields.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const next = await invoke<AuthenticationStatus>("configure_login", { password });
      setStatus(next);
      setPassword("");
      setConfirmation("");
    } catch (reason) {
      setPassword("");
      setConfirmation("");
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  const logout = async () => {
    const next = await invoke<AuthenticationStatus>("logout");
    setStatus(next);
    setError("");
  };

  if (status?.configured) {
    return (
      <>
        {children({
          authenticated: status.authenticated,
          logout,
          onAuthenticated: setStatus,
        })}
      </>
    );
  }

  return (
    <main className="auth-screen">
      <section className="auth-panel" aria-labelledby="auth-title">
        <div className="auth-mark" aria-hidden="true">
          <span />
        </div>
        <div className="auth-copy">
          <h1 id="auth-title">Secure this Aaru-OS profile</h1>
          <p>
            Create the local password used to enter this profile. It is stored only as an
            Argon2 hash.
          </p>
        </div>
        {status === null ? (
          <div className="auth-loading" role="status">
            Loading security state…
          </div>
        ) : (
          <form className="auth-form" onSubmit={submitSetup} aria-busy={busy}>
            <label>
              <span>New password</span>
              <input
                ref={passwordRef}
                type="password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                autoComplete="new-password"
                minLength={8}
                disabled={busy}
                required
              />
            </label>
            <label>
              <span>Confirm password</span>
              <input
                type="password"
                value={confirmation}
                onChange={(event) => setConfirmation(event.target.value)}
                autoComplete="new-password"
                minLength={8}
                disabled={busy}
                required
              />
            </label>
            {error && (
              <p className="auth-error" role="alert">
                {error}
              </p>
            )}
            <button type="submit" disabled={busy}>
              {busy ? "Securing profile…" : "Create profile password"}
            </button>
          </form>
        )}
        <p className="auth-footnote">
          Passwords are never accepted as Almanac command arguments.
        </p>
      </section>
    </main>
  );
}

function readError(reason: unknown) {
  const message =
    typeof reason === "string"
      ? reason
      : reason instanceof Error
        ? reason.message
        : "The Rust backend could not complete this request.";
  return message.includes("invoke")
    ? "Security setup is available in the Tauri app. Run npm run tauri dev."
    : message;
}
