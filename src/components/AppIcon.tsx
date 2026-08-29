import type { ReactElement, SVGProps } from "react";

export type IconName =
  | "aaru"
  | "terminal"
  | "folder"
  | "download"
  | "apps"
  | "games"
  | "image"
  | "music"
  | "code"
  | "host"
  | "status"
  | "task"
  | "settings"
  | "calculator"
  | "editor"
  | "power"
  | "chevron"
  | "shield"
  | "speaker"
  | "network";

interface AppIconProps extends SVGProps<SVGSVGElement> {
  name: IconName;
}

export function AppIcon({ name, ...props }: AppIconProps) {
  const paths: Record<IconName, ReactElement> = {
    aaru: (
      <path
        d="M10.7 2.25h2.6l9.45 19.5h-5.1l-2.15-4.7h-7l-2.15 4.7h-5.1l9.45-19.5ZM12 8.7l-2.05 4.6h4.1L12 8.7Z"
        fill="#8b7cff"
        fillRule="evenodd"
        stroke="none"
      />
    ),
    terminal: (
      <>
        <path d="m5 7 4 4-4 4" />
        <path d="M11.5 16H19" />
        <rect x="2.5" y="3.5" width="19" height="17" rx="3" />
      </>
    ),
    folder: <path d="M3 6.5h7l2 2h9v9.5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6.5Z" />,
    download: (
      <>
        <path d="M12 3v11m0 0 4-4m-4 4-4-4" />
        <path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
      </>
    ),
    apps: (
      <>
        <rect x="3" y="3" width="7" height="7" rx="1.5" />
        <rect x="14" y="3" width="7" height="7" rx="1.5" />
        <rect x="3" y="14" width="7" height="7" rx="1.5" />
        <rect x="14" y="14" width="7" height="7" rx="1.5" />
      </>
    ),
    games: (
      <path d="M8 8h8a5 5 0 0 1 4.6 7l-1.1 2.6a2.4 2.4 0 0 1-4 .7L13.7 16h-3.4l-1.8 2.3a2.4 2.4 0 0 1-4-.7L3.4 15A5 5 0 0 1 8 8Zm-1 4v4m-2-2h4m7-1h.01M18 15h.01" />
    ),
    image: (
      <>
        <rect x="3" y="4" width="18" height="16" rx="2" />
        <circle cx="8.5" cy="9" r="1.5" />
        <path d="m4 17 5-4 3 3 3-2 5 4" />
      </>
    ),
    music: (
      <>
        <path d="M9 18V6l10-2v12" />
        <circle cx="6" cy="18" r="3" />
        <circle cx="16" cy="16" r="3" />
      </>
    ),
    code: (
      <>
        <path d="m9 7-5 5 5 5m6-10 5 5-5 5" />
        <path d="m13.5 4-3 16" />
      </>
    ),
    host: (
      <>
        <rect x="3" y="4" width="18" height="13" rx="2" />
        <path d="M8 21h8m-4-4v4" />
        <circle cx="6.5" cy="7.5" r=".5" fill="currentColor" />
      </>
    ),
    status: (
      <>
        <path d="M4 18V9m5 9V5m5 13v-7m5 7V3" />
      </>
    ),
    task: (
      <>
        <rect x="3" y="3" width="18" height="18" rx="3" />
        <path d="M7 15h2l2-6 3 9 2-6h2" />
      </>
    ),
    settings: (
      <>
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6a1.7 1.7 0 0 0 1-1.6v-.2h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z" />
      </>
    ),
    calculator: (
      <>
        <rect x="5" y="2.5" width="14" height="19" rx="2" />
        <path d="M8 6h8v3H8zM8 13h.01M12 13h.01M16 13h.01M8 17h.01M12 17h.01M16 17h.01" />
      </>
    ),
    editor: (
      <>
        <path d="M4 4h10l6 6v10H4V4Z" />
        <path d="M14 4v6h6M8 14h8m-8 3h6" />
      </>
    ),
    power: (
      <>
        <path d="M12 2v10" />
        <path d="M7 5.4a8 8 0 1 0 10 0" />
      </>
    ),
    chevron: <path d="m9 6 6 6-6 6" />,
    shield: <path d="M12 2 4 5v6c0 5.2 3.4 9 8 11 4.6-2 8-5.8 8-11V5l-8-3Z" />,
    speaker: (
      <>
        <path d="M4 10v4h4l5 4V6L8 10H4Z" />
        <path d="M16 9a4 4 0 0 1 0 6m2-8a7 7 0 0 1 0 10" />
      </>
    ),
    network: (
      <>
        <path d="M4 9a12 12 0 0 1 16 0M7 12.5a7.5 7.5 0 0 1 10 0M10 16a3 3 0 0 1 4 0" />
        <circle cx="12" cy="19" r="1" fill="currentColor" />
      </>
    ),
  };
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      {...props}
    >
      {paths[name]}
    </svg>
  );
}
