// Site info bubble (the lock in the address bar), like Chrome's page info: is the connection
// secure, and the site's certificate. Data from `siteinfo.rs`; this window closes on blur.
import { invoke } from "@tauri-apps/api/core";
import { icons } from "../shared/icons";
import { initTheme } from "../shared/theme";
import "./siteinfo.css";

initTheme();

interface Cert {
  subject: string;
  subjectOrg: string | null;
  issuer: string;
  issuerOrg: string | null;
  notBefore: string;
  notAfter: string;
  daysLeft: number;
  serial: string;
  sha256: string;
  sans: string[];
  key: string;
}
interface Info {
  url: string;
  host: string;
  kind: "secure" | "insecure" | "internal" | "local";
  trusted: boolean | null;
  trustNote: string | null;
  source: "page" | "separate" | "";
  mixed: boolean;
  chain: Cert[];
  error: string | null;
}

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
const shield = `<svg class="icon" viewBox="0 0 24 24"><path d="M12 3l7.5 3v5.5c0 4.6-3.2 8.3-7.5 9.5-4.3-1.2-7.5-4.9-7.5-9.5V6z"/><path d="M8.8 12.2l2.2 2.2 4.2-4.4"/></svg>`;
const warn = `<svg class="icon" viewBox="0 0 24 24"><path d="M12 3.5l9.5 16.5h-19z"/><path d="M12 10v4.5M12 17.3v.2"/></svg>`;
const info = `<svg class="icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.5"/><path d="M12 11v5.5M12 7.8v.2"/></svg>`;
const chevron = `<svg class="icon" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6"/></svg>`;

let data: Info | null = null;
let open = false;

function status(d: Info) {
  if (d.kind === "internal") return { cls: "info", icon: info, title: "Kite 내부 페이지", text: "Kite 안에 있는 페이지입니다. 인터넷을 거치지 않습니다." };
  if (d.kind === "local") return { cls: "info", icon: info, title: "로컬 또는 특수 주소", text: "파일이나 특수 주소라서 인증서가 없습니다." };
  if (d.kind === "insecure")
    return { cls: "bad", icon: warn, title: "안전하지 않은 연결", text: "이 사이트와 주고받는 정보는 암호화되지 않습니다. 비밀번호나 카드 번호 같은 민감한 정보를 입력하지 마세요." };
  if (d.trusted === false)
    return { cls: "bad", icon: warn, title: "인증서를 신뢰할 수 없습니다", text: `연결은 암호화되었지만 인증서를 확인하지 못했습니다${d.trustNote ? ` (${esc(d.trustNote)})` : ""}. 회사 보안 장비가 대신 발급한 인증서일 수도 있습니다.` };
  return { cls: "ok", icon: shield, title: "연결이 안전합니다", text: "이 사이트와 주고받는 정보(비밀번호, 카드 번호 등)는 암호화되어 중간에서 볼 수 없습니다." };
}

function render() {
  const d = data;
  if (!d) {
    document.body.innerHTML = `<div class="box"><div class="loading"><span class="spin"></span>확인하는 중…</div></div>`;
    return fit();
  }
  const s = status(d);
  const leaf = d.chain[0];
  const certRow =
    d.kind === "secure" && leaf
      ? `<button class="row toggle${open ? " open" : ""}" id="toggle">${icons.lock}<span><b>${d.trusted === false ? "인증서 문제" : "인증서가 유효함"}</b><small>${esc(leaf.issuerOrg || leaf.issuer)} 발급 · ${leaf.notAfter}까지</small></span>${chevron}</button>`
      : "";
  const detail = open && leaf ? certDetail(d, leaf) : "";
  document.body.innerHTML = `<div class="box">
    <header><b title="${esc(d.url)}">${esc(d.host || d.url)}</b><button class="x" id="close" title="닫기 (Esc)">${icons.close}</button></header>
    <div class="status ${s.cls}"><span class="si">${s.icon}</span><div><b>${s.title}</b><p>${s.text}</p></div></div>
    ${d.mixed ? `<p class="note warn">${warn}이 페이지의 일부 내용(이미지 등)은 암호화되지 않은 http로 받았습니다.</p>` : ""}
    ${d.error ? `<p class="note">${esc(d.error)}</p>` : ""}
    ${certRow}
    ${detail}
  </div>`;
  document.getElementById("close")!.onclick = close;
  document.getElementById("toggle")?.addEventListener("click", () => {
    open = !open;
    render();
  });
  document.getElementById("copy")?.addEventListener("click", async () => {
    if (!leaf) return;
    await invoke("copy_text", { text: leaf.sha256 }).catch(() => {});
    const b = document.getElementById("copy")!;
    b.textContent = "복사됨";
    setTimeout(() => (b.textContent = "복사"), 1200);
  });
  fit();
}

function certDetail(d: Info, c: Cert) {
  const row = (k: string, v: string) => `<div class="kv"><span>${k}</span><div>${v}</div></div>`;
  const left = c.daysLeft < 0 ? `<em class="bad">만료됨</em>` : c.daysLeft <= 14 ? `<em class="warn">${c.daysLeft}일 남음</em>` : `<em>${c.daysLeft}일 남음</em>`;
  const sans = c.sans.length ? `${c.sans.slice(0, 6).map(esc).join("<br>")}${c.sans.length > 6 ? `<br><small>외 ${c.sans.length - 6}개</small>` : ""}` : "—";
  const chain = d.chain.map((x, i) => `<li style="padding-left:${i * 12}px">${i ? "└ " : ""}${esc(x.subject)}</li>`).join("");
  return `<div class="detail">
    ${row("발급 대상", `${esc(c.subject)}${c.subjectOrg ? `<br><small>${esc(c.subjectOrg)}</small>` : ""}`)}
    ${row("발급자", `${esc(c.issuer)}${c.issuerOrg ? `<br><small>${esc(c.issuerOrg)}</small>` : ""}`)}
    ${row("유효 기간", `${c.notBefore} ~ ${c.notAfter} ${left}`)}
    ${row("공개 키", esc(c.key))}
    ${row("대체 이름", sans)}
    ${row("인증서 체인", `<ul class="chain">${chain}</ul>`)}
    ${row("SHA-256 지문", `<code>${esc(c.sha256)}</code><button class="copy" id="copy">복사</button>`)}
    ${row("일련번호", `<code>${esc(c.serial)}</code>`)}
    <p class="src">${d.source === "page" ? "이 페이지가 실제로 받은 인증서입니다." : "Kite가 이 사이트에 따로 연결해 받은 인증서입니다(공개 루트 인증서 기준으로 확인)."}</p>
  </div>`;
}

/** Window to the content's height (then it shows). Not requestAnimationFrame: the window starts
 *  hidden, and hidden windows do not get animation frames. */
function fit() {
  setTimeout(() => invoke("site_info_size", { height: Math.ceil(document.querySelector(".box")!.getBoundingClientRect().height) + 2 }), 0);
}
function close() {
  invoke("site_info_close");
}
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") close();
});

render();
invoke<Info>("site_info").then(
  (d) => {
    data = d;
    render();
  },
  (e) => {
    data = { url: "", host: "", kind: "local", trusted: null, trustNote: null, source: "", mixed: false, chain: [], error: String(e) };
    render();
  },
);
