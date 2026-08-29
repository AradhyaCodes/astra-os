import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import type {
  MemorySnapshot,
  PcbView,
  ProcessMemoryView,
  ReplacementPolicy,
  SchedProcessView,
  SchedulerAlgorithm,
  SchedulerSnapshot,
} from "../types";

const POLL_MS = 1000;

const ALGORITHMS: { value: SchedulerAlgorithm; label: string }[] = [
  { value: "RoundRobin", label: "Round Robin" },
  { value: "FCFS", label: "FCFS" },
  { value: "Priority", label: "Priority" },
];

const POLICIES: ReplacementPolicy[] = ["FIFO", "LRU"];

// A stable, distinguishable colour per PID for the aggregated frame bar.
const SPAN_COLORS = [
  "#9cdcfe",
  "#f0a35e",
  "#c586c0",
  "#6a9955",
  "#dcdcaa",
  "#4ec9b0",
  "#ce9178",
  "#569cd6",
];
const spanColor = (pid: number | null) =>
  pid == null ? "rgba(255,255,255,0.08)" : SPAN_COLORS[pid % SPAN_COLORS.length];

const pct = (fraction: number) => `${Math.round(fraction * 100)}%`;

export function TaskManager() {
  const [tab, setTab] = useState<"processes" | "performance" | "memory">("processes");
  const [rows, setRows] = useState<PcbView[]>([]);
  const [scheduler, setScheduler] = useState<SchedulerSnapshot | null>(null);
  const [memory, setMemory] = useState<MemorySnapshot | null>(null);
  const [error, setError] = useState("");
  const [busyPid, setBusyPid] = useState<number | null>(null);
  const [switching, setSwitching] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [processes, sched, mem] = await Promise.all([
        invoke<PcbView[]>("process_list"),
        invoke<SchedulerSnapshot>("scheduler_status"),
        invoke<MemorySnapshot>("memory_status"),
      ]);
      setRows(processes);
      setScheduler(sched);
      setMemory(mem);
      setError("");
    } catch (reason) {
      setError(readError(reason));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  const act = async (
    command: "process_terminate" | "process_suspend" | "process_resume",
    pid: number,
  ) => {
    setBusyPid(pid);
    setError("");
    try {
      await invoke(command, { pid });
      await refresh();
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusyPid(null);
    }
  };

  const changeAlgorithm = async (algorithm: SchedulerAlgorithm) => {
    setSwitching(true);
    setError("");
    try {
      setScheduler(
        await invoke<SchedulerSnapshot>("scheduler_set_algorithm", { algorithm }),
      );
      await refresh();
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setSwitching(false);
    }
  };

  const changePolicy = async (policy: ReplacementPolicy) => {
    setSwitching(true);
    setError("");
    try {
      setMemory(await invoke<MemorySnapshot>("memory_set_policy", { policy }));
      await refresh();
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setSwitching(false);
    }
  };

  const schedByPid = useMemo(() => {
    const map = new Map<number, SchedProcessView>();
    for (const proc of scheduler?.processes ?? []) map.set(proc.pid, proc);
    return map;
  }, [scheduler]);

  const memByPid = useMemo(() => {
    const map = new Map<number, ProcessMemoryView>();
    for (const proc of memory?.processes ?? []) map.set(proc.pid, proc);
    return map;
  }, [memory]);

  const nameOf = useCallback(
    (pid: number | null) => {
      if (pid == null) return "—";
      const row = rows.find((r) => r.pid === pid);
      return row ? `${row.name} · ${pid}` : `PID ${pid}`;
    },
    [rows],
  );

  const simulated = rows.filter((row) => row.simulated);
  const host = rows.filter((row) => !row.simulated);

  return (
    <div className="taskman">
      <header className="taskman-header">
        <div>
          <h2>Task Manager</h2>
          <span className="taskman-live">
            <i />
            LIVE BACKEND
          </span>
        </div>
        <div className="taskman-legend">
          <span className="badge badge-sim">SIMULATED</span> Aaru-scheduled
          <span className="badge badge-host">HOST</span> Windows-managed
        </div>
        <button type="button" onClick={() => void refresh()}>
          Refresh
        </button>
      </header>

      <nav className="taskman-tabs" aria-label="Task Manager sections">
        {(["processes", "performance", "memory"] as const).map((item) => (
          <button
            key={item}
            type="button"
            className={tab === item ? "active" : ""}
            aria-current={tab === item ? "page" : undefined}
            onClick={() => setTab(item)}
          >
            {item[0].toUpperCase() + item.slice(1)}
          </button>
        ))}
      </nav>

      {error && (
        <p className="taskman-error" role="alert">
          {error}
        </p>
      )}

      {tab === "performance" && scheduler && (
        <section className="cpu-overview" aria-label="Virtual CPU">
          <div className="cpu-overview-head">
            <h3>Virtual CPU · 2 cores</h3>
            <label className="cpu-algo">
              Scheduler
              <select
                value={scheduler.algorithm}
                disabled={switching}
                onChange={(event) =>
                  void changeAlgorithm(event.target.value as SchedulerAlgorithm)
                }
              >
                {ALGORITHMS.map((algo) => (
                  <option key={algo.value} value={algo.value}>
                    {algo.label}
                  </option>
                ))}
              </select>
            </label>
            {scheduler.quantum != null && (
              <span className="cpu-chip">quantum {scheduler.quantum} ticks</span>
            )}
            <span className="cpu-chip">tick {scheduler.tick}</span>
            <span className="cpu-chip">
              {scheduler.context_switches} context switches
            </span>
          </div>

          <div className="cpu-cores">
            {scheduler.cores.map((core) => (
              <div className="cpu-core" key={core.core}>
                <span className="cpu-core-label">Core {core.core}</span>
                <span className="cpu-core-proc">{nameOf(core.pid)}</span>
                <span className="cpu-core-util">{pct(core.utilization)} busy</span>
              </div>
            ))}
          </div>

          <div className="cpu-meters">
            <Meter label="Total utilization" value={scheduler.utilization} />
            {scheduler.per_core_utilization.map((value, index) => (
              <Meter key={index} label={`Core ${index}`} value={value} />
            ))}
          </div>

          <p className="cpu-ready">
            Ready queue:{" "}
            {scheduler.ready_queue.length === 0
              ? "empty"
              : scheduler.ready_queue.map((pid) => nameOf(pid)).join("  ·  ")}
          </p>
          {scheduler.averages.completed > 0 && (
            <p className="cpu-ready">
              Averages over {scheduler.averages.completed} completed — wait{" "}
              {scheduler.averages.waiting.toFixed(1)} · turnaround{" "}
              {scheduler.averages.turnaround.toFixed(1)} · response{" "}
              {scheduler.averages.response.toFixed(1)} ticks
            </p>
          )}
        </section>
      )}

      {tab === "memory" && memory && (
        <section className="mem-overview" aria-label="Simulated memory">
          <div className="cpu-overview-head">
            <h3>Aaru Memory · {memory.page_size_mb} MB pages</h3>
            <label className="cpu-algo">
              Replacement
              <select
                value={memory.policy}
                disabled={switching}
                onChange={(event) =>
                  void changePolicy(event.target.value as ReplacementPolicy)
                }
              >
                {POLICIES.map((policy) => (
                  <option key={policy} value={policy}>
                    {policy}
                  </option>
                ))}
              </select>
            </label>
            <span className="cpu-chip">{memory.page_faults} page faults</span>
            <span className="cpu-chip">{memory.page_hits} page hits</span>
            <span className="cpu-chip">
              swap-out {memory.swap_outs} · swap-in {memory.swap_ins}
            </span>
          </div>

          <div className="cpu-meters">
            <Meter
              label={`RAM · ${memory.ram_used_mb} / ${memory.ram_total_mb} MB`}
              value={memory.ram_used_mb / memory.ram_total_mb}
            />
            <Meter
              label={`Frames · ${memory.frames_used} / ${memory.frames_total}`}
              value={memory.frames_used / memory.frames_total}
            />
            <Meter
              label={`Swap · ${memory.swap_used_mb} / ${memory.swap_total_mb} MB`}
              value={
                memory.swap_total_mb === 0
                  ? 0
                  : memory.swap_used_mb / memory.swap_total_mb
              }
            />
          </div>

          <div
            className="mem-frames"
            aria-label={`${memory.frames_used} of ${memory.frames_total} frames in use`}
          >
            {memory.frame_spans.map((span, index) => (
              <div
                key={`${span.pid ?? "free"}-${index}`}
                className="mem-frame-span"
                title={
                  span.pid == null
                    ? `${span.frames} free frames`
                    : `${nameOf(span.pid)} — ${span.frames} frames`
                }
                style={{
                  flexGrow: span.frames,
                  background: spanColor(span.pid),
                }}
              />
            ))}
          </div>

          {memory.host && (
            <p className="cpu-ready">
              HOST MEMORY (Windows, shown separately) — {memory.host.used_mb} /{" "}
              {memory.host.total_mb} MB used ({memory.host.load_percent}%)
            </p>
          )}
          <p className="cpu-ready">
            Simulated Aaru RAM is independent of the Windows host machine.
          </p>
        </section>
      )}

      {tab === "processes" && (
        <div className="taskman-scroll">
          <ProcessTable
            caption="Simulated processes (scheduled + paged by Aaru)"
            rows={simulated}
            schedByPid={schedByPid}
            memByPid={memByPid}
            busyPid={busyPid}
            onAct={act}
          />
          <ProcessTable
            caption="Observed host processes (run by Windows — not scheduled or paged by Aaru)"
            rows={host}
            schedByPid={schedByPid}
            memByPid={memByPid}
            busyPid={busyPid}
            onAct={act}
            hostSection
          />
        </div>
      )}

      {tab === "performance" && !scheduler && (
        <div className="taskman-panel-empty">Waiting for scheduler telemetry…</div>
      )}
      {tab === "memory" && !memory && (
        <div className="taskman-panel-empty">Waiting for memory telemetry…</div>
      )}

      <footer className="taskman-footer">
        The Aaru scheduler and memory model cover Aaru processes only. They do not replace
        or control real Windows scheduling or memory.
      </footer>
    </div>
  );
}

function Meter({ label, value }: { label: string; value: number }) {
  return (
    <div className="cpu-meter">
      <div className="cpu-meter-top">
        <span>{label}</span>
        <span>{pct(value)}</span>
      </div>
      <div className="cpu-meter-track">
        <div
          className="cpu-meter-fill"
          style={{ width: `${Math.min(100, Math.round(value * 100))}%` }}
        />
      </div>
    </div>
  );
}

interface ProcessTableProps {
  caption: string;
  rows: PcbView[];
  schedByPid: Map<number, SchedProcessView>;
  memByPid: Map<number, ProcessMemoryView>;
  busyPid: number | null;
  hostSection?: boolean;
  onAct: (
    command: "process_terminate" | "process_suspend" | "process_resume",
    pid: number,
  ) => void;
}

function ProcessTable({
  caption,
  rows,
  schedByPid,
  memByPid,
  busyPid,
  hostSection = false,
  onAct,
}: ProcessTableProps) {
  return (
    <table className="taskman-table">
      <caption className="taskman-caption">{caption}</caption>
      <thead>
        <tr>
          <th>PID</th>
          <th>Name</th>
          <th>Type</th>
          <th>State</th>
          <th>Core</th>
          <th>CPU</th>
          <th>Sim RAM</th>
          <th>Priority</th>
          <th>Origin</th>
          <th aria-label="actions" />
        </tr>
      </thead>
      <tbody>
        {rows.length === 0 && (
          <tr>
            <td className="taskman-empty" colSpan={10}>
              {hostSection
                ? "No host processes launched from Aaru."
                : "No simulated processes running."}
            </td>
          </tr>
        )}
        {rows.map((row) => {
          const sched = schedByPid.get(row.pid);
          const mem = memByPid.get(row.pid);
          const cpu = sched && row.simulated ? pct(sched.cpu_share) : row.cpu;
          return (
            <tr key={row.pid} className={row.state === "TERMINATED" ? "is-dead" : ""}>
              <td>{row.pid}</td>
              <td title={row.command}>
                {row.name}
                {row.parent_pid != null && (
                  <span className="ppid"> ◂ {row.parent_pid}</span>
                )}
                {row.note && <div className="taskman-note">{row.note}</div>}
              </td>
              <td>{row.process_type}</td>
              <td>{row.state}</td>
              <td>
                {sched && sched.core != null ? (
                  <span className="core-pill">core {sched.core}</span>
                ) : (
                  <span className="core-none">{row.simulated ? "—" : "n/a"}</span>
                )}
              </td>
              <td>{cpu}</td>
              <td>
                {mem ? (
                  <span title={`${mem.faults} page faults`}>
                    {mem.resident_mb} MB · {mem.resident_pages}f
                    {mem.swapped_pages > 0 && (
                      <span className="mem-swap"> / {mem.swapped_pages}s</span>
                    )}
                  </span>
                ) : (
                  row.memory
                )}
              </td>
              <td>{row.priority}</td>
              <td>
                <span className={`badge ${row.simulated ? "badge-sim" : "badge-host"}`}>
                  {row.simulated ? "SIMULATED" : "HOST"}
                </span>
              </td>
              <td className="taskman-actions">
                {!row.protected && row.state !== "TERMINATED" && (
                  <>
                    {row.simulated && row.state === "SUSPENDED" ? (
                      <button
                        type="button"
                        disabled={busyPid === row.pid}
                        onClick={() => onAct("process_resume", row.pid)}
                      >
                        Resume
                      </button>
                    ) : row.simulated ? (
                      <button
                        type="button"
                        disabled={busyPid === row.pid}
                        onClick={() => onAct("process_suspend", row.pid)}
                      >
                        Suspend
                      </button>
                    ) : null}
                    <button
                      type="button"
                      className="danger"
                      disabled={busyPid === row.pid}
                      onClick={() => onAct("process_terminate", row.pid)}
                    >
                      End
                    </button>
                  </>
                )}
                {row.protected && <span className="taskman-protected">protected</span>}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function readError(reason: unknown): string {
  const message =
    typeof reason === "string"
      ? reason
      : reason instanceof Error
        ? reason.message
        : "The process manager is unavailable.";
  return message.includes("invoke")
    ? "The process manager requires the Tauri runtime. Run npm run tauri dev."
    : message;
}
