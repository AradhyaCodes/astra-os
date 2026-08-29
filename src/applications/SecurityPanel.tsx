import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import type {
  Permissions,
  ResourceAuthenticationStatus,
  ResourceInfo,
  ResourceSecurityInfo,
} from "../types";

const DEFAULT_PERMISSIONS: Permissions = {
  read: true,
  write: true,
  execute: true,
};

export function SecurityPanel() {
  const [path, setPath] = useState("ROOT>Projects");
  const [password, setPassword] = useState("");
  const [resource, setResource] = useState<ResourceInfo | null>(null);
  const [permissions, setPermissions] = useState(DEFAULT_PERMISSIONS);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [activeAction, setActiveAction] = useState<
    "inspect" | "lock" | "authenticate" | "unlock" | "permissions" | null
  >(null);

  const inspect = async () => {
    setBusy(true);
    setActiveAction("inspect");
    setError("");
    try {
      const result = await invoke<ResourceSecurityInfo>("fs_security_info", {
        cwd: "ROOT",
        path,
      });
      applyResource(result.resource);
      setNotice(
        result.pending_lock_boundaries.length === 0
          ? "Resource state loaded from Rust."
          : `${result.pending_lock_boundaries.length} lock boundary remains.`,
      );
    } catch (reason) {
      setResource(null);
      setNotice("");
      setError(readError(reason));
    } finally {
      setBusy(false);
      setActiveAction(null);
    }
  };

  const submitPasswordAction = async (action: "lock" | "authenticate" | "unlock") => {
    if (!password || busy) return;
    setBusy(true);
    setActiveAction(action);
    setError("");
    setNotice("");
    try {
      if (action === "authenticate") {
        const result = await invoke<ResourceAuthenticationStatus>(
          "fs_authenticate_resource",
          { cwd: "ROOT", path, password },
        );
        setNotice(
          result.remaining_boundaries === 0
            ? "Every lock boundary for this path is authenticated."
            : `Boundary accepted. ${result.remaining_boundaries} more must be authenticated.`,
        );
        if (result.remaining_boundaries === 0) {
          const current = await invoke<ResourceSecurityInfo>("fs_security_info", {
            cwd: "ROOT",
            path,
          });
          applyResource(current.resource);
        }
      } else {
        const result = await invoke<ResourceSecurityInfo>(
          action === "lock" ? "fs_lock" : "fs_unlock",
          { cwd: "ROOT", path, password },
        );
        applyResource(result.resource);
        setNotice(
          action === "lock"
            ? "Lock created. This path now requires its resource password."
            : "Lock removed. Permissions were not changed.",
        );
      }
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setPassword("");
      setBusy(false);
      setActiveAction(null);
    }
  };

  const savePermissions = async () => {
    setBusy(true);
    setActiveAction("permissions");
    setError("");
    try {
      const result = await invoke<ResourceInfo>("fs_set_permissions", {
        cwd: "ROOT",
        path,
        permissions,
      });
      applyResource(result);
      setNotice("READ, WRITE, and EXECUTE were saved independently of the lock.");
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
      setActiveAction(null);
    }
  };

  const applyResource = (next: ResourceInfo) => {
    setResource(next);
    setPermissions(next.metadata.permissions);
  };

  return (
    <div className="security-panel" aria-busy={busy}>
      <header className="security-panel-header">
        <div>
          <h2>Resource security test panel</h2>
          <p>
            Temporary Phase 2 controls. All decisions and password checks run in Rust.
          </p>
        </div>
        <span className="phase-badge">Phase 2</span>
      </header>

      <section className="security-target" aria-labelledby="security-target-title">
        <h3 id="security-target-title">Target resource</h3>
        <div className="security-path-row">
          <label>
            <span>Aaru path</span>
            <input
              value={path}
              onChange={(event) => {
                setPath(event.target.value);
                setResource(null);
                setPermissions(DEFAULT_PERMISSIONS);
                setNotice("");
              }}
              spellCheck={false}
              placeholder="ROOT>Projects"
            />
          </label>
          <button type="button" onClick={inspect} disabled={busy || !path}>
            {activeAction === "inspect" ? "Loading…" : "Load state"}
          </button>
        </div>
      </section>

      {resource && (
        <div className="security-state-strip" aria-live="polite">
          <span>
            <strong>{resource.metadata.resource_type}</strong>
            {resource.path}
          </span>
          <span className={resource.metadata.locked ? "state-locked" : "state-open"}>
            {resource.metadata.locked ? "Locked" : "No lock"}
          </span>
        </div>
      )}

      <form
        className="security-lock-controls"
        onSubmit={(event) => {
          event.preventDefault();
          void submitPasswordAction("authenticate");
        }}
      >
        <div className="security-section-copy">
          <h3>Resource password</h3>
          <p>
            Authentication grants access for this process only. Removing a lock is a
            separate, persistent action.
          </p>
        </div>
        <label>
          <span>Password</span>
          <input
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            autoComplete="off"
            minLength={8}
            disabled={busy}
            required
          />
        </label>
        <div className="security-action-row">
          <button
            type="button"
            onClick={() => void submitPasswordAction("lock")}
            disabled={busy || !password}
          >
            {activeAction === "lock" ? "Creating…" : "Create lock"}
          </button>
          <button
            type="button"
            onClick={() => void submitPasswordAction("authenticate")}
            disabled={busy || !password}
          >
            {activeAction === "authenticate"
              ? "Authenticating…"
              : "Authenticate boundary"}
          </button>
          <button
            className="secondary-action"
            type="button"
            onClick={() => void submitPasswordAction("unlock")}
            disabled={busy || !password}
          >
            {activeAction === "unlock" ? "Removing…" : "Remove lock"}
          </button>
        </div>
      </form>

      <section className="security-permissions" aria-labelledby="permissions-title">
        <div className="security-section-copy">
          <h3 id="permissions-title">Permissions</h3>
          <p>Permission flags remain independent when a lock is created or removed.</p>
        </div>
        <div className="permission-options">
          {(["read", "write", "execute"] as const).map((permission) => (
            <label key={permission}>
              <input
                type="checkbox"
                checked={permissions[permission]}
                disabled={busy || resource === null}
                onChange={(event) =>
                  setPermissions((current) => ({
                    ...current,
                    [permission]: event.target.checked,
                  }))
                }
              />
              <span>{permission.toUpperCase()}</span>
            </label>
          ))}
        </div>
        <button
          type="button"
          onClick={savePermissions}
          disabled={busy || !path || resource === null}
        >
          {activeAction === "permissions" ? "Saving…" : "Save permissions"}
        </button>
      </section>

      {(notice || error) && (
        <p
          className={error ? "security-message error" : "security-message"}
          role={error ? "alert" : "status"}
        >
          {error || notice}
        </p>
      )}
    </div>
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
    ? "This control requires the Tauri runtime. Run npm run tauri dev."
    : message;
}
