import { useEffect, useRef, useState } from "react";
import { AppIcon, type IconName } from "../components";
import { useWindowStore, type AppId } from "../stores";

interface TaskbarProps {
  authenticated: boolean;
  onAlmanacCommand: (command: string) => void;
}
const START_ITEMS: {
  label: string;
  detail: string;
  shortcut?: string;
  icon: IconName;
  app: [AppId, string, number, number];
}[] = [
  {
    label: "Almanac",
    detail: "Command language",
    shortcut: "Ctrl + `",
    icon: "terminal",
    app: ["almanac", "Almanac", 900, 620],
  },
  {
    label: "System Status",
    detail: "Kernel and runtime",
    icon: "status",
    app: ["system-info", "System Status", 720, 600],
  },
  {
    label: "Task Manager",
    detail: "Processes and resources",
    icon: "task",
    app: ["taskmanager", "Task Manager", 920, 640],
  },
  {
    label: "Settings",
    detail: "Desktop and HOST mounts",
    icon: "settings",
    app: ["settings", "Settings", 780, 560],
  },
  {
    label: "Windows Apps",
    detail: "Installed Windows apps",
    icon: "apps",
    app: ["host-apps", "Windows Apps", 640, 520],
  },
];
const appIcon = (id: AppId): IconName =>
  id === "taskmanager"
    ? "task"
    : id === "settings"
      ? "settings"
      : id === "calculator"
        ? "calculator"
        : id === "texteditor"
          ? "editor"
          : id === "imageviewer"
            ? "image"
            : id === "system-info"
              ? "status"
              : id === "host-apps"
                ? "apps"
                : "terminal";

export function Taskbar({ authenticated, onAlmanacCommand }: TaskbarProps) {
  const store = useWindowStore();
  const [time, setTime] = useState(new Date());
  const [startOpen, setStartOpen] = useState(false);
  const [trayOpen, setTrayOpen] = useState(false);
  const startRef = useRef<HTMLDivElement>(null);
  const trayRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const timer = setInterval(() => setTime(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    const close = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!startRef.current?.contains(target)) setStartOpen(false);
      if (!trayRef.current?.contains(target)) setTrayOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setStartOpen(false);
        setTrayOpen(false);
      }
    };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", close);
      document.removeEventListener("keydown", escape);
    };
  }, []);
  const open = ([id, title, width, height]: [AppId, string, number, number]) => {
    store.openWindow(id, title, width, height);
    setStartOpen(false);
  };
  const toggleWindow = (id: string) => {
    const state = store.windows.find((windowState) => windowState.id === id);
    if (!state) return;
    if (store.activeWindowId === id && !state.isMinimized) store.toggleMinimize(id);
    else store.focusWindow(id);
  };

  return (
    <nav className="taskbar" aria-label="Desktop taskbar">
      <div className="taskbar-context">
        <span className="status-dot" />
        <span>{authenticated ? "AARU ONLINE" : "SESSION LOCKED"}</span>
      </div>
      <div className="taskbar-center">
        <div className="launcher" ref={startRef}>
          <button
            className={`taskbar-icon-btn start-button ${startOpen ? "active" : ""}`}
            type="button"
            aria-label="Open Start"
            aria-expanded={startOpen}
            onClick={() => {
              setStartOpen((value) => !value);
              setTrayOpen(false);
            }}
          >
            <AppIcon name="aaru" />
          </button>
          {startOpen && (
            <section className="start-menu" aria-label="Start menu">
              <header>
                <AppIcon name="aaru" />
                <div>
                  <strong>Aaru</strong>
                  <span>
                    {authenticated ? "LapSession active" : "Authentication required"}
                  </span>
                </div>
                <span className="session-pill">ROOT</span>
              </header>
              <div className="start-list">
                {START_ITEMS.map((item) => (
                  <button
                    type="button"
                    key={item.label}
                    disabled={!authenticated && item.app[0] !== "almanac"}
                    onClick={() => open(item.app)}
                  >
                    <span className="start-app-icon">
                      <AppIcon name={item.icon} />
                    </span>
                    <span>
                      <strong>{item.label}</strong>
                      <small>{item.detail}</small>
                    </span>
                    {item.shortcut ? (
                      <kbd className="start-shortcut">{item.shortcut}</kbd>
                    ) : (
                      <AppIcon name="chevron" />
                    )}
                  </button>
                ))}
              </div>
              <footer>
                <button
                  type="button"
                  disabled={!authenticated}
                  onClick={() => onAlmanacCommand("almanac logout")}
                >
                  <AppIcon name="power" />
                  Logout
                </button>
                <div className="power-actions">
                  <button
                    type="button"
                    disabled={!authenticated}
                    onClick={() => {
                      setStartOpen(false);
                      onAlmanacCommand("almanac hibernate");
                    }}
                  >
                    Hibernate
                  </button>
                  <button
                    type="button"
                    disabled={!authenticated}
                    onClick={() => {
                      setStartOpen(false);
                      onAlmanacCommand("almanac restart");
                    }}
                  >
                    Restart
                  </button>
                  <button
                    className="danger-text"
                    type="button"
                    disabled={!authenticated}
                    onClick={() => {
                      setStartOpen(false);
                      onAlmanacCommand("almanac kill lapsession");
                    }}
                  >
                    Kill LapSession
                  </button>
                </div>
              </footer>
            </section>
          )}
        </div>
        <button
          className="taskbar-icon-btn pinned"
          type="button"
          aria-label="Open Almanac (Ctrl + backtick)"
          title="Open Almanac · Ctrl + `"
          onClick={() => open(["almanac", "Almanac", 900, 620])}
        >
          <AppIcon name="terminal" />
        </button>
        <div className="taskbar-divider" />
        <div className="taskbar-apps">
          {store.windows.map((windowState) => (
            <button
              key={windowState.id}
              type="button"
              className={`taskbar-window-btn ${store.activeWindowId === windowState.id && !windowState.isMinimized ? "active" : ""}`}
              onClick={() => toggleWindow(windowState.id)}
              title={windowState.title}
            >
              <AppIcon name={appIcon(windowState.appId)} />
              <span>{windowState.title}</span>
            </button>
          ))}
        </div>
      </div>
      <div className="taskbar-right" ref={trayRef}>
        <button
          className="tray-button"
          type="button"
          aria-expanded={trayOpen}
          aria-label="Open system tray"
          onClick={() => {
            setTrayOpen((value) => !value);
            setStartOpen(false);
          }}
        >
          <AppIcon name="network" />
          <AppIcon name="speaker" />
          <span>
            <strong>
              {time.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
            </strong>
            <small>
              {time.toLocaleDateString([], { day: "2-digit", month: "short" })}
            </small>
          </span>
        </button>
        {trayOpen && (
          <section className="tray-panel">
            <header>
              <strong>System tray</strong>
              <span>LIVE</span>
            </header>
            <div className="tray-grid">
              <button type="button">
                <AppIcon name="network" />
                <span>
                  Network<strong>Host connected</strong>
                </span>
              </button>
              <button type="button">
                <AppIcon name="speaker" />
                <span>
                  Audio<strong>Windows managed</strong>
                </span>
              </button>
            </div>
            <div className="tray-status">
              <span className="resource-badge virtual">VIRTUAL</span>
              <p>Aaru scheduler and memory simulation active</p>
            </div>
            <div className="tray-status host">
              <span className="resource-badge host">HOST</span>
              <p>Physical resources stay Windows-managed</p>
            </div>
          </section>
        )}
      </div>
    </nav>
  );
}
