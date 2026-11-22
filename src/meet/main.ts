// kite://meet — video meetings between Kite browsers (WebRTC mesh), with screen sharing and chat.
//
// "새 회의 만들기": this Kite runs a small meeting server (Rust, `src-tauri/src/meet.rs`, router
// port opened with UPnP when possible) and shows an invite code. Others paste the code, their
// Kite connects and knocks, the host accepts. The server then only relays WebRTC setup and chat;
// audio/video go straight between the members (each pair has its own connection; fine for a
// handful of people). Three transceivers per connection (voice, camera, screen) so screen
// sharing starts and stops without renegotiating.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { call, esc, flash } from "../shared/page";
import { initTheme } from "../shared/theme";
import "./meet.css";

initTheme();
document.title = "영상회의";

const ICE: RTCIceServer[] = [
  { urls: ["stun:stun.l.google.com:19302", "stun:stun1.l.google.com:19302"] },
  { urls: "stun:stun.cloudflare.com:3478" },
];
const NAME_KEY = "kite.meet.name";
/** Voice: mono 48 kHz, echo cancellation on (needed without headphones), lowest capture latency. */
const AUDIO: MediaTrackConstraints & { latency?: number } = { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1, sampleRate: 48000, latency: 0 };
/** Video bitrate caps (per connection), so video never queues voice behind it on a slow uplink. */
const CAM_BPS = 900_000;
const SCREEN_BPS = 2_500_000;

const svg = (d: string) => `<svg class="icon" viewBox="0 0 24 24">${d}</svg>`;
const ic = {
  mic: svg('<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>'),
  micOff: svg('<path d="M15 9.5V6a3 3 0 0 0-5.8-1.1M9 9v2a3 3 0 0 0 4.6 2.5M5.5 11a6.5 6.5 0 0 0 10.8 4.9M18.5 11a6.4 6.4 0 0 1-.5 2.4M12 17.5V21M4 4l16 16"/>'),
  cam: svg('<rect x="3" y="6.5" width="12.5" height="11" rx="2.5"/><path d="M15.5 10.5l5-3v9l-5-3"/>'),
  camOff: svg('<path d="M15.5 13.5l5 3v-9l-5 3M9 6.5h4a2.5 2.5 0 0 1 2.5 2.5v4M15.5 17.5h-10A2.5 2.5 0 0 1 3 15V9a2.5 2.5 0 0 1 1.2-2.1M3 3l18 18"/>'),
  screen: svg('<rect x="3" y="4.5" width="18" height="12" rx="2"/><path d="M8.5 20h7M12 16.5V20M9.5 10.5l2.5-2.5 2.5 2.5M12 8v5.5"/>'),
  chat: svg('<path d="M20 12.5a7.5 7.5 0 0 1-11 6.6L4 20l1-4.3A7.5 7.5 0 1 1 20 12.5z"/>'),
  hang: svg('<path d="M3.5 14.5c4.7-4.2 12.3-4.2 17 0l-2 2.5-3.3-1.3v-2.4a10 10 0 0 0-6.4 0v2.4L5.5 17z"/>'),
  copy: svg('<rect x="8.5" y="8.5" width="11" height="11" rx="2"/><path d="M15.5 8.5V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v7.5a2 2 0 0 0 2 2h2.5"/>'),
  video: svg('<rect x="3" y="6.5" width="12.5" height="11" rx="2.5"/><path d="M15.5 10.5l5-3v9l-5-3"/>'),
  join: svg('<path d="M10 7l5 5-5 5M15 12H4M14 4h4a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-4"/>'),
  people: svg('<circle cx="9" cy="8.5" r="3.2"/><path d="M3 19c.6-3.3 3-5 6-5s5.4 1.7 6 5M16 5.6a3 3 0 0 1 0 5.8M18 14.3c1.6.6 2.7 2.2 3 4.7"/>'),
  add: svg('<circle cx="9" cy="8.5" r="3.2"/><path d="M3 19c.6-3.3 3-5 6-5s5.4 1.7 6 5M18 8v6M15 11h6"/>'),
};

type View = "lobby" | "starting" | "waiting" | "call";
interface Peer {
  id: string;
  name: string;
  pc: RTCPeerConnection;
  senders: { audio?: RTCRtpSender; cam?: RTCRtpSender; screen?: RTCRtpSender };
  cam: MediaStream;
  screen: MediaStream;
  mic: boolean;
  camOn: boolean;
  sharing: boolean;
  state: RTCPeerConnectionState | "new";
  ice: RTCIceCandidateInit[];
}
interface HostInfo {
  code: string;
  seq: number;
  port: number;
  public: string | null;
  lan: string | null;
  blocked: "double-nat" | "no-upnp" | null;
}

let view: View = "lobby";
let role: "host" | "guest" | null = null;
let seq = 0;
let host: HostInfo | null = null;
let myName = (() => {
  try {
    return localStorage.getItem(NAME_KEY) || "";
  } catch {
    return "";
  }
})();
let local: MediaStream | null = null;
let screen: MediaStream | null = null;
let micOn = true;
let camOn = true;
const peers = new Map<string, Peer>();
let knocks: { id: string; name: string }[] = [];
let showInvite = false;
let chatOpen = false;
let unread = 0;
const chat: { from: string; me: boolean; text: string; at: string }[] = [];
let startedAt = 0;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const nameOf = (id: string) => peers.get(id)?.name ?? "참가자";

// ---------------------------------------------------------------- local media

async function ensureLocal() {
  if (local) return;
  const md = navigator.mediaDevices;
  if (!md?.getUserMedia) {
    flash("이 환경에서는 카메라·마이크를 쓸 수 없습니다.");
    local = new MediaStream();
    return;
  }
  try {
    local = await md.getUserMedia({ audio: AUDIO, video: { width: { ideal: 1280 }, height: { ideal: 720 } } });
  } catch {
    try {
      local = await md.getUserMedia({ audio: AUDIO });
      camOn = false;
      flash("카메라를 쓸 수 없어 음성만 연결합니다.");
    } catch {
      local = new MediaStream();
      micOn = camOn = false;
      flash("카메라·마이크 권한이 없어 보고 듣기만 합니다.");
    }
  }
  applyEnabled();
}

const audioTrack = () => local?.getAudioTracks()[0] ?? null;
const camTrack = () => local?.getVideoTracks()[0] ?? null;

function applyEnabled() {
  const a = audioTrack();
  const v = camTrack();
  if (a) a.enabled = micOn;
  if (v) v.enabled = camOn;
}

async function devices() {
  try {
    const list = await navigator.mediaDevices.enumerateDevices();
    return { cams: list.filter((d) => d.kind === "videoinput"), mics: list.filter((d) => d.kind === "audioinput") };
  } catch {
    return { cams: [], mics: [] };
  }
}

async function switchDevice(kind: "audio" | "video", deviceId: string) {
  try {
    const s = await navigator.mediaDevices.getUserMedia(kind === "audio" ? { audio: { ...AUDIO, deviceId: { exact: deviceId } } } : { video: { deviceId: { exact: deviceId }, width: { ideal: 1280 }, height: { ideal: 720 } } });
    const track = kind === "audio" ? s.getAudioTracks()[0] : s.getVideoTracks()[0];
    const old = kind === "audio" ? audioTrack() : camTrack();
    if (old) {
      local!.removeTrack(old);
      old.stop();
    }
    local!.addTrack(track);
    applyEnabled();
    for (const p of peers.values()) await (kind === "audio" ? p.senders.audio : p.senders.cam)?.replaceTrack(track);
    bindVideos();
  } catch {
    flash("장치를 바꾸지 못했습니다.");
  }
}

// ---------------------------------------------------------------- signaling (via Kite's meeting server)

type Data = { k: "sdp"; type: "offer" | "answer"; sdp: string } | { k: "ice"; c: RTCIceCandidateInit } | { k: "state"; mic: boolean; cam: boolean; screen: boolean } | { k: "chat"; text: string };

const sendTo = (to: string, data: Data) => invoke("meet_send", { to, data }).catch(() => {});
const sendAll = (data: Data) => invoke("meet_send", { to: null, data }).catch(() => {});
const myState = (): Data => ({ k: "state", mic: micOn, cam: camOn && !!camTrack(), screen: !!screen });

async function onEvent(e: { seq: number; msg: Record<string, unknown> }) {
  if (e.seq !== seq) return;
  const m = e.msg;
  switch (m.t) {
    case "knock":
      knocks.push({ id: String(m.id), name: String(m.name) });
      flash(`${m.name}님이 참가를 요청했습니다.`);
      return render();
    case "knock-gone":
      knocks = knocks.filter((k) => k.id !== m.id);
      return render();
    case "admit": {
      view = "call";
      startedAt = Date.now();
      render();
      // The newcomer calls everyone already in the meeting.
      for (const p of m.peers as { id: string; name: string }[]) await connect(p.id, p.name, true);
      return;
    }
    case "deny":
      leave(false);
      return flash("참가 요청이 거절되었습니다.");
    case "joined":
      // The invite panel would cover the tiles: out of the way once someone is in (button reopens it).
      if (!peers.size) showInvite = false;
      // They will call us; get ready.
      if (!peers.has(String(m.id))) await connect(String(m.id), String(m.name), false);
      flash(`${m.name}님이 들어왔습니다.`);
      return render();
    case "left": {
      const p = peers.get(String(m.id));
      if (p) flash(`${p.name}님이 나갔습니다.`);
      dropPeer(String(m.id));
      return render();
    }
    case "closed":
      if (role === "guest") {
        const wasWaiting = view === "waiting";
        leave(false);
        flash(wasWaiting ? "회의에 연결하지 못했습니다(회의가 끝났거나 연결이 끊어짐)." : "회의가 끝났습니다.");
      }
      return;
    case "from":
      return onData(String(m.from), m.data as Data);
  }
}

async function onData(from: string, d: Data) {
  if (!d || typeof d !== "object") return;
  if (d.k === "sdp") {
    const p = peers.get(from) ?? (await connect(from, "참가자", false));
    if (d.type === "offer") {
      await p.pc.setRemoteDescription({ type: "offer", sdp: d.sdp });
      const ts = p.pc.getTransceivers();
      for (const t of ts) t.direction = "sendrecv";
      p.senders = { audio: ts[0]?.sender, cam: ts[1]?.sender, screen: ts[2]?.sender };
      await p.senders.audio?.replaceTrack(audioTrack());
      await p.senders.cam?.replaceTrack(camTrack());
      if (screen) await p.senders.screen?.replaceTrack(screen.getVideoTracks()[0] ?? null);
      attachReceivers(p);
      const answer = await p.pc.createAnswer();
      await p.pc.setLocalDescription({ type: "answer", sdp: tuneSdp(answer.sdp ?? "") });
      await tuneSenders(p);
      sendTo(from, { k: "sdp", type: "answer", sdp: p.pc.localDescription!.sdp });
    } else {
      await p.pc.setRemoteDescription({ type: "answer", sdp: d.sdp });
    }
    for (const c of p.ice.splice(0)) await p.pc.addIceCandidate(c).catch(() => {});
  } else if (d.k === "ice") {
    const p = peers.get(from);
    if (!p) return;
    if (p.pc.remoteDescription) await p.pc.addIceCandidate(d.c).catch(() => {});
    else p.ice.push(d.c);
  } else if (d.k === "state") {
    const p = peers.get(from);
    if (!p) return;
    const changed = p.mic !== !!d.mic || p.camOn !== !!d.cam || p.sharing !== !!d.screen;
    Object.assign(p, { mic: !!d.mic, camOn: !!d.cam, sharing: !!d.screen });
    if (changed && view === "call") renderCall();
  } else if (d.k === "chat") {
    chat.push({ from: nameOf(from), me: false, text: String(d.text).slice(0, 2000), at: clock() });
    if (!chatOpen) unread++;
    renderChat();
  }
}

/** A connection to one member. `call`: we make the offer (we are the newcomer). */
async function connect(id: string, name: string, call: boolean): Promise<Peer> {
  dropPeer(id);
  const pc = new RTCPeerConnection({ iceServers: ICE });
  const p: Peer = { id, name, pc, senders: {}, cam: new MediaStream(), screen: new MediaStream(), mic: true, camOn: true, sharing: false, state: "new", ice: [] };
  peers.set(id, p);
  pc.onicecandidate = (e) => {
    if (e.candidate) sendTo(id, { k: "ice", c: e.candidate.toJSON() });
  };
  pc.onconnectionstatechange = () => {
    p.state = pc.connectionState;
    if (p.state === "connected") sendTo(id, myState());
    if (view === "call") renderCall();
  };
  if (call) {
    const ta = pc.addTransceiver("audio", { direction: "sendrecv" });
    const tc = pc.addTransceiver("video", { direction: "sendrecv" });
    const tsc = pc.addTransceiver("video", { direction: "sendrecv" });
    p.senders = { audio: ta.sender, cam: tc.sender, screen: tsc.sender };
    await ta.sender.replaceTrack(audioTrack());
    await tc.sender.replaceTrack(camTrack());
    if (screen) await tsc.sender.replaceTrack(screen.getVideoTracks()[0] ?? null);
    attachReceivers(p);
    const offer = await pc.createOffer();
    await pc.setLocalDescription({ type: "offer", sdp: tuneSdp(offer.sdp ?? "") });
    await tuneSenders(p);
    sendTo(id, { k: "sdp", type: "offer", sdp: pc.localDescription!.sdp });
  }
  if (view === "call") renderCall();
  return p;
}

function dropPeer(id: string) {
  const p = peers.get(id);
  if (!p) return;
  try {
    p.pc.close();
  } catch {
    /* closed */
  }
  peers.delete(id);
  lastJb.delete(id);
  audioFor(id, false);
}

/** Their tracks. Voice plays in its own <audio> element (in the <video> with the picture it would
 *  wait for the slower video to stay in sync), kept outside the redrawn page. */
function attachReceivers(p: Peer) {
  const ts = p.pc.getTransceivers();
  const a = ts[0]?.receiver.track;
  const c = ts[1]?.receiver.track;
  const s = ts[2]?.receiver.track;
  p.cam = new MediaStream(c ? [c] : []);
  p.screen = new MediaStream(s ? [s] : []);
  const el = audioFor(p.id, true)!;
  el.srcObject = a ? new MediaStream([a]) : null;
  el.play().catch(() => {});
  // Smallest receive (jitter) buffer the engine allows for voice.
  const ar = ts[0]?.receiver as (RTCRtpReceiver & { jitterBufferTarget?: number | null }) | undefined;
  try {
    if (ar && "jitterBufferTarget" in ar) ar.jitterBufferTarget = 0;
  } catch {
    /* engine default */
  }
}

function audioFor(id: string, make: boolean): HTMLAudioElement | null {
  const aid = `audio-${id}`;
  let a = document.getElementById(aid) as HTMLAudioElement | null;
  if (!make) {
    if (a) {
      a.srcObject = null;
      a.remove();
    }
    return null;
  }
  if (!a) {
    a = document.createElement("audio");
    a.id = aid;
    a.autoplay = true;
    document.documentElement.append(a);
  }
  return a;
}

/** Voice first: high priority for audio, bitrate caps for camera and screen. */
async function tuneSenders(p: Peer) {
  const set = async (sn: RTCRtpSender | undefined, enc: Record<string, unknown>, pref?: string) => {
    if (!sn) return;
    try {
      const params = sn.getParameters();
      if (!params.encodings?.length) params.encodings = [{}];
      Object.assign(params.encodings[0], enc);
      if (pref) (params as { degradationPreference?: string }).degradationPreference = pref;
      await sn.setParameters(params);
    } catch {
      /* engine defaults */
    }
  };
  await set(p.senders.audio, { priority: "high", networkPriority: "high", maxBitrate: 64_000 });
  await set(p.senders.cam, { priority: "low", networkPriority: "low", maxBitrate: CAM_BPS }, "balanced");
  // Text on a shared screen must stay sharp: drop frames, not resolution.
  await set(p.senders.screen, { priority: "medium", networkPriority: "low", maxBitrate: SCREEN_BPS }, "maintain-resolution");
}

/** Opus: 10 ms packets (default 20), no DTX (it clips the start of words), in-band FEC, mono. */
function tuneSdp(sdp: string) {
  const pt = sdp.match(/a=rtpmap:(\d+) opus\/48000/i)?.[1];
  if (!pt) return sdp;
  const want = { minptime: "10", useinbandfec: "1", usedtx: "0", stereo: "0", "sprop-stereo": "0", maxplaybackrate: "48000" };
  const lines = sdp.split("\r\n");
  const i = lines.findIndex((l) => l.startsWith(`a=fmtp:${pt} `));
  const merge = (params: string) => {
    const kv = new Map(params.split(";").filter(Boolean).map((x) => x.split("=") as [string, string]));
    for (const [k, v] of Object.entries(want)) kv.set(k, v);
    return [...kv].map(([k, v]) => `${k}=${v}`).join(";");
  };
  if (i >= 0) lines[i] = `a=fmtp:${pt} ${merge(lines[i].slice(`a=fmtp:${pt} `.length))}`;
  else lines.splice(lines.findIndex((l) => l.startsWith(`a=rtpmap:${pt} `)) + 1, 0, `a=fmtp:${pt} ${merge("")}`);
  const audioAt = lines.findIndex((l) => l.startsWith("m=audio"));
  if (audioAt >= 0) {
    const end = lines.findIndex((l, j) => j > audioAt && l.startsWith("m="));
    const section = lines.slice(audioAt, end < 0 ? undefined : end);
    if (!section.some((l) => l.startsWith("a=ptime:"))) lines.splice(end < 0 ? lines.length - 1 : end, 0, "a=ptime:10");
  }
  return lines.join("\r\n");
}

// ---------------------------------------------------------------- start / join / leave

async function startHost() {
  await ensureLocal();
  role = "host";
  view = "starting";
  render();
  const info = await call<HostInfo>("meet_host", { name: myName || "호스트" });
  if (!info) {
    role = null;
    view = "lobby";
    return render();
  }
  host = info;
  seq = info.seq;
  view = "call";
  showInvite = true;
  startedAt = Date.now();
  render();
}

async function join(code: string) {
  if (!code.trim()) return flash("초대 코드를 붙여 넣으세요.");
  await ensureLocal();
  role = "guest";
  view = "waiting";
  render();
  const s = await call<number>("meet_join", { code, name: myName || "참가자" });
  if (s === undefined) {
    role = null;
    view = "lobby";
    return render();
  }
  seq = s;
}

function leave(tell: boolean) {
  if (tell || role) invoke("meet_leave").catch(() => {});
  stopScreen(false);
  for (const id of [...peers.keys()]) dropPeer(id);
  role = null;
  seq = 0;
  host = null;
  knocks = [];
  showInvite = false;
  chat.length = 0;
  unread = 0;
  chatOpen = false;
  view = "lobby";
  document.title = "영상회의";
  render();
}

async function admit(id: string, accept: boolean) {
  knocks = knocks.filter((k) => k.id !== id);
  await call("meet_admit", { id, accept });
  render();
}

// ---------------------------------------------------------------- controls

function toggleMic() {
  micOn = !micOn;
  applyEnabled();
  sendAll(myState());
  render();
}
function toggleCam() {
  if (!camTrack()) return flash("카메라가 없습니다.");
  camOn = !camOn;
  applyEnabled();
  sendAll(myState());
  render();
}

async function toggleScreen() {
  if (screen) return stopScreen(true);
  if (!navigator.mediaDevices?.getDisplayMedia) return flash("이 환경에서는 화면 공유를 쓸 수 없습니다.");
  try {
    screen = await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: { ideal: 15, max: 30 } }, audio: false });
  } catch {
    return flash("화면 공유를 시작하지 못했습니다. (macOS: 시스템 설정 → 개인정보 보호 및 보안 → 화면 기록 권한)");
  }
  const t = screen.getVideoTracks()[0];
  if ("contentHint" in t) t.contentHint = "detail";
  t.onended = () => stopScreen(true);
  for (const p of peers.values()) await p.senders.screen?.replaceTrack(t);
  sendAll(myState());
  render();
}

function stopScreen(update: boolean) {
  if (!screen) return;
  screen.getTracks().forEach((t) => t.stop());
  screen = null;
  for (const p of peers.values()) p.senders.screen?.replaceTrack(null).catch(() => {});
  if (update) {
    sendAll(myState());
    render();
  }
}

function sendChat(text: string) {
  const t = text.trim();
  if (!t) return;
  sendAll({ k: "chat", text: t });
  chat.push({ from: "나", me: true, text: t, at: clock() });
  renderChat();
}

function clock() {
  const d = new Date();
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

async function copyCode() {
  if (host && (await call("copy_text", { text: host.code })) !== undefined) flash("초대 코드를 복사했습니다. 메신저나 메일로 보내세요.");
}

// ---------------------------------------------------------------- views

function render() {
  if (view === "call") return renderCall();
  const panel =
    view === "starting"
      ? `<div class="steps"><div class="code wait"><span class="spin"></span>회의를 여는 중… (공유기 포트 확인)</div></div>`
      : view === "waiting"
        ? `<div class="steps"><div class="step"><span class="no"><span class="spin"></span></span><div class="body"><b>참가를 요청했습니다</b><small>회의를 연 사람이 수락하면 바로 들어갑니다.</small>
            <div class="row-btns"><button class="btn" id="cancel">취소</button></div></div></div></div>`
        : `<div class="choices">
          <button class="choice" id="host">${ic.video}<b>새 회의 만들기</b><small>이 컴퓨터에서 회의를 열고 초대 코드를 보냅니다. 들어오려는 사람은 내가 수락해야 들어옵니다.</small></button>
          <div class="choice join">${ic.join}<b>초대 코드로 참여</b><small>받은 초대 코드를 붙여 넣으면 회의를 연 사람에게 참가 요청이 갑니다</small>
            <textarea id="invite" spellcheck="false" placeholder="KITE-ROOM1.…"></textarea>
            <button class="btn primary" id="join">참가 요청</button></div>
        </div>`;
  document.body.innerHTML = `
  <div class="meet">
    <header class="top"><h1>${ic.video}영상회의</h1><span class="sub">초대 코드 · 수락 후 입장 · 여러 명 · 화면 공유</span></header>
    <div class="lobby">
      <section class="preview">
        <div class="tile self big">${videoHtml("self-video", true)}${!camOn || !camTrack() ? `<div class="off">${initial(myName || "나")}</div>` : ""}</div>
        <div class="dev">
          <input id="name" maxlength="40" placeholder="내 이름 (다른 사람에게 보임)" value="${esc(myName)}" />
          <button class="round ${micOn ? "" : "off"}" id="mic" title="마이크">${micOn ? ic.mic : ic.micOff}</button>
          <button class="round ${camOn ? "" : "off"}" id="cam" title="카메라">${camOn ? ic.cam : ic.camOff}</button>
        </div>
        <div class="dev selects" id="selects"></div>
      </section>
      <section class="panel">${panel}
        <p class="note">회의를 연 컴퓨터의 Kite가 참가 요청과 연결 정보만 중계하고(초대 코드의 비밀 키로 암호화), 영상·음성은 참가자 컴퓨터끼리 암호화되어 직접 오갑니다. 연결할 주소를 찾을 때만 공개 STUN 서버(Google·Cloudflare)를 씁니다. 초대 코드에는 회의를 연 컴퓨터의 IP 주소가 들어 있으니 참가할 사람에게만 보내세요.</p>
      </section>
    </div>
  </div>`;
  bindLobby();
  bindVideos();
  fillDevices();
}

function videoHtml(id: string, mirror = false) {
  return `<video id="${id}" autoplay playsinline muted class="${mirror ? "mirror" : ""}"></video>`;
}

const initial = (n: string) => `<span>${esc((n.trim()[0] ?? "?").toUpperCase())}</span>`;

function stateNote(p: Peer) {
  if (p.state === "connected") return "";
  if (p.state === "failed") return `<div class="conn bad">연결 실패 — 서로 직접 연결이 막힌 네트워크입니다</div>`;
  return `<div class="conn"><span class="spin"></span>연결하는 중…</div>`;
}

function tile(p: Peer) {
  return `<div class="tile" data-peer="${esc(p.id)}">${videoHtml(`cam-${p.id}`)}${!p.camOn ? `<div class="off">${initial(p.name)}</div>` : ""}${stateNote(p)}<div class="tag">${p.mic ? "" : ic.micOff}${esc(p.name)}${p.id === "host" ? `<i class="host-mark">호스트</i>` : ""}</div></div>`;
}

function selfTile() {
  return `<div class="tile">${videoHtml("self-video", true)}${!camOn || !camTrack() ? `<div class="off">${initial(myName || "나")}</div>` : ""}<div class="tag">${micOn ? "" : ic.micOff}나${role === "host" ? `<i class="host-mark">호스트</i>` : ""}</div></div>`;
}

function invitePanel() {
  if (role !== "host" || !host) return "";
  const reach = host.public
    ? `<p class="reach ok">외부에서 접속 가능 · 공인 주소 ${esc(host.public)} (공유기 포트를 자동으로 열었습니다)</p>`
    : host.blocked === "double-nat"
      ? `<p class="reach warn">같은 네트워크(사무실·집)에서만 접속할 수 있습니다. 이 공유기가 다른 공유기나 통신사 장비 뒤에 있어(이중 NAT) 바깥에서는 들어올 수 없습니다. 바깥쪽 장비(모뎀·회사 방화벽)에서 TCP ${host.port} 포트를 이 공유기로 포워딩해야 합니다.</p>`
      : `<p class="reach warn">같은 네트워크(사무실·집)에서만 접속할 수 있습니다. 다른 곳의 사람이 들어오려면 공유기 설정에서 UPnP를 켜거나 TCP ${host.port} 포트를 이 컴퓨터${host.lan ? `(${esc(host.lan.split(":")[0])})` : ""}로 포워딩하세요.</p>`;
  return `<div class="invite-card">
      <header><b>${ic.add}초대하기</b><button class="x" id="invite-close" title="닫기">✕</button></header>
      <p class="hint">이 코드를 참가할 사람에게 보내세요. 들어오려는 사람은 여기서 수락해야 들어옵니다.</p>
      <div class="code"><textarea readonly id="mycode">${esc(host.code)}</textarea><button class="btn primary" id="copy">${ic.copy}복사</button></div>
      ${reach}
    </div>`;
}

function knockCards() {
  return knocks
    .map(
      (k) => `<div class="knock" data-id="${esc(k.id)}"><span class="av">${initial(k.name)}</span><div class="who"><b>${esc(k.name)}</b><small>참가를 요청했습니다</small></div>
      <button class="btn" data-act="deny">거절</button><button class="btn primary" data-act="accept">수락</button></div>`,
    )
    .join("");
}

function renderCall() {
  const list = [...peers.values()];
  const sharer = list.find((p) => p.sharing);
  const count = list.length + 1;
  const keepChat = (document.getElementById("chat-input") as HTMLInputElement | null)?.value ?? "";
  document.title = `영상회의 · ${count}명`;
  const alone = !list.length;
  const stage = sharer
    ? `<div class="spot"><div class="tile main">${videoHtml(`screen-${sharer.id}`)}<div class="tag">${ic.screen}${esc(sharer.name)}님의 화면</div></div>
        <div class="strip">${list.map(tile).join("")}${selfTile()}${screen ? `<div class="tile">${videoHtml("self-screen")}<div class="tag">${ic.screen}내 화면</div></div>` : ""}</div></div>`
    : `<div class="grid n${Math.min(count + (screen ? 1 : 0), 9)}">${list.map(tile).join("")}${selfTile()}${screen ? `<div class="tile">${videoHtml("self-screen")}<div class="tag">${ic.screen}내 화면 공유 중</div></div>` : ""}</div>`;
  document.body.innerHTML = `
  <div class="call ${chatOpen ? "with-chat" : ""}">
    <div class="stage">
      ${stage}
      ${alone ? `<div class="alone">${role === "host" ? "아직 아무도 없습니다. 초대 코드를 보내고, 참가 요청이 오면 수락하세요." : "연결하는 중…"}</div>` : ""}
      <div class="top-left"><span class="timer" id="timer"></span><span class="net" id="net" hidden></span><span class="count">${ic.people}${count}명</span></div>
      <div class="side-cards">${knockCards()}${showInvite ? invitePanel() : ""}</div>
    </div>
    <aside class="chat">
      <header>채팅<button class="x" id="chat-close" title="닫기">✕</button></header>
      <div class="msgs" id="msgs"></div>
      <form id="chat-form"><input id="chat-input" autocomplete="off" placeholder="모두에게 메시지 보내기" /><button class="btn primary">보내기</button></form>
    </aside>
    <nav class="controls">
      <button class="round ${micOn ? "" : "off"}" id="mic" title="마이크 ${micOn ? "끄기" : "켜기"}">${micOn ? ic.mic : ic.micOff}</button>
      <button class="round ${camOn ? "" : "off"}" id="cam" title="카메라 ${camOn ? "끄기" : "켜기"}">${camOn ? ic.cam : ic.camOff}</button>
      <button class="round ${screen ? "on" : ""}" id="share" title="${screen ? "화면 공유 중지" : "화면 공유"}">${ic.screen}</button>
      <button class="round ${chatOpen ? "on" : ""}" id="chat" title="채팅">${ic.chat}${unread ? `<i class="badge">${unread}</i>` : ""}</button>
      ${role === "host" ? `<button class="round ${showInvite ? "on" : ""}" id="invite-btn" title="초대하기">${ic.add}${knocks.length ? `<i class="badge">${knocks.length}</i>` : ""}</button>` : ""}
      <button class="round hang" id="hang" title="${role === "host" ? "회의 끝내기 (모두 나가짐)" : "나가기"}">${ic.hang}</button>
    </nav>
  </div>`;
  bindVideos();
  renderChat();
  $("mic").onclick = toggleMic;
  $("cam").onclick = toggleCam;
  $("share").onclick = () => toggleScreen();
  $("chat").onclick = () => {
    chatOpen = !chatOpen;
    unread = 0;
    renderCall();
  };
  $("chat-close").onclick = () => {
    chatOpen = false;
    renderCall();
  };
  $("hang").onclick = () => {
    const wasHost = role === "host";
    leave(true);
    flash(wasHost ? "회의를 끝냈습니다." : "회의에서 나왔습니다.");
  };
  document.getElementById("invite-btn")?.addEventListener("click", () => {
    showInvite = !showInvite;
    renderCall();
  });
  document.getElementById("invite-close")?.addEventListener("click", () => {
    showInvite = false;
    renderCall();
  });
  document.getElementById("copy")?.addEventListener("click", copyCode);
  const mine = document.getElementById("mycode") as HTMLTextAreaElement | null;
  mine?.addEventListener("focus", () => mine.select());
  document.querySelectorAll<HTMLElement>(".knock").forEach((k) => {
    k.querySelector<HTMLElement>('[data-act="accept"]')!.onclick = () => admit(k.dataset.id!, true);
    k.querySelector<HTMLElement>('[data-act="deny"]')!.onclick = () => admit(k.dataset.id!, false);
  });
  const input = $<HTMLInputElement>("chat-input");
  input.value = keepChat;
  $<HTMLFormElement>("chat-form").onsubmit = (e) => {
    e.preventDefault();
    sendChat(input.value);
    input.value = "";
    input.focus();
  };
  if (chatOpen && keepChat) input.focus();
  tick();
}

/** New chat message: only the list and the badge change (videos untouched). */
function renderChat() {
  const msgs = document.getElementById("msgs");
  const btn = document.getElementById("chat");
  if (view !== "call" || !msgs || !btn) return;
  msgs.innerHTML =
    chat.map((m) => `<div class="msg ${m.me ? "me" : ""}">${m.me ? "" : `<em>${esc(m.from)}</em>`}<span>${esc(m.text)}</span><small>${m.at}</small></div>`).join("") ||
    `<p class="empty-chat">메시지는 회의 참가자에게만 갑니다.</p>`;
  msgs.scrollTop = msgs.scrollHeight;
  btn.querySelector(".badge")?.remove();
  if (unread) btn.insertAdjacentHTML("beforeend", `<i class="badge">${unread}</i>`);
}

function bindVideos() {
  const set = (id: string, s: MediaStream | null) => {
    const v = document.getElementById(id) as HTMLVideoElement | null;
    if (v && s && v.srcObject !== s) {
      v.srcObject = s;
      v.play().catch(() => {});
    }
  };
  set("self-video", local);
  set("self-screen", screen);
  for (const p of peers.values()) {
    set(`cam-${p.id}`, p.cam);
    set(`screen-${p.id}`, p.screen);
  }
}

function bindLobby() {
  const name = $<HTMLInputElement>("name");
  name.oninput = () => {
    myName = name.value.trim();
    try {
      localStorage.setItem(NAME_KEY, myName);
    } catch {
      /* fine */
    }
  };
  $("mic").onclick = toggleMic;
  $("cam").onclick = toggleCam;
  document.getElementById("host")?.addEventListener("click", () => startHost());
  document.getElementById("join")?.addEventListener("click", () => join($<HTMLTextAreaElement>("invite").value));
  document.getElementById("cancel")?.addEventListener("click", () => leave(true));
}

async function fillDevices() {
  const box = document.getElementById("selects");
  if (!box || !local) return;
  const { cams, mics } = await devices();
  const pick = (list: MediaDeviceInfo[], cur: MediaStreamTrack | null, id: string, label: string) =>
    list.length > 1
      ? `<label>${label}<select id="${id}">${list.map((d, i) => `<option value="${esc(d.deviceId)}" ${cur?.getSettings().deviceId === d.deviceId ? "selected" : ""}>${esc(d.label || `${label} ${i + 1}`)}</option>`).join("")}</select></label>`
      : "";
  box.innerHTML = pick(cams, camTrack(), "cam-dev", "카메라") + pick(mics, audioTrack(), "mic-dev", "마이크");
  (document.getElementById("cam-dev") as HTMLSelectElement | null)?.addEventListener("change", (e) => switchDevice("video", (e.target as HTMLSelectElement).value));
  (document.getElementById("mic-dev") as HTMLSelectElement | null)?.addEventListener("change", (e) => switchDevice("audio", (e.target as HTMLSelectElement).value));
}

// ---------------------------------------------------------------- timer and network stats

const lastJb = new Map<string, { delay: number; count: number }>();
/** Worst round trip and average voice buffer over the connections. */
async function netStats() {
  const el = document.getElementById("net");
  if (!el || !peers.size) return;
  let rtt = -1;
  const jbs: number[] = [];
  for (const p of peers.values()) {
    try {
      const stats = await p.pc.getStats();
      stats.forEach((r) => {
        if (r.type === "candidate-pair" && r.state === "succeeded" && typeof r.currentRoundTripTime === "number") rtt = Math.max(rtt, r.currentRoundTripTime * 1000);
        if (r.type === "inbound-rtp" && r.kind === "audio" && typeof r.jitterBufferDelay === "number" && r.jitterBufferEmittedCount) {
          const last = lastJb.get(p.id) ?? { delay: 0, count: 0 };
          const n = r.jitterBufferEmittedCount - last.count;
          jbs.push(n > 0 ? ((r.jitterBufferDelay - last.delay) / n) * 1000 : (r.jitterBufferDelay / r.jitterBufferEmittedCount) * 1000);
          lastJb.set(p.id, { delay: r.jitterBufferDelay, count: r.jitterBufferEmittedCount });
        }
      });
    } catch {
      /* closed */
    }
  }
  const jb = jbs.length ? jbs.reduce((a, b) => a + b, 0) / jbs.length : -1;
  const parts = [rtt >= 0 ? `왕복 ${Math.round(rtt)}ms` : "", jb >= 0 ? `음성 버퍼 ${Math.round(jb)}ms` : ""].filter(Boolean);
  el.textContent = parts.join(" · ");
  el.hidden = !parts.length;
  const total = Math.max(rtt, 0) / 2 + Math.max(jb, 0);
  el.className = `net ${total > 250 ? "bad" : total > 120 ? "fair" : "good"}`;
}

let timerOn = false;
let statTick = 0;
function tick() {
  const el = document.getElementById("timer");
  if (el && startedAt) {
    const s = Math.floor((Date.now() - startedAt) / 1000);
    el.textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }
  if (++statTick % 2 === 0) netStats();
  if (!timerOn && view === "call") {
    timerOn = true;
    setTimeout(() => {
      timerOn = false;
      if (view === "call") tick();
    }, 1000);
  }
}

window.addEventListener("beforeunload", () => {
  if (role) invoke("meet_leave").catch(() => {});
});

getCurrentWebview()
  .listen<{ seq: number; msg: Record<string, unknown> }>("meet", (e) => onEvent(e.payload))
  .catch(() => {});
render();
ensureLocal().then(() => {
  if (view === "lobby") render();
});
