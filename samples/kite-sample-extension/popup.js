// Popup: a counter kept in storage.local, and a quick check of the extension APIs.
const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);
const mark = (id, ok, note) => {
  $(id).classList.add(ok ? "ok" : "fail");
  if (note) $(id).append(` — ${note}`);
};

async function load() {
  try {
    const { count = 0 } = await api.storage.local.get("count");
    $("n").textContent = count;
    mark("c-storage", true);
  } catch (e) {
    $("n").textContent = "?";
    mark("c-storage", false, String(e && e.message || e));
  }
}

async function set(n) {
  await api.storage.local.set({ count: n });
  $("n").textContent = n;
}

mark("c-api", !!(api && api.runtime && api.runtime.id), api && api.runtime ? `id ${api.runtime.id.slice(0, 8)}…` : "없음");
$("inc").onclick = async () => set(Number($("n").textContent || 0) + 1);
$("reset").onclick = () => set(0);
load();
