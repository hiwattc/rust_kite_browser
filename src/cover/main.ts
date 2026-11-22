// Stand-in over the page area while a tab whose page process macOS ended reloads: its last
// picture, so the tab looks kept (Rust shows/hides this webview; see `Browser::revive`).
import { getCurrentWebview } from "@tauri-apps/api/webview";

const wv = getCurrentWebview();
document.body.innerHTML = `<img id="shot" alt="" /><div class="pill"><span class="spin"></span>다시 불러오는 중</div>`;
const style = document.createElement("style");
style.textContent = `
  html, body { margin: 0; height: 100%; overflow: hidden; background: #fff; }
  #shot { position: fixed; inset: 0; width: 100%; height: 100%; object-fit: cover; object-position: top left; }
  body { transition: opacity .16s ease; }
  body.out { opacity: 0; }
  .pill { position: fixed; top: 12px; right: 14px; display: flex; align-items: center; gap: 7px; padding: 5px 11px; border-radius: 14px;
    background: rgba(20, 22, 28, .72); color: #fff; font: 600 11.5px -apple-system, "Segoe UI", sans-serif; opacity: 0; animation: show .2s ease .5s forwards; }
  .spin { width: 11px; height: 11px; border-radius: 50%; border: 2px solid rgba(255,255,255,.35); border-top-color: #fff; animation: spin .8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @keyframes show { to { opacity: 1; } }`;
document.head.append(style);
const img = document.getElementById("shot") as HTMLImageElement;

wv.listen<string>("cover", (e) => {
  document.body.classList.remove("out");
  img.src = e.payload;
}).catch(() => {});
wv.listen("cover-hide", () => document.body.classList.add("out")).catch(() => {});
