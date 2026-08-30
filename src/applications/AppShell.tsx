import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { PcbView } from "../types";

/**
 * Generic placeholder window for built-in Astra apps and games that are
 * registered as processes but do not yet have a bespoke UI (for example,
 * Snake, Pong, Minesweeper and Tetris). This is not a playable game surface.
 *
 * It shows the live process record so `almanac run <App>` visibly does
 * something and the simulated workload metadata is inspectable.
 */
export function AppShell({ title }: { title: string }) {
  const [process, setProcess] = useState<PcbView | null>(null);

  useEffect(() => {
    const wanted = title.toLowerCase();
    const load = async () => {
      try {
        const list = await invoke<PcbView[]>("process_list");
        const match = [...list]
          .reverse()
          .find((p) => p.name.toLowerCase() === wanted && p.state !== "TERMINATED");
        setProcess(match ?? null);
      } catch {
        setProcess(null);
      }
    };
    void load();
    const timer = setInterval(() => void load(), 2000);
    return () => clearInterval(timer);
  }, [title]);

  return (
    <div className="app-shell">
      <h2>{title}</h2>
      <p className="app-shell-sub">
        Built-in Astra application — a full UI arrives in a later phase.
      </p>
      {process ? (
        <dl className="app-shell-grid">
          <dt>Astra PID</dt>
          <dd>{process.pid}</dd>
          <dt>Type</dt>
          <dd>{process.process_type}</dd>
          <dt>State</dt>
          <dd>{process.state}</dd>
          <dt>Priority</dt>
          <dd>{process.priority}</dd>
          <dt>CPU</dt>
          <dd>{process.cpu}</dd>
          <dt>Memory</dt>
          <dd>{process.memory}</dd>
          <dt>Workload</dt>
          <dd>{process.workload}</dd>
        </dl>
      ) : (
        <p>
          No running process for “{title}”. Launch it from Almanac:{" "}
          <code>almanac run {title}</code>
        </p>
      )}
    </div>
  );
}
