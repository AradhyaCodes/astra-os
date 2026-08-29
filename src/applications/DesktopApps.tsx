import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import type { MountView } from "../types";
import { AppIcon } from "../components";

export function Settings() {
  const [mounts, setMounts] = useState<MountView[]>([]);
  const [message, setMessage] = useState("");
  const [reducedGlass, setReducedGlass] = useState(
    () =>
      (localStorage.getItem("astra-reduced-glass") ??
        localStorage.getItem("aaru-reduced-glass")) === "1",
  );

  const refresh = () =>
    invoke<MountView[]>("host_mounts")
      .then(setMounts)
      .catch(() => setMounts([]));
  useEffect(() => {
    void refresh();
  }, []);

  const toggleGlass = (value: boolean) => {
    setReducedGlass(value);
    localStorage.setItem("astra-reduced-glass", value ? "1" : "0");
    document.documentElement.classList.toggle("reduce-glass", value);
  };

  const mountDirectory = async () => {
    setMessage("");
    try {
      const picked = await invoke<string | null>("host_pick_directory");
      if (!picked) return;
      const alias = await invoke<string>("host_mount", { path: picked, alias: null });
      setMessage(`${alias} is now available beneath HOST.`);
      await refresh();
    } catch (error) {
      setMessage(readError(error));
    }
  };

  return (
    <div className="settings-app app-page">
      <aside className="app-sidebar">
        <div className="sidebar-brand">
          <AppIcon name="settings" />
          Settings
        </div>
        <button className="sidebar-link active" type="button">
          System
        </button>
        <button className="sidebar-link" type="button">
          Appearance
        </button>
        <button className="sidebar-link" type="button">
          Host mounts
        </button>
      </aside>
      <main className="settings-main">
        <header>
          <h2>System settings</h2>
          <p>Desktop preferences and explicit laptop connections.</p>
        </header>
        <section className="settings-section">
          <div>
            <h3>Translucent surfaces</h3>
            <p>Reduce blur while keeping the same surface hierarchy.</p>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={!reducedGlass}
              onChange={(event) => toggleGlass(!event.target.checked)}
            />
            <span />
          </label>
        </section>
        <section className="settings-section settings-mounts">
          <div>
            <h3>HOST mounts</h3>
            <p>
              Mounted laptop folders stay under an alias. Astra does not encrypt or lock
              Windows files.
            </p>
          </div>
          <button
            className="primary-button"
            type="button"
            onClick={() => void mountDirectory()}
          >
            Mount a folder
          </button>
          <div className="mount-list">
            {mounts.length === 0 ? (
              <p className="empty-copy">No laptop folders are mounted.</p>
            ) : (
              mounts.map((mount) => (
                <div className="mount-row" key={mount.alias}>
                  <span className="resource-badge host">HOST</span>
                  <strong>{mount.alias}</strong>
                  <span>{mount.available ? "Available" : "Offline"}</span>
                  <button
                    type="button"
                    onClick={async () => {
                      await invoke("host_unmount", { alias: mount.alias });
                      await refresh();
                    }}
                  >
                    Disconnect
                  </button>
                </div>
              ))
            )}
          </div>
        </section>
        {message && (
          <p className="inline-message" role="status">
            {message}
          </p>
        )}
      </main>
    </div>
  );
}

const CALC_KEYS = [
  "C",
  "±",
  "%",
  "÷",
  "7",
  "8",
  "9",
  "×",
  "4",
  "5",
  "6",
  "−",
  "1",
  "2",
  "3",
  "+",
  "0",
  ".",
  "⌫",
  "=",
];

export function Calculator() {
  const [display, setDisplay] = useState("0");
  const [stored, setStored] = useState<number | null>(null);
  const [operator, setOperator] = useState<string | null>(null);
  const [replace, setReplace] = useState(true);
  const input = (key: string) => {
    if (/\d|\./.test(key)) {
      setDisplay((current) =>
        replace
          ? key === "."
            ? "0."
            : key
          : key === "." && current.includes(".")
            ? current
            : current + key,
      );
      setReplace(false);
      return;
    }
    if (key === "C") {
      setDisplay("0");
      setStored(null);
      setOperator(null);
      setReplace(true);
      return;
    }
    if (key === "⌫") {
      setDisplay((value) => (value.length > 1 ? value.slice(0, -1) : "0"));
      return;
    }
    if (key === "±") {
      setDisplay((value) => String(-Number(value)));
      return;
    }
    if (key === "%") {
      setDisplay((value) => String(Number(value) / 100));
      return;
    }
    const value = Number(display);
    if (key === "=" && stored != null && operator) {
      const result =
        operator === "+"
          ? stored + value
          : operator === "−"
            ? stored - value
            : operator === "×"
              ? stored * value
              : value === 0
                ? NaN
                : stored / value;
      setDisplay(
        Number.isFinite(result)
          ? String(Number(result.toPrecision(12)))
          : "Cannot divide by zero",
      );
      setStored(null);
      setOperator(null);
      setReplace(true);
      return;
    }
    setStored(value);
    setOperator(key);
    setReplace(true);
  };
  return (
    <div className="calculator-app">
      <div className="calc-mode">ASTRA STANDARD</div>
      <output>{display}</output>
      <div className="calc-grid">
        {CALC_KEYS.map((key) => (
          <button
            key={key}
            className={/[÷×−+=]/.test(key) ? "operator" : ""}
            type="button"
            onClick={() => input(key)}
          >
            {key}
          </button>
        ))}
      </div>
    </div>
  );
}

export function TextEditor() {
  const [path, setPath] = useState("Documents>notes.txt");
  const [content, setContent] = useState("");
  const [status, setStatus] = useState("New buffer");
  const load = async () => {
    try {
      setContent(await invoke<string>("fs_read_file", { cwd: "ROOT", path }));
      setStatus(`Opened ROOT>${path}`);
    } catch (error) {
      setStatus(readError(error));
    }
  };
  const save = async () => {
    try {
      await invoke("fs_write_file", { cwd: "ROOT", path, content });
      setStatus(`Saved ROOT>${path}`);
    } catch {
      try {
        await invoke("fs_create_file", { cwd: "ROOT", path, content });
        setStatus(`Created ROOT>${path}`);
      } catch (error) {
        setStatus(readError(error));
      }
    }
  };
  return (
    <div className="editor-app">
      <div className="editor-toolbar">
        <label>
          <span>VIRTUAL PATH</span>
          <input value={path} onChange={(event) => setPath(event.target.value)} />
        </label>
        <button type="button" onClick={() => void load()}>
          Open
        </button>
        <button className="primary-button" type="button" onClick={() => void save()}>
          Save
        </button>
      </div>
      <textarea
        aria-label="Text editor"
        value={content}
        onChange={(event) => {
          setContent(event.target.value);
          setStatus("Unsaved changes");
        }}
        spellCheck="false"
      />
      <footer>
        <span className="resource-badge virtual">VIRTUAL</span>
        {status}
        <span>{content.length} characters</span>
      </footer>
    </div>
  );
}

export function ImageViewer() {
  const [source, setSource] = useState("");
  const [shown, setShown] = useState("");
  const isValid = useMemo(() => /^(data:image\/|https?:\/\/)/i.test(shown), [shown]);
  return (
    <div className="image-viewer">
      <div className="viewer-toolbar">
        <label>
          <span>IMAGE SOURCE</span>
          <input
            value={source}
            onChange={(event) => setSource(event.target.value)}
            placeholder="Paste an image URL or data URL"
          />
        </label>
        <button
          className="primary-button"
          type="button"
          onClick={() => setShown(source.trim())}
        >
          View
        </button>
      </div>
      <div className="viewer-stage">
        {isValid ? (
          <img src={shown} alt="User-selected preview" />
        ) : (
          <div className="viewer-empty">
            <AppIcon name="image" />
            <h2>Image Viewer</h2>
            <p>
              Paste an image or data URL above. Use <code>almanac reveal</code> to open
              physical files with the Windows default app.
            </p>
          </div>
        )}
      </div>
    </div>
  );
}

export function HostWarning({
  action,
  onCancel,
  onConfirm,
}: {
  action: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div
      className="host-warning"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="host-warning-title"
    >
      <AppIcon name="shield" />
      <div>
        <h2 id="host-warning-title">Physical file warning</h2>
        <strong>THIS ACTION AFFECTS PHYSICAL FILES ON THIS COMPUTER.</strong>
        <p>{action}</p>
        <p>Astra cannot undo changes made outside its virtual filesystem.</p>
        <div className="warning-actions">
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
          <button className="danger-button" type="button" onClick={onConfirm}>
            Continue on HOST
          </button>
        </div>
      </div>
    </div>
  );
}

function readError(reason: unknown) {
  return typeof reason === "string"
    ? reason
    : reason instanceof Error
      ? reason.message
      : "The backend could not complete this request.";
}
