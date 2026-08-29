import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AppIcon } from "../components";
import type { HostAppInfo } from "../types";

interface HostAppsProps {
  /** Run an Almanac command (opens the Almanac window and dispatches it). */
  onLaunch: (command: string) => void;
}

function readError(reason: unknown) {
  return typeof reason === "string"
    ? reason
    : reason instanceof Error
      ? reason.message
      : "The desktop bridge could not list host applications.";
}

/**
 * HostApps — the "Applications" window.
 *
 * Lists the real Windows applications Astra knows how to launch and shows which
 * are installed on this machine. Detection happens entirely in Rust
 * (`host_apps` command); launching goes through `almanac run <name>` so the
 * Almanac console stays the single launcher surface.
 */
export function HostApps({ onLaunch }: HostAppsProps) {
  const [apps, setApps] = useState<HostAppInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(() => {
    setLoading(true);
    setError(null);
    invoke<HostAppInfo[]>("host_apps")
      .then(setApps)
      .catch((reason: unknown) => setError(readError(reason)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const installed = apps.filter((app) => app.installed);
  const missing = apps.filter((app) => !app.installed);

  return (
    <div className="host-apps">
      <header className="host-apps-header">
        <div>
          <h2>Windows Apps</h2>
          <p>
            Real Windows apps Astra can launch. Detection is read-only; launching runs{" "}
            <code>almanac run &lt;name&gt;</code>.
          </p>
        </div>
        <button type="button" className="host-apps-refresh" onClick={refresh}>
          Rescan
        </button>
      </header>

      <main className="host-apps-main" aria-busy={loading}>
        {error && (
          <p className="host-apps-error" role="alert">
            {error}
          </p>
        )}

        {!error && (
          <>
            <section>
              <h3>
                Installed <span>{installed.length}</span>
              </h3>
              {installed.length === 0 ? (
                <p className="host-apps-empty">
                  {loading
                    ? "Scanning this machine…"
                    : "None of the known apps were detected here."}
                </p>
              ) : (
                <ul className="host-apps-grid">
                  {installed.map((app) => (
                    <li key={app.name} className="host-app-tile">
                      <span className="host-app-mark">
                        <AppIcon name="apps" />
                      </span>
                      <span className="host-app-name">{app.name}</span>
                      <span className="host-app-kind">
                        {app.store_app ? "Microsoft Store" : "Desktop app"}
                      </span>
                      <button
                        type="button"
                        className="host-app-launch"
                        onClick={() => onLaunch(`almanac run ${app.name}`)}
                      >
                        Launch
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            {missing.length > 0 && (
              <section>
                <h3>
                  Not detected <span>{missing.length}</span>
                </h3>
                <ul className="host-apps-grid muted">
                  {missing.map((app) => (
                    <li key={app.name} className="host-app-tile">
                      <span className="host-app-mark">
                        <AppIcon name="apps" />
                      </span>
                      <span className="host-app-name">{app.name}</span>
                      <span className="host-app-kind">Not installed</span>
                    </li>
                  ))}
                </ul>
              </section>
            )}
          </>
        )}
      </main>

      <footer className="host-apps-footer">
        <span className="resource-badge host">HOST</span>
        These processes run on Windows — Astra only tracks them.
      </footer>
    </div>
  );
}
