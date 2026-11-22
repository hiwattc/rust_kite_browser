export interface Tab {
  id: number;
  url: string;
  title: string;
  loading: boolean;
  favicon: string | null;
  zoom: number;
  bookmarked: boolean;
  live: boolean;
  /** Showing the "cannot reach" page for `url`. */
  error?: boolean;
  /** The user chose to continue despite this site's certificate. */
  badCert?: boolean;
}

export interface McpStatus {
  running: boolean;
  port: number;
  url: string;
}

export interface DownloadItem {
  id: number;
  url: string;
  name: string;
  path: string;
  total: number;
  received: number;
  state: "progress" | "done" | "failed" | "canceled";
  started: number;
  ended: number | null;
  missing: boolean;
  cancelable: boolean;
}
export interface DownloadsSnapshot {
  items: DownloadItem[];
  /** 0..1 over active downloads, -1 when a size is unknown. */
  progress: number;
  active: number;
  dir: string;
}

export interface BrowserState {
  tabs: Tab[];
  active: number | null;
  agentTab: number | null;
  mcp: McpStatus;
  /** The vertical tab list on the left is open. */
  side: boolean;
}

export interface BarItem {
  kind: "link" | "folder";
  title: string;
  /** URL, or the folder name for folders */
  url: string;
}

export interface Bookmark {
  id: number;
  url: string;
  title: string;
  folder: string;
  created: number;
}

export interface Init {
  state: BrowserState;
  bookmarkBar: BarItem[] | null;
  keymap: Record<string, string>;
  platform: "mac" | "windows" | "linux";
  extensions: ToolbarExt[];
}

/** An installed extension's toolbar button. */
export interface ToolbarExt {
  id: string;
  name: string;
  /** `data:` URL */
  icon: string | null;
  popup: boolean;
}

export interface Item {
  kind: "tab" | "bookmark" | "history" | "command" | "url" | "search";
  id: string;
  title: string;
  subtitle: string;
  icon?: string;
  hint?: string;
}

export interface Group {
  title: string;
  items: Item[];
}

export const NEWTAB = "kite://newtab";
