export interface Tab {
  id: number;
  url: string;
  title: string;
  loading: boolean;
  favicon: string | null;
  zoom: number;
  bookmarked: boolean;
  live: boolean;
}

export interface McpStatus {
  running: boolean;
  port: number;
  url: string;
}

export interface BrowserState {
  tabs: Tab[];
  active: number | null;
  agentTab: number | null;
  mcp: McpStatus;
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
