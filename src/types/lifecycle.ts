import type { WindowState } from "../stores";

export interface BootCheck {
  name: string;
  detail: string;
  ok: boolean;
}

export interface ResumeSession {
  cwd: string;
  ui_session: {
    windows?: WindowState[];
    activeWindowId?: string | null;
  } | null;
  almanac_session: {
    cwd?: string;
    history?: unknown[];
  } | null;
}

export interface BootReport {
  version: string;
  checks: BootCheck[];
  resumed: boolean;
  resume_session: ResumeSession | null;
}
