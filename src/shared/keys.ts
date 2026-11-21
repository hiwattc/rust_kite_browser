// Same combo format as keymap.rs: Ctrl+Alt+Shift+Meta+<KeyboardEvent.code>.
export function combo(e: KeyboardEvent): string {
  let s = "";
  if (e.ctrlKey) s += "Ctrl+";
  if (e.altKey) s += "Alt+";
  if (e.shiftKey) s += "Shift+";
  if (e.metaKey) s += "Meta+";
  return s + e.code;
}
