import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SystemConfig } from "../types";

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

interface StatCardProps {
  label: string;
  value: string | number;
  unit?: string;
  icon: StatIconName;
}

type StatIconName = "cpu" | "memory" | "disk" | "directory";

function StatIcon({ name }: { name: StatIconName }) {
  const commonProps = {
    "aria-hidden": true,
    className: "stat-icon",
    fill: "none",
    focusable: false,
    stroke: "currentColor",
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    strokeWidth: 1.7,
    viewBox: "0 0 24 24",
  };

  switch (name) {
    case "cpu":
      return (
        <svg {...commonProps}>
          <rect x="7" y="7" width="10" height="10" rx="2" />
          <path d="M9 2v3m6-3v3M9 19v3m6-3v3M2 9h3m-3 6h3m14-6h3m-3 6h3" />
          <path d="M10 10h4v4h-4z" />
        </svg>
      );
    case "memory":
      return (
        <svg {...commonProps}>
          <path d="M5 7h14v10H5zM8 10v4m4-4v4m4-4v4M7 17v3m5-3v3m5-3v3" />
          <path d="M5 5v2m14-2v2" />
        </svg>
      );
    case "disk":
      return (
        <svg {...commonProps}>
          <ellipse cx="12" cy="6" rx="7" ry="3" />
          <path d="M5 6v6c0 1.7 3.1 3 7 3s7-1.3 7-3V6M5 12v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6" />
        </svg>
      );
    case "directory":
      return (
        <svg {...commonProps}>
          <path d="M3.5 7.5h6l2-2h9v13h-17z" />
          <path d="M8 11h8m-8 3h5" />
        </svg>
      );
  }
}

function StatCard({ label, value, unit, icon }: StatCardProps) {
  return (
    <div className="stat-card">
      <div className="stat-icon-shell">
        <StatIcon name={icon} />
      </div>
      <div className="stat-body">
        <p className="stat-label">{label}</p>
        <p className="stat-value">
          {value}
          {unit && <span className="stat-unit"> {unit}</span>}
        </p>
      </div>
    </div>
  );
}

interface ChipListProps {
  label: string;
  items: string[];
}

function ChipList({ label, items }: ChipListProps) {
  return (
    <div className="chip-section">
      <p className="chip-label">{label}</p>
      <div className="chip-row">
        {items.length > 0 ? (
          items.map((item) => (
            <span key={item} className="chip">
              {item}
            </span>
          ))
        ) : (
          <span className="empty-value">None configured</span>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Loading skeleton
// ---------------------------------------------------------------------------

function LoadingSkeleton() {
  return (
    <div className="loading-state" role="status" aria-live="polite">
      <span className="sr-only">Loading system configuration</span>
      <div className="skeleton-grid" aria-hidden="true">
        {Array.from({ length: 4 }).map((_, i) => (
          <div key={i} className="skeleton-card" />
        ))}
      </div>
    </div>
  );
}

function getErrorMessage(error: unknown) {
  if (error instanceof Error) {
    return error.message;
  }

  if (typeof error === "string") {
    return error;
  }

  return "The desktop bridge returned an unknown error.";
}

// ---------------------------------------------------------------------------
// Main DevInfoScreen component
// ---------------------------------------------------------------------------

/**
 * SystemInfo — Phase 1 Application.
 *
 * Calls the `get_system_config` Tauri command on mount and displays the
 * returned KernelConfig and policy data.
 */
export function SystemInfo() {
  const [config, setConfig] = useState<SystemConfig | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [requestVersion, setRequestVersion] = useState(0);

  useEffect(() => {
    let isCurrent = true;

    setConfig(null);
    setError(null);
    setLoading(true);

    invoke<SystemConfig>("get_system_config")
      .then((cfg) => {
        if (isCurrent) {
          setConfig(cfg);
        }
      })
      .catch((err: unknown) => {
        if (isCurrent) {
          setError(getErrorMessage(err));
        }
      })
      .finally(() => {
        if (isCurrent) {
          setLoading(false);
        }
      });

    return () => {
      isCurrent = false;
    };
  }, [requestVersion]);

  function retry() {
    setRequestVersion((version) => version + 1);
  }

  return (
    <div className="dev-screen">
      {/* Header */}
      <header className="dev-header">
        <div className="dev-header-left">
          <div className="os-badge" aria-label="Astra OS">
            <span className="os-mark" aria-hidden="true" />
            ASTRA-OS
          </div>
          <div>
            <h1 className="dev-title">System Configuration</h1>
            <p className="dev-subtitle">Phase 1 — Kernel and policy overview</p>
          </div>
        </div>
        {config && <div className="version-badge">v{config.version}</div>}
      </header>

      {/* Content */}
      <main className="dev-main" aria-busy={loading}>
        {loading && <LoadingSkeleton />}

        {error && (
          <div className="error-panel" role="alert">
            <svg
              aria-hidden="true"
              className="error-icon"
              fill="none"
              focusable="false"
              stroke="currentColor"
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth="1.8"
              viewBox="0 0 24 24"
            >
              <path d="M12 3 2.8 20h18.4z" />
              <path d="M12 9v5m0 3h.01" />
            </svg>
            <div className="error-copy">
              <p className="error-title">Failed to load system configuration</p>
              <p className="error-guidance">
                Start the app with <code>npm run tauri dev</code>, then try the connection
                again.
              </p>
              <details className="error-details">
                <summary>Technical details</summary>
                <p className="error-detail">{error}</p>
              </details>
              <button className="retry-button" type="button" onClick={retry}>
                Try again
              </button>
            </div>
          </div>
        )}

        {config && (
          <>
            {/* Kernel hardware stats */}
            <section className="section">
              <h2 className="section-title">Virtual Hardware</h2>
              <div className="stats-grid">
                <StatCard icon="cpu" label="CPU Cores" value={config.kernel.cpu_cores} />
                <StatCard
                  icon="memory"
                  label="RAM"
                  value={config.kernel.ram_mb}
                  unit="MB"
                />
                <StatCard
                  icon="disk"
                  label="Disk"
                  value={config.kernel.disk_mb}
                  unit="MB"
                />
                <StatCard
                  icon="directory"
                  label="Max Directory Depth"
                  value={config.kernel.max_filesystem_depth}
                />
              </div>
            </section>

            {/* Schedulers & Memory policies */}
            <section className="section">
              <h2 className="section-title">Policies</h2>
              <div className="policy-grid">
                <div className="policy-card">
                  <ChipList label="Schedulers" items={config.supported_schedulers} />
                </div>
                <div className="policy-card">
                  <ChipList label="Memory Policies" items={config.memory_policies} />
                </div>
              </div>
            </section>

            {/* Filesystem rules */}
            <section className="section">
              <h2 className="section-title">Filesystem Rules</h2>
              <div className="rule-grid">
                <RuleRow
                  label="Case-sensitive names"
                  value={config.filesystem.case_sensitive ? "Enabled" : "Disabled"}
                  state={config.filesystem.case_sensitive ? "enabled" : "disabled"}
                />
                <RuleRow
                  label="Spaces allowed in names"
                  value={
                    config.filesystem.allow_spaces_in_names ? "Allowed" : "Not allowed"
                  }
                  state={config.filesystem.allow_spaces_in_names ? "enabled" : "disabled"}
                />
                <RuleRow
                  label="Files require extensions"
                  value={
                    config.filesystem.files_require_extensions ? "Required" : "Optional"
                  }
                  state={
                    config.filesystem.files_require_extensions ? "enabled" : "disabled"
                  }
                />
                <RuleRow
                  label="Maximum directory depth"
                  value={`${config.filesystem.max_depth} levels`}
                  state="neutral"
                />
              </div>
            </section>

            {/* Security */}
            <section className="section">
              <h2 className="section-title">Security</h2>
              <div className="rule-grid">
                <RuleRow
                  label="Single-user mode"
                  value={config.security.single_user ? "Enabled" : "Disabled"}
                  state={config.security.single_user ? "enabled" : "disabled"}
                />
                <RuleRow
                  label="Lockout threshold"
                  value={`${config.security.max_failed_attempts} failed attempts`}
                  state="neutral"
                />
              </div>
            </section>

            {/* IPC success banner */}
            <div className="success-banner">
              <svg
                aria-hidden="true"
                className="success-icon"
                fill="none"
                focusable="false"
                stroke="currentColor"
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth="2"
                viewBox="0 0 24 24"
              >
                <path d="m5 12 4 4L19 6" />
              </svg>
              <span>
                Tauri IPC bridge is operational — React successfully received
                configuration from Rust.
              </span>
            </div>
          </>
        )}
      </main>

      {/* Footer */}
      <footer className="dev-footer">
        Phase 1 · Tauri 2 · React 19 · TypeScript · Rust
      </footer>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helper: rule row
// ---------------------------------------------------------------------------

interface RuleRowProps {
  label: string;
  value: string;
  state: "enabled" | "disabled" | "neutral";
}

function RuleRow({ label, value, state }: RuleRowProps) {
  return (
    <div className="rule-row">
      <span className="rule-label">
        <span className="status-dot" data-state={state} aria-hidden="true" />
        {label}
      </span>
      <span className="rule-value">{value}</span>
    </div>
  );
}
