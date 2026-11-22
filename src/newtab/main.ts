// New-tab page, shown inside a content tab. It may only call `content_event` (like any web page).
import { invoke } from "@tauri-apps/api/core";
import "./newtab.css";
import { initTheme } from "../shared/theme";

initTheme();

const isMac = navigator.platform.toLowerCase().includes("mac");
const k = (s: string) => `<kbd>${s}</kbd>`;
const m = isMac ? "⌘" : "Ctrl";

document.title = "새 탭";
document.body.innerHTML = `
  <main>
    <div class="logo"><span>Kite</span></div>
    <button class="search" id="open">
      <span>탭 · 북마크 · 기록 검색, 또는 주소 입력</span>
      ${k("Ctrl")}${k("Shift")}${k("P")}
    </button>
    <ul class="tips">
      <li>${k(">")} 명령 실행 &nbsp; ${k("*")} 북마크 &nbsp; ${k("#")} 방문 기록 &nbsp; ${k("@")} 열린 탭</li>
      <li>${k(m)}${k("L")} 주소창 &nbsp; ${k(m)}${k("T")} 새 탭 &nbsp; ${k(m)}${k("D")} 북마크</li>
      <li>명령 팔레트에서 <b>크롬 북마크 가져오기</b>, <b>Claude Code 연결 명령 복사</b></li>
    </ul>
  </main>
`;

document.getElementById("open")!.addEventListener("click", () => {
  invoke("content_event", { kind: "shortcut", action: "palette" }).catch(() => {});
});
