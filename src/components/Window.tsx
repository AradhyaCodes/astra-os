import {
  useRef,
  type CSSProperties,
  type MouseEvent,
  type PointerEvent,
  type ReactNode,
} from "react";
import { useWindowStore, type WindowState } from "../stores";
import { AppIcon, type IconName } from "./AppIcon";

interface WindowProps {
  windowState: WindowState;
  children: ReactNode;
}

export function Window({ windowState, children }: WindowProps) {
  const { id, title, x, y, width, height, zIndex, isMaximized, isMinimized } =
    windowState;
  const store = useWindowStore();
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    initialX: number;
    initialY: number;
  } | null>(null);

  if (isMinimized) {
    return null;
  }

  const handleWindowPointerDown = () => {
    store.focusWindow(id);
  };

  const handleTitlePointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || isMaximized) return;
    if ((event.target as HTMLElement).closest(".window-btn")) return;

    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      initialX: x,
      initialY: y,
    };
    event.preventDefault();
  };

  const handleTitlePointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (drag === null || drag.pointerId !== event.pointerId || isMaximized) return;

    const maxX = Math.max(0, window.innerWidth - width);
    const maxY = Math.max(0, window.innerHeight - 48 - height);
    const nextX = Math.min(
      Math.max(0, drag.initialX + event.clientX - drag.startX),
      maxX,
    );
    const nextY = Math.min(
      Math.max(0, drag.initialY + event.clientY - drag.startY),
      maxY,
    );

    store.updatePosition(id, nextX, nextY);
  };

  const handleTitlePointerUp = (event: PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    dragRef.current = null;
  };

  const handleDoubleClickTitle = (event: MouseEvent) => {
    if ((event.target as HTMLElement).closest(".window-btn")) return;
    store.toggleMaximize(id);
  };

  const style: CSSProperties = isMaximized
    ? {
        top: 0,
        left: 0,
        width: "100%",
        height: "calc(100% - 48px)", // Taskbar space
        zIndex,
      }
    : {
        top: y,
        left: x,
        width,
        height,
        zIndex,
      };

  return (
    <div
      className={`window ${store.activeWindowId === id ? "active" : ""} ${isMaximized ? "maximized" : ""}`}
      style={style}
      onPointerDown={handleWindowPointerDown}
      role="region"
      aria-labelledby={`${id}-title`}
    >
      <div
        className="window-title-bar"
        onDoubleClick={handleDoubleClickTitle}
        onPointerDown={handleTitlePointerDown}
        onPointerMove={handleTitlePointerMove}
        onPointerUp={handleTitlePointerUp}
        onPointerCancel={handleTitlePointerUp}
      >
        <div className="window-title" id={`${id}-title`}>
          <AppIcon name={windowIcon(windowState.appId)} />
          <span>{title}</span>
        </div>
        <div className="window-controls">
          <button
            className="window-btn minimize-btn"
            onClick={(e) => {
              e.stopPropagation();
              store.toggleMinimize(id);
            }}
            aria-label="Minimize"
          >
            <svg viewBox="0 0 10 1" width="10" height="1">
              <path fill="currentColor" d="M0 0h10v1H0z" />
            </svg>
          </button>
          <button
            className="window-btn maximize-btn"
            onClick={(e) => {
              e.stopPropagation();
              store.toggleMaximize(id);
            }}
            aria-label={isMaximized ? "Restore" : "Maximize"}
          >
            {isMaximized ? (
              <svg viewBox="0 0 10 10" width="10" height="10">
                <path
                  fill="currentColor"
                  d="M3 0v3H0v7h7V7h3V0H3zm3 9H1V4h5v5zm3-3H7V3H3V1h6v5z"
                />
              </svg>
            ) : (
              <svg viewBox="0 0 10 10" width="10" height="10">
                <path fill="none" stroke="currentColor" d="M.5.5h9v9h-9z" />
              </svg>
            )}
          </button>
          <button
            className="window-btn close-btn"
            onClick={(e) => {
              e.stopPropagation();
              store.closeWindow(id);
            }}
            aria-label="Close"
          >
            <svg viewBox="0 0 10 10" width="10" height="10">
              <path
                fill="currentColor"
                d="M6.4 5l3.3-3.3-.7-.7L5.7 4.3 2.4 1l-.7.7L5 5 1.7 8.3l.7.7 3.3-3.3 3.3 3.3.7-.7z"
              />
            </svg>
          </button>
        </div>
      </div>
      <div className="window-content">{children}</div>
    </div>
  );
}

function windowIcon(appId: WindowState["appId"]): IconName {
  if (appId === "taskmanager") return "task";
  if (appId === "settings") return "settings";
  if (appId === "calculator") return "calculator";
  if (appId === "texteditor") return "editor";
  if (appId === "imageviewer") return "image";
  if (appId === "system-info") return "status";
  if (appId === "security") return "shield";
  return "terminal";
}
