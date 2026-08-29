import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, type ReactNode } from "react";
import type { BootCheck, BootReport } from "../types";

interface BootScreenProps {
  children: (report: BootReport) => ReactNode;
}

const FALLBACK_CHECK: BootCheck = {
  name: "Aaru system core",
  detail: "Tauri backend did not respond",
  ok: false,
};

export function BootScreen({ children }: BootScreenProps) {
  const [report, setReport] = useState<BootReport | null>(null);
  const [visibleCount, setVisibleCount] = useState(0);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const started = performance.now();
    invoke<BootReport>("boot_status")
      .catch(() => ({
        version: "0.1",
        checks: [FALLBACK_CHECK],
        resumed: false,
        resume_session: null,
      }))
      .then((next) => {
        if (cancelled) return;
        setReport(next);
        const minimum = Math.max(0, 900 - (performance.now() - started));
        window.setTimeout(() => !cancelled && setReady(true), minimum);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!report || visibleCount >= report.checks.length) return;
    const timer = window.setTimeout(() => setVisibleCount((count) => count + 1), 72);
    return () => window.clearTimeout(timer);
  }, [report, visibleCount]);

  if (report && ready && visibleCount >= report.checks.length)
    return <>{children(report)}</>;

  return (
    <main className="boot-screen" aria-live="polite">
      <div className="boot-terminal">
        <pre
          className="boot-mark"
          aria-label="Aaru OS"
        >{`############################################
#                                          #
#              A  A  R  U                  #
#                 O  S                     #
#                                          #
############################################`}</pre>
        <div className="boot-version">AARU SYSTEM CORE v{report?.version ?? "0.1"}</div>
        <div className="boot-divider" />
        <div className="boot-checks">
          {!report && (
            <p>
              <span className="boot-pending">[..]</span> Establishing kernel IPC
            </p>
          )}
          {report?.checks.slice(0, visibleCount).map((check) => (
            <p key={check.name} className={check.ok ? "ok" : "fail"}>
              <span>[{check.ok ? "OK" : "FAIL"}]</span> {check.name}
              <small>{check.detail}</small>
            </p>
          ))}
        </div>
        {report?.resumed && (
          <p className="boot-resume">HIBERNATE IMAGE FOUND — RESTORING AARU RUNTIME</p>
        )}
        <div className="boot-cursor" aria-hidden="true" />
      </div>
    </main>
  );
}
