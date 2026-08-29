import { create } from "zustand";

export type AppId =
  | "almanac"
  | "terminal"
  | "system-info"
  | "security"
  | "taskmanager"
  | "settings"
  | "calculator"
  | "texteditor"
  | "imageviewer"
  | "app-shell"
  | "host-apps";

export interface WindowState {
  id: string;
  appId: AppId;
  title: string;
  x: number;
  y: number;
  width: number;
  height: number;
  zIndex: number;
  isMaximized: boolean;
  isMinimized: boolean;
}

interface WindowStore {
  windows: WindowState[];
  activeWindowId: string | null;
  openWindow: (appId: AppId, title: string, width: number, height: number) => void;
  closeWindow: (id: string) => void;
  focusWindow: (id: string) => void;
  updatePosition: (id: string, x: number, y: number) => void;
  updateSize: (id: string, width: number, height: number) => void;
  toggleMaximize: (id: string) => void;
  toggleMinimize: (id: string) => void;
  constrainWindows: (viewportWidth: number, viewportHeight: number) => void;
  restoreSession: (windows: WindowState[], activeWindowId: string | null) => void;
  clearSession: () => void;
}

const TASKBAR_HEIGHT = 48;
const MIN_WINDOW_WIDTH = 320;
const MIN_WINDOW_HEIGHT = 220;

function fitWindowToViewport(width: number, height: number) {
  const viewportWidth = typeof window === "undefined" ? width : window.innerWidth;
  const viewportHeight = typeof window === "undefined" ? height : window.innerHeight;
  const availableWidth = Math.max(MIN_WINDOW_WIDTH, viewportWidth);
  const availableHeight = Math.max(MIN_WINDOW_HEIGHT, viewportHeight - TASKBAR_HEIGHT);

  return {
    width: Math.min(width, availableWidth),
    height: Math.min(height, availableHeight),
    availableWidth,
    availableHeight,
  };
}

export const useWindowStore = create<WindowStore>((set) => ({
  windows: [],
  activeWindowId: null,

  openWindow: (appId, title, width, height) => {
    set((state) => {
      // Check if window already exists for single-instance apps
      const existing = state.windows.find((w) => w.appId === appId);
      if (existing) {
        // Just focus it
        const maxZ = Math.max(...state.windows.map((w) => w.zIndex), 0);
        return {
          windows: state.windows.map((w) =>
            w.id === existing.id ? { ...w, zIndex: maxZ + 1, isMinimized: false } : w,
          ),
          activeWindowId: existing.id,
        };
      }

      const id = `${appId}-${Date.now()}`;
      const maxZ = Math.max(...state.windows.map((w) => w.zIndex), 0);
      const bounds = fitWindowToViewport(width, height);
      const cascadeOffset = state.windows.length * 30;

      const newWindow: WindowState = {
        id,
        appId,
        title,
        x: Math.min(
          50 + cascadeOffset,
          Math.max(0, bounds.availableWidth - bounds.width),
        ),
        y: Math.min(
          50 + cascadeOffset,
          Math.max(0, bounds.availableHeight - bounds.height),
        ),
        width: bounds.width,
        height: bounds.height,
        zIndex: maxZ + 1,
        isMaximized: false,
        isMinimized: false,
      };

      return {
        windows: [...state.windows, newWindow],
        activeWindowId: id,
      };
    });
  },

  closeWindow: (id) => {
    set((state) => {
      const windows = state.windows.filter((w) => w.id !== id);
      const nextActive = windows
        .filter((windowState) => !windowState.isMinimized)
        .reduce<WindowState | null>(
          (highest, windowState) =>
            highest === null || windowState.zIndex > highest.zIndex
              ? windowState
              : highest,
          null,
        );

      return {
        windows,
        activeWindowId:
          state.activeWindowId === id ? (nextActive?.id ?? null) : state.activeWindowId,
      };
    });
  },

  focusWindow: (id) => {
    set((state) => {
      if (state.activeWindowId === id) return state; // Already focused

      const maxZ = Math.max(...state.windows.map((w) => w.zIndex), 0);
      return {
        windows: state.windows.map((w) =>
          w.id === id ? { ...w, zIndex: maxZ + 1, isMinimized: false } : w,
        ),
        activeWindowId: id,
      };
    });
  },

  updatePosition: (id, x, y) => {
    set((state) => ({
      windows: state.windows.map((w) => (w.id === id ? { ...w, x, y } : w)),
    }));
  },

  updateSize: (id, width, height) => {
    set((state) => ({
      windows: state.windows.map((w) => (w.id === id ? { ...w, width, height } : w)),
    }));
  },

  toggleMaximize: (id) => {
    set((state) => ({
      windows: state.windows.map((w) =>
        w.id === id ? { ...w, isMaximized: !w.isMaximized } : w,
      ),
    }));
  },

  toggleMinimize: (id) => {
    set((state) => {
      let nextActive = state.activeWindowId;
      const windows = state.windows.map((w) => {
        if (w.id === id) {
          const isMinimized = !w.isMinimized;
          if (isMinimized && nextActive === id) {
            nextActive = null;
          }
          return { ...w, isMinimized };
        }
        return w;
      });
      if (nextActive === null) {
        const candidate = windows
          .filter((windowState) => !windowState.isMinimized)
          .sort((a, b) => b.zIndex - a.zIndex)[0];
        nextActive = candidate?.id ?? null;
      }
      return { windows, activeWindowId: nextActive };
    });
  },

  constrainWindows: (viewportWidth, viewportHeight) => {
    const availableWidth = Math.max(MIN_WINDOW_WIDTH, viewportWidth);
    const availableHeight = Math.max(MIN_WINDOW_HEIGHT, viewportHeight - TASKBAR_HEIGHT);

    set((state) => ({
      windows: state.windows.map((windowState) => {
        const width = Math.min(windowState.width, availableWidth);
        const height = Math.min(windowState.height, availableHeight);

        return {
          ...windowState,
          width,
          height,
          x: Math.min(Math.max(0, windowState.x), Math.max(0, availableWidth - width)),
          y: Math.min(Math.max(0, windowState.y), Math.max(0, availableHeight - height)),
        };
      }),
    }));
  },

  restoreSession: (windows, activeWindowId) => {
    const valid = windows.filter(
      (item) =>
        typeof item.id === "string" &&
        typeof item.appId === "string" &&
        Number.isFinite(item.x) &&
        Number.isFinite(item.y) &&
        Number.isFinite(item.width) &&
        Number.isFinite(item.height),
    );
    set({ windows: valid, activeWindowId });
  },

  clearSession: () => set({ windows: [], activeWindowId: null }),
}));
