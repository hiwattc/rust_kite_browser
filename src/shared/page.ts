// Helpers for Kite's own pages in tabs (settings, bookmarks).
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import "../styles/page.css";

// Never work inside a frame: a web page could embed us to trick clicks (clickjacking).
if (window.top !== window) {
  document.documentElement.innerHTML = "";
  throw new Error("framed");
}

export const invoke = tauriInvoke;

export function esc(s: string) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

let flashTimer = 0;
export function flash(msg: string) {
  let el = document.querySelector<HTMLElement>(".flash");
  if (!el) {
    el = document.createElement("div");
    el.className = "flash";
    document.body.appendChild(el);
  }
  el.textContent = msg;
  el.hidden = false;
  clearTimeout(flashTimer);
  flashTimer = window.setTimeout(() => (el!.hidden = true), 3000);
}

/** Runs a command; shows its error (Korean user message) instead of throwing. */
export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    flash(String(e));
    return undefined;
  }
}
