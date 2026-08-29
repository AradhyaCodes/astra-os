import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, type KeyboardEvent } from "react";
import { useWindowStore, type AppId } from "../stores";
import { Taskbar } from "./Taskbar";
import { AppIcon, Window, type IconName } from "../components";
import {
  Almanac,
  AppShell,
  Calculator,
  HostApps,
  ImageViewer,
  SecurityPanel,
  Settings,
  SystemInfo,
  TaskManager,
  TextEditor,
} from "../applications";
import type {
  AuthenticationStatus,
  HostAppInfo,
  MountView,
  ResumeSession,
} from "../types";

interface DesktopProps {
  authenticated: boolean;
  onAuthenticated: (status: AuthenticationStatus) => void;
  onLogout: () => Promise<void>;
  resumeSession: ResumeSession | null;
}
interface DesktopResource {
  name: string;
  icon: IconName;
  kind: "virtual" | "host" | "system";
  command: string;
  app?: { id: AppId; title: string; width: number; height: number };
}

const RESOURCES: DesktopResource[] = [
  {
    name: "Documents",
    icon: "folder",
    kind: "virtual",
    command: "almanac open Documents",
  },
  {
    name: "Downloads",
    icon: "download",
    kind: "virtual",
    command: "almanac open Downloads",
  },
  {
    name: "Applications",
    icon: "apps",
    kind: "virtual",
    command: "almanac open Applications",
  },
  { name: "Games", icon: "games", kind: "virtual", command: "almanac open Games" },
  { name: "Pictures", icon: "image", kind: "virtual", command: "almanac open Pictures" },
  { name: "Music", icon: "music", kind: "virtual", command: "almanac open Music" },
  { name: "Projects", icon: "code", kind: "virtual", command: "almanac open Projects" },
  { name: "HOST", icon: "host", kind: "host", command: "almanac open HOST" },
  {
    name: "Almanac",
    icon: "terminal",
    kind: "system",
    command: "almanac",
    app: { id: "almanac", title: "Almanac", width: 900, height: 620 },
  },
  {
    name: "Windows Apps",
    icon: "apps",
    kind: "system",
    command: "almanac run <app>",
    app: { id: "host-apps", title: "Windows Apps", width: 640, height: 520 },
  },
];

export function Desktop({
  authenticated,
  onAuthenticated,
  onLogout,
  resumeSession,
}: DesktopProps) {
  const store = useWindowStore();
  const openWindowInStore = useWindowStore((state) => state.openWindow);
  const constrainOpenWindows = useWindowStore((state) => state.constrainWindows);
  const restoreSession = useWindowStore((state) => state.restoreSession);
  const [selected, setSelected] = useState<string | null>(null);
  const [hint, setHint] = useState<DesktopResource | null>(null);
  const [hibernating, setHibernating] = useState(resumeSession !== null);
  const [hostMounts, setHostMounts] = useState<MountView[]>([]);
  const [hostApps, setHostApps] = useState<HostAppInfo[]>([]);

  useEffect(() => {
    document.documentElement.classList.toggle(
      "reduce-glass",
      (localStorage.getItem("astra-reduced-glass") ??
        localStorage.getItem("aaru-reduced-glass")) === "1",
    );
    const constrain = () => constrainOpenWindows(window.innerWidth, window.innerHeight);
    constrain();
    window.addEventListener("resize", constrain);
    return () => window.removeEventListener("resize", constrain);
  }, [constrainOpenWindows]);

  useEffect(() => {
    const restored = resumeSession?.ui_session;
    if (restored?.windows) {
      restoreSession(restored.windows, restored.activeWindowId ?? null);
      constrainOpenWindows(window.innerWidth, window.innerHeight);
    }
  }, [constrainOpenWindows, restoreSession, resumeSession]);

  useEffect(() => {
    const hibernate = () => setHibernating(true);
    window.addEventListener("astra:hibernate", hibernate);
    return () => window.removeEventListener("astra:hibernate", hibernate);
  }, []);

  useEffect(() => {
    // Desktop icons: installed apps only, minus the always-present Windows
    // built-ins (still listed in the Applications window).
    const ALWAYS_PRESENT = new Set(["Explorer", "Notepad"]);
    void invoke<HostAppInfo[]>("host_apps")
      .then((apps) =>
        setHostApps(apps.filter((app) => app.installed && !ALWAYS_PRESENT.has(app.name))),
      )
      .catch(() => setHostApps([]));
  }, []);

  useEffect(() => {
    const openAlmanac = (event: globalThis.KeyboardEvent) => {
      if (
        event.ctrlKey &&
        !event.altKey &&
        !event.metaKey &&
        !event.shiftKey &&
        event.code === "Backquote" &&
        !event.repeat &&
        !hibernating
      ) {
        event.preventDefault();
        openWindowInStore("almanac", "Almanac", 900, 620);
      }
    };
    window.addEventListener("keydown", openAlmanac);
    return () => window.removeEventListener("keydown", openAlmanac);
  }, [hibernating, openWindowInStore]);

  const openApp = (appId: AppId, title: string, width: number, height: number) =>
    openWindowInStore(appId, title, width, height);
  const activateResource = (resource: DesktopResource) => {
    setSelected(resource.name);
    if (resource.app)
      openApp(
        resource.app.id,
        resource.app.title,
        resource.app.width,
        resource.app.height,
      );
    else {
      setHint(resource);
      if (resource.kind === "host") {
        void invoke<MountView[]>("host_mounts")
          .then(setHostMounts)
          .catch(() => setHostMounts([]));
      }
    }
  };
  const handleKey = (
    event: KeyboardEvent<HTMLButtonElement>,
    resource: DesktopResource,
  ) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      activateResource(resource);
    }
  };
  const runAlmanacCommand = (command: string) => {
    openApp("almanac", "Almanac", 900, 620);
    window.setTimeout(
      () => window.dispatchEvent(new CustomEvent("astra:command", { detail: command })),
      50,
    );
  };

  return (
    <main
      className="desktop"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) {
          setSelected(null);
          setHint(null);
        }
      }}
    >
      <div className="wallpaper-orbit orbit-one" />
      <div className="wallpaper-orbit orbit-two" />
      <section className="desktop-icons" aria-label="Desktop resources">
        {RESOURCES.map((resource) => (
          <button
            key={resource.name}
            type="button"
            className={`desktop-icon ${selected === resource.name ? "selected" : ""}`}
            onClick={() => {
              setSelected(resource.name);
              if (!resource.app) {
                setHint(resource);
                if (resource.kind === "host") {
                  void invoke<MountView[]>("host_mounts")
                    .then(setHostMounts)
                    .catch(() => setHostMounts([]));
                }
              }
            }}
            onDoubleClick={() => activateResource(resource)}
            onKeyDown={(event) => handleKey(event, resource)}
            aria-label={`${resource.name}, ${resource.kind === "host" ? "host laptop resource" : resource.kind === "virtual" ? "Astra virtual resource" : "Astra application"}`}
          >
            <span className={`desktop-icon-art ${resource.kind}`}>
              <AppIcon name={resource.icon} />
              {resource.kind !== "system" && (
                <small>{resource.kind === "host" ? "H" : "A"}</small>
              )}
            </span>
            <span className="desktop-icon-label">{resource.name}</span>
            {resource.kind !== "system" && (
              <span className={`resource-badge ${resource.kind}`}>
                {resource.kind === "host" ? "HOST" : "VIRTUAL"}
              </span>
            )}
          </button>
        ))}
        {hostApps.map((app) => (
          <button
            key={`hostapp-${app.name}`}
            type="button"
            className={`desktop-icon ${selected === `hostapp-${app.name}` ? "selected" : ""}`}
            onClick={() => setSelected(`hostapp-${app.name}`)}
            onDoubleClick={() => {
              setSelected(`hostapp-${app.name}`);
              runAlmanacCommand(`almanac run ${app.name}`);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                runAlmanacCommand(`almanac run ${app.name}`);
              }
            }}
            aria-label={`${app.name}, installed Windows application`}
          >
            <span className="desktop-icon-art host">
              <AppIcon name="apps" />
              <small>H</small>
            </span>
            <span className="desktop-icon-label">{app.name}</span>
            <span className="resource-badge host">HOST</span>
          </button>
        ))}
      </section>
      {hint && (
        <aside className={`desktop-hint ${hint.kind}`} role="status">
          <div className="hint-icon">
            <AppIcon name={hint.icon} />
          </div>
          <div>
            <span>{hint.kind === "host" ? "LAPTOP RESOURCE" : "ASTRA RESOURCE"}</span>
            <strong>{hint.name}</strong>
            <p>
              {hint.kind === "host"
                ? "Mounted laptop directories live here. Astra does not encrypt or lock Windows files."
                : "Open this resource through Almanac to preserve the system command flow."}
            </p>
            <code>{hint.command}</code>
            {hint.kind === "host" && (
              <div className="host-tree" aria-label="Mounted laptop directories">
                <strong>HOST</strong>
                {hostMounts.length === 0 ? (
                  <span>└── No mounted directories</span>
                ) : (
                  hostMounts.map((mount, index) => (
                    <span key={mount.alias}>
                      {index === hostMounts.length - 1 ? "└──" : "├──"} {mount.alias}
                      {mount.available ? "" : " [offline]"}
                    </span>
                  ))
                )}
              </div>
            )}
          </div>
          <button
            type="button"
            aria-label="Dismiss resource hint"
            onClick={() => setHint(null)}
          >
            ×
          </button>
        </aside>
      )}
      {store.windows.map((windowState) => (
        <Window key={windowState.id} windowState={windowState}>
          {(windowState.appId === "almanac" || windowState.appId === "terminal") && (
            <Almanac
              authenticated={authenticated}
              acceptExternalCommands={windowState.appId === "almanac"}
              onAuthenticated={onAuthenticated}
              onLogout={onLogout}
              resumeSession={resumeSession}
            />
          )}
          {!authenticated &&
          windowState.appId !== "almanac" &&
          windowState.appId !== "terminal" ? (
            <div className="session-locked-window">
              <AppIcon name="shield" />
              <strong>LapSession locked</strong>
              <p>Authenticate in Almanac to restore system interaction.</p>
            </div>
          ) : (
            <>
              {windowState.appId === "system-info" && <SystemInfo />}
              {windowState.appId === "security" && <SecurityPanel />}
              {windowState.appId === "taskmanager" && <TaskManager />}
              {windowState.appId === "settings" && <Settings />}
              {windowState.appId === "calculator" && <Calculator />}
              {windowState.appId === "texteditor" && <TextEditor />}
              {windowState.appId === "imageviewer" && <ImageViewer />}
              {windowState.appId === "app-shell" && (
                <AppShell title={windowState.title} />
              )}
              {windowState.appId === "host-apps" && (
                <HostApps onLaunch={runAlmanacCommand} />
              )}
            </>
          )}
        </Window>
      ))}
      {hibernating && (
        <button
          className="hibernate-screen"
          type="button"
          onClick={() => {
            void invoke("lifecycle_resume");
            setHibernating(false);
          }}
        >
          <AppIcon name="astra" />
          <strong>Astra is hibernating</strong>
          <span>Simulated runtime, windows, and Almanac state are saved.</span>
          <small>Click to resume this session</small>
        </button>
      )}
      <Taskbar authenticated={authenticated} onAlmanacCommand={runAlmanacCommand} />
    </main>
  );
}
