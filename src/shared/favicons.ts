// Site icons for lists of addresses (bookmarks): only the icons Kite kept while each site was
// open in a tab (`favicons` command, data URLs from its database). Never `https://site/favicon.ico`:
// a list must not contact every site in it (that looks up and connects to old or unwanted
// bookmarked sites just by opening the list).
import { invoke } from "@tauri-apps/api/core";

const icons = new Map<string, string>();
const asked = new Set<string>();

export function originOf(u: string) {
  try {
    const x = new URL(u);
    return /^https?:$/.test(x.protocol) ? x.origin : "";
  } catch {
    return "";
  }
}

/** The saved icon (data URL) for this address, or "" (show a generic icon). */
export function savedIcon(u: string) {
  return icons.get(originOf(u)) ?? "";
}

/** Reads saved icons for these addresses from Kite. True when new ones arrived (draw again). */
export async function loadIcons(urls: string[]): Promise<boolean> {
  const want = [...new Set(urls.map(originOf).filter((o) => o && !asked.has(o)))];
  if (!want.length) return false;
  want.forEach((o) => asked.add(o));
  const got = await invoke<Record<string, string>>("favicons", { origins: want }).catch(() => ({}) as Record<string, string>);
  let added = 0;
  for (const [o, d] of Object.entries(got)) {
    if (d.startsWith("data:image/")) {
      icons.set(o, d);
      added++;
    }
  }
  return added > 0;
}
