// kite://neterror — shown when a page cannot be reached (see `src-tauri/src/neterror.rs`):
// what went wrong and what to do, over a small low-poly 3D scene (three.js) of a desert island
// where someone reads under a beach umbrella. Everything is built from primitives in code.
import * as THREE from "three";
import { invoke } from "@tauri-apps/api/core";
import { THEMES, initTheme } from "../shared/theme";
import "./neterror.css";

initTheme();

// ---------------------------------------------------------------- what happened

const params = new URLSearchParams(location.search);
const failed = params.get("u") ?? "";
const code = (params.get("r") ?? "unknown") as Code;
const detail = params.get("d") ?? "";
type Code = "offline" | "dns" | "refused" | "timeout" | "tls" | "cert" | "unknown";

const host = (() => {
  try {
    return new URL(failed).host || failed;
  } catch {
    return failed;
  }
})();
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
const h = `<b>${esc(host)}</b>`;

const TEXT: Record<Code, { title: string; lead: string; tips: string[]; err: string }> = {
  offline: {
    title: "인터넷에 연결되어 있지 않아요",
    lead: "잠시 무인도에서 쉬어 가세요. 연결이 돌아오면 이 페이지를 자동으로 다시 불러옵니다.",
    tips: ["Wi-Fi나 유선 랜이 연결되어 있는지 확인하세요.", "공유기나 모뎀을 껐다 켜 보세요.", "VPN을 쓰고 있다면 연결 상태를 확인하세요."],
    err: "KITE_INTERNET_DISCONNECTED",
  },
  dns: {
    title: `${h} 주소를 찾을 수 없어요`,
    lead: "주소에 오타가 없는지 확인해 보세요. 이름에 해당하는 서버를 찾지 못했습니다.",
    tips: ["주소의 철자를 확인하세요.", "회사·학교 안에서만 열리는 주소라면 VPN에 연결되어 있는지 확인하세요.", "다른 사이트는 열리는지 확인해 보세요."],
    err: "KITE_NAME_NOT_RESOLVED",
  },
  refused: {
    title: `${h}에서 연결을 거부했어요`,
    lead: "서버가 꺼져 있거나 이 주소(포트)로 오는 연결을 받지 않습니다.",
    tips: ["주소와 포트 번호를 확인하세요.", "개발 중인 서버라면 실행 중인지 확인하세요.", "방화벽이나 프록시 설정을 확인하세요."],
    err: "KITE_CONNECTION_REFUSED",
  },
  timeout: {
    title: `${h}에서 응답이 없어요`,
    lead: "서버가 너무 오래 응답하지 않았습니다. 잠시 뒤 다시 시도해 보세요.",
    tips: ["잠시 뒤 다시 시도하세요.", "네트워크가 느리거나 끊겼는지 확인하세요.", "회사망이라면 이 사이트가 막혀 있을 수 있습니다."],
    err: "KITE_CONNECTION_TIMED_OUT",
  },
  tls: {
    title: "보안 연결을 만들 수 없어요",
    lead: `${h}와 암호화된 연결(HTTPS)을 맺지 못했습니다.`,
    tips: ["컴퓨터의 날짜와 시간이 맞는지 확인하세요.", "보안 프로그램이나 프록시가 연결을 가로채고 있지 않은지 확인하세요.", "http:// 주소로 열리는 사이트인지 확인하세요."],
    err: "KITE_SSL_PROTOCOL_ERROR",
  },
  cert: {
    title: "이 사이트의 인증서를 확인할 수 없어요",
    lead: `${h}의 인증서를 신뢰할 수 없습니다. 비밀번호나 카드 번호 같은 정보는 입력하지 마세요.`,
    tips: ["주소가 정확한지 확인하세요.", "회사·학교 보안 장비가 대신 발급한 인증서일 수 있습니다.", "컴퓨터의 날짜와 시간을 확인하세요."],
    err: "KITE_CERT_INVALID",
  },
  unknown: {
    title: "이 페이지에 연결할 수 없어요",
    lead: `${h}에서 페이지를 받지 못했습니다.`,
    tips: ["잠시 뒤 다시 시도하세요.", "주소가 정확한지 확인하세요."],
    err: "KITE_FAILED",
  },
};
const t = TEXT[code] ?? TEXT.unknown;
/** Why the certificate failed, in words (rustls reports it in English). */
function certWhy(d: string) {
  const after = /not valid after (\d+)/.exec(d);
  if (/expired/i.test(d)) return `인증서가 만료되었습니다${after ? ` (${new Date(Number(after[1]) * 1000).toLocaleDateString("ko-KR")}까지 유효)` : ""}`;
  if (/not valid before|not yet valid/i.test(d)) return "인증서의 유효 기간이 아직 시작되지 않았습니다";
  if (/unknown ?issuer|self.?signed/i.test(d)) return "신뢰할 수 있는 기관이 발급하지 않은 인증서입니다(자체 서명 등)";
  if (/not valid for name|NotValidForName|hostname/i.test(d)) return "다른 주소용으로 발급된 인증서입니다";
  if (/revoked/i.test(d)) return "취소된 인증서입니다";
  return "";
}
const why = code === "cert" ? certWhy(detail) : "";
if (why) t.lead = `${h}: ${why}. 비밀번호나 카드 번호 같은 정보는 입력하지 마세요.`;
document.title = `${host || "페이지"} — 연결할 수 없음`;

document.body.innerHTML = `
  <canvas id="scene"></canvas>
  <main class="card" role="alert">
    <div class="badge"><span class="dot"></span>연결 쉬는 중</div>
    <h1>${t.title}</h1>
    <p class="lead">${t.lead}</p>
    ${failed ? `<div class="url" title="${esc(failed)}">${esc(failed)}</div>` : ""}
    <div class="actions">
      <button class="btn primary" id="retry">다시 시도</button>
      ${host ? `<button class="btn" id="search">‘${esc(host)}’ 검색</button>` : ""}
    </div>
    ${
      code === "cert" && host
        ? `<details class="advanced"><summary>고급</summary>
      <p>Kite가 이 인증서를 확인하지 못했습니다(만료되었거나, 스스로 서명했거나, 다른 주소용일 수 있음). 누군가 연결을 가로채고 있을 수도 있습니다.
      사이트를 믿을 수 있을 때만 계속하세요. 이 선택은 Kite를 다시 시작할 때까지 유지됩니다.</p>
      <button class="proceed" id="proceed">‘${esc(host)}’(으)로 이동 (안전하지 않음)</button></details>`
        : ""
    }
    <details><summary>해결해 볼 방법</summary><ul>${t.tips.map((x) => `<li>${x}</li>`).join("")}</ul></details>
    <p class="code">${t.err}${detail ? ` · ${esc(detail)}` : ""}</p>
  </main>`;

let retrying = false;
function retry() {
  if (retrying) return;
  retrying = true;
  document.getElementById("retry")!.textContent = "다시 시도하는 중…";
  invoke("neterror_retry").catch(() => {
    retrying = false;
    document.getElementById("retry")!.textContent = "다시 시도";
  });
}
document.getElementById("retry")!.onclick = retry;
document.getElementById("proceed")?.addEventListener("click", (e) => {
  (e.currentTarget as HTMLButtonElement).textContent = "이동하는 중…";
  invoke("neterror_proceed").catch(() => {});
});
document.getElementById("search")?.addEventListener("click", () => invoke("neterror_search").catch(() => {}));
// Back online: try again by itself.
window.addEventListener("online", () => setTimeout(retry, 600));

// ---------------------------------------------------------------- which picture

/** Each time one of two: the island (three.js) or a kite whose string broke (black and white).
 *  `?s=kite|island` picks one (for checking). */
const pick = params.get("s");
const SCENE: "island" | "kite" = pick === "kite" || pick === "island" ? pick : Math.random() < 0.5 ? "kite" : "island";
document.body.classList.add(`scene-${SCENE}`);
const isDark = () => {
  const id = document.documentElement.dataset.theme;
  return id ? !!THEMES.find((x) => x.id === id)?.dark : matchMedia("(prefers-color-scheme: dark)").matches;
};
if (SCENE === "kite") drawKite();

/** A kite (the browser's name) drifting away, its string snapped: line art, one color. */
function drawKite() {
  // Black on white or white on black, following the theme (it may arrive a moment later).
  const mono = () => document.body.classList.toggle("mono-dark", isDark());
  mono();
  new MutationObserver(mono).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", mono);
  document.getElementById("scene")?.remove();
  document.querySelector(".badge")!.innerHTML = `<span class="dot"></span>연결이 끊어졌어요`;
  if (code === "offline") document.querySelector(".lead")!.textContent = "연줄이 끊어져 연이 멀리 날아갔어요. 인터넷이 다시 연결되면 이 페이지를 자동으로 다시 불러옵니다.";
  const art = document.createElement("div");
  art.className = "kite-art";
  art.setAttribute("aria-hidden", "true");
  art.innerHTML = `
  <svg viewBox="0 0 640 400" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round">
    <g class="wind" stroke-width="2" opacity=".35">
      <path d="M40 90 q60 -18 120 0 t120 0" stroke-dasharray="6 10"/>
      <path d="M380 300 q50 -14 100 0 t100 0" stroke-dasharray="6 10"/>
      <path d="M90 200 q40 -12 80 0" stroke-dasharray="4 9"/>
    </g>
    <g class="flyer">
      <g class="kite" transform="translate(430 120)">
        <path class="frame" d="M0 -78 L52 0 L0 104 L-52 0 Z" stroke-width="3.2"/>
        <path d="M0 -78 L0 0 L-52 0 Z" class="fill"/>
        <path d="M0 0 L52 0 L0 104 Z" class="fill"/>
        <path d="M0 -78 L0 104 M-52 0 L52 0" stroke-width="2"/>
        <g class="tail" stroke-width="2">
          <path d="M0 104 C 18 132 -16 150 4 178 S -10 222 10 246"/>
          <path d="M2 140 l-11 -7 v14 z M2 140 l11 -7 v14 z" class="fill"/>
          <path d="M3 192 l-10 -6 v12 z M3 192 l10 -6 v12 z" class="fill"/>
          <path d="M8 236 l-9 -5 v10 z M8 236 l9 -5 v10 z" class="fill"/>
        </g>
        <g class="snapped" stroke-width="1.8">
          <path d="M0 8 C -40 60 -60 120 -112 150"/>
          <path d="M-112 150 l-9 -2 M-112 150 l-7 6 M-112 150 l-3 -8"/>
        </g>
      </g>
    </g>
    <g class="ground" stroke-width="1.8">
      <path d="M78 368 C 150 330 210 300 262 262"/>
      <path d="M262 262 l9 -1 M262 262 l6 -7 M262 262 l3 8"/>
      <circle cx="66" cy="372" r="22" stroke-width="3"/>
      <circle cx="66" cy="372" r="6" stroke-width="2.4"/>
      <path d="M66 372 L 40 398" stroke-width="3"/>
    </g>
    <g class="gap" stroke-width="1.6" opacity=".55">
      <path d="M282 246 l6 -3 M292 238 l4 -4 M300 226 l5 -2"/>
    </g>
  </svg>`;
  document.body.prepend(art);
}

// ---------------------------------------------------------------- the island

const canvas = document.getElementById("scene") as HTMLCanvasElement;
const themeId = document.documentElement.dataset.theme;
const dusk = themeId ? !!THEMES.find((x) => x.id === themeId)?.dark : matchMedia("(prefers-color-scheme: dark)").matches;
const still = matchMedia("(prefers-reduced-motion: reduce)").matches;

const P = dusk
  ? { top: 0x34408a, bottom: 0xffb28a, sea: 0x5a86d6, sand: 0xf3c992, shallow: 0x8fd3e0, sun: 0xffd6a8, light: 2.4, hemi: 1.7, fog: 0xe7a99a }
  : { top: 0x4fb6ef, bottom: 0xfff0d2, sea: 0x22acd0, sand: 0xf6dcaa, shallow: 0x86e8e2, sun: 0xfff6dc, light: 2.4, hemi: 1.5, fog: 0xd8eef5 };

let renderer: THREE.WebGLRenderer | null = null;
if (SCENE === "island") {
  try {
    renderer = new THREE.WebGLRenderer({ canvas, antialias: true, powerPreference: "low-power" });
  } catch {
    document.body.classList.add("no-gl");
  }
}

if (renderer) {
  const r = renderer;
  r.setPixelRatio(Math.min(devicePixelRatio, 2));
  r.shadowMap.enabled = true;
  r.shadowMap.type = THREE.PCFSoftShadowMap;
  r.toneMapping = THREE.ACESFilmicToneMapping;
  r.toneMappingExposure = dusk ? 1.25 : 1.1;
  r.outputColorSpace = THREE.SRGBColorSpace;

  const scene = new THREE.Scene();
  scene.fog = new THREE.Fog(P.fog, 70, 210);
  const camera = new THREE.PerspectiveCamera(38, 1, 0.1, 600);

  // Sky: a big sphere, gradient from the horizon up.
  const sky = new THREE.Mesh(
    new THREE.SphereGeometry(400, 32, 16),
    new THREE.ShaderMaterial({
      side: THREE.BackSide,
      depthWrite: false,
      fog: false,
      uniforms: { top: { value: new THREE.Color(P.top) }, bottom: { value: new THREE.Color(P.bottom) } },
      vertexShader: "varying vec3 vP; void main(){ vP = (modelMatrix*vec4(position,1.)).xyz; gl_Position = projectionMatrix*viewMatrix*vec4(vP,1.); }",
      fragmentShader: "uniform vec3 top; uniform vec3 bottom; varying vec3 vP; void main(){ float h = clamp(normalize(vP).y*1.6+0.08,0.,1.); gl_FragColor = vec4(mix(bottom, top, pow(h,0.8)),1.); }",
    }),
  );
  scene.add(sky);

  // Sun: a glow low in the sky behind the island (decoration); the light that makes the shadows
  // comes from in front, so the island is lit rather than a silhouette.
  const sunDir = new THREE.Vector3(-0.7, dusk ? 0.1 : 0.32, -1).normalize();
  const lightDir = new THREE.Vector3(0.45, dusk ? 0.55 : 0.8, 0.6).normalize();
  // Drawn by a shader (a soft disc), no texture: a texture's bright middle breaks into dots.
  const sun = new THREE.Mesh(
    new THREE.PlaneGeometry(1, 1),
    new THREE.ShaderMaterial({
      transparent: true,
      depthWrite: false,
      fog: false,
      toneMapped: false,
      uniforms: { color: { value: new THREE.Color(P.sun) } },
      vertexShader: "varying vec2 vUv; void main(){ vUv = uv; gl_Position = projectionMatrix*modelViewMatrix*vec4(position,1.); }",
      fragmentShader:
        "uniform vec3 color; varying vec2 vUv; void main(){ float d = length(vUv-0.5)*2.; float core = smoothstep(0.42,0.36,d); float halo = pow(max(0.,1.-d),2.2)*0.55; gl_FragColor = vec4(mix(color, vec3(1.,1.,0.96), core*0.6), clamp(core+halo,0.,1.)); }",
    }),
  );
  sun.position.copy(sunDir.clone().multiplyScalar(300));
  sun.scale.setScalar(dusk ? 70 : 52);
  scene.add(sun);
  const faceCamera = () => sun.quaternion.copy(camera.quaternion);

  scene.add(new THREE.HemisphereLight(P.top, P.sand, P.hemi));
  const light = new THREE.DirectionalLight(P.sun, P.light);
  light.position.copy(lightDir.clone().multiplyScalar(40));
  light.castShadow = true;
  light.shadow.mapSize.set(1024, 1024);
  Object.assign(light.shadow.camera, { left: -12, right: 12, top: 12, bottom: -12, near: 1, far: 100 });
  light.shadow.bias = -0.0008;
  scene.add(light);

  const mat = (color: number, extra: THREE.MeshStandardMaterialParameters = {}) => new THREE.MeshStandardMaterial({ color, flatShading: true, roughness: 0.85, ...extra });
  const mesh = (g: THREE.BufferGeometry, m: THREE.Material, shadow = true) => {
    const x = new THREE.Mesh(g, m);
    x.castShadow = shadow;
    x.receiveShadow = true;
    return x;
  };

  // Sea: a plane whose vertices move every frame (low-poly waves).
  const seaGeo = new THREE.PlaneGeometry(320, 320, 80, 80);
  seaGeo.rotateX(-Math.PI / 2);
  const base = Float32Array.from(seaGeo.attributes.position.array as Float32Array);
  const sea = new THREE.Mesh(seaGeo, mat(P.sea, { roughness: 0.25, metalness: 0.08, transparent: true, opacity: 0.94 }));
  sea.receiveShadow = true;
  scene.add(sea);

  const island = new THREE.Group();
  island.position.set(3.2, 0, 0);
  scene.add(island);

  // Shallow water ring and the sand: a squashed, slightly uneven sphere.
  const shallow = new THREE.Mesh(new THREE.CircleGeometry(10.5, 40).rotateX(-Math.PI / 2), new THREE.MeshStandardMaterial({ color: P.shallow, transparent: true, opacity: 0.55, roughness: 0.3 }));
  shallow.position.y = 0.06;
  island.add(shallow);
  const sandGeo = new THREE.SphereGeometry(7.2, 28, 12);
  const sp = sandGeo.attributes.position;
  for (let i = 0; i < sp.count; i++) {
    const y = sp.getY(i);
    const n = Math.sin(sp.getX(i) * 1.7) * Math.cos(sp.getZ(i) * 1.3) * 0.18;
    sp.setY(i, y * 0.24 + (y > 0 ? n : 0));
  }
  sandGeo.computeVertexNormals();
  const sand = mesh(sandGeo, mat(P.sand), false);
  sand.scale.set(1, 1, 0.82);
  sand.position.y = -0.55;
  island.add(sand);
  const top = 0.95; // sand surface height near the middle

  // A few rocks and a starfish.
  for (const [x, z, s] of [[-5.4, 1.8, 0.5], [-4.8, 2.6, 0.32], [5.6, -1.4, 0.42]]) {
    const rock = mesh(new THREE.DodecahedronGeometry(s, 0), mat(0x8c8f94));
    rock.position.set(x, 0.35, z);
    rock.rotation.set(x, z, s);
    island.add(rock);
  }
  const star = mesh(new THREE.CylinderGeometry(0.28, 0.28, 0.06, 5), mat(0xff7a59));
  star.position.set(1.8, top - 0.05, 3.4);
  island.add(star);

  // Palm tree: a bent trunk of rings, drooping leaves, coconuts.
  const palm = new THREE.Group();
  palm.position.set(-2.6, top - 0.2, -1.6);
  island.add(palm);
  let tip = new THREE.Vector3();
  for (let i = 0; i < 9; i++) {
    const ring = mesh(new THREE.CylinderGeometry(0.17 - i * 0.008, 0.2 - i * 0.008, 0.62, 7), mat(i % 2 ? 0x8a5a33 : 0x9c6a3d));
    const bend = i * 0.055;
    ring.position.set(Math.sin(bend) * i * 0.55, i * 0.58 + 0.3, 0);
    ring.rotation.z = -bend;
    palm.add(ring);
    tip = ring.position.clone().add(new THREE.Vector3(Math.sin(bend) * 0.3, 0.3, 0));
  }
  const leaves = new THREE.Group();
  leaves.position.copy(tip);
  palm.add(leaves);
  for (let i = 0; i < 8; i++) {
    const g = new THREE.PlaneGeometry(0.9, 3.4, 1, 8);
    const p = g.attributes.position;
    for (let k = 0; k < p.count; k++) {
      const y = p.getY(k) + 1.7; // 0..3.4 along the leaf
      p.setZ(k, -0.13 * y * y); // droop
      p.setX(k, p.getX(k) * (1 - y / 4.2));
    }
    g.translate(0, 1.7, 0);
    g.computeVertexNormals();
    const leaf = mesh(g, mat(i % 2 ? 0x3d9b4b : 0x4fb35a, { side: THREE.DoubleSide }));
    leaf.rotation.set(-1.15, (i / 8) * Math.PI * 2, 0, "YXZ");
    leaves.add(leaf);
  }
  for (let i = 0; i < 3; i++) {
    const nut = mesh(new THREE.SphereGeometry(0.16, 7, 5), mat(0x5b3a1e));
    nut.position.set(Math.cos(i * 2.1) * 0.22, -0.18, Math.sin(i * 2.1) * 0.22);
    leaves.add(nut);
  }

  // Beach umbrella: striped canopy (vertex colors per slice), pole, knob.
  const umbrella = new THREE.Group();
  umbrella.position.set(1.4, top - 0.1, 0.2);
  umbrella.rotation.z = 0.16;
  umbrella.scale.setScalar(1.12);
  island.add(umbrella);
  umbrella.add(mesh(new THREE.CylinderGeometry(0.045, 0.045, 3.3, 6), mat(0xf5f1e8)).translateY(1.65));
  const canopyGeo = new THREE.ConeGeometry(2.5, 0.85, 16, 1, true).toNonIndexed();
  const cols: number[] = [];
  const red = new THREE.Color(dusk ? 0xff6b6b : 0xff5a4e);
  const white = new THREE.Color(0xfff8ee);
  for (let i = 0; i < canopyGeo.attributes.position.count / 3; i++) {
    const c = Math.floor(i / 2) % 2 ? white : red;
    for (let k = 0; k < 3; k++) cols.push(c.r, c.g, c.b);
  }
  canopyGeo.setAttribute("color", new THREE.Float32BufferAttribute(cols, 3));
  canopyGeo.computeVertexNormals();
  const canopy = mesh(canopyGeo, new THREE.MeshStandardMaterial({ vertexColors: true, flatShading: true, side: THREE.DoubleSide, roughness: 0.7 }));
  canopy.position.y = 3.25;
  umbrella.add(canopy);
  umbrella.add(mesh(new THREE.SphereGeometry(0.09, 6, 4), mat(0xf5f1e8)).translateY(3.72));

  // Striped towel under it.
  const towelTex = (() => {
    const c = document.createElement("canvas");
    c.width = 64;
    c.height = 128;
    const g = c.getContext("2d")!;
    const bands = ["#3b82f6", "#fef3c7", "#f59e0b", "#fef3c7"];
    for (let i = 0; i < 8; i++) {
      g.fillStyle = bands[i % 4];
      g.fillRect(0, i * 16, 64, 16);
    }
    const tx = new THREE.CanvasTexture(c);
    tx.colorSpace = THREE.SRGBColorSpace;
    return tx;
  })();
  const towel = mesh(new THREE.BoxGeometry(1.5, 0.03, 2.8), new THREE.MeshStandardMaterial({ map: towelTex, roughness: 0.95 }), false);
  towel.position.set(1.9, top + 0.02, 0.9);
  towel.rotation.y = -0.35;
  island.add(towel);

  // The reader: leaning back on a little backrest, legs out, a book in both hands.
  const reader = new THREE.Group();
  reader.position.set(1.85, top + 0.05, 0.55);
  reader.rotation.y = -0.35 + Math.PI; // facing the sea and the camera
  reader.scale.setScalar(1.3);
  island.add(reader);
  const skin = mat(0xf0c08f);
  const rest = mesh(new THREE.BoxGeometry(0.9, 0.8, 0.12), mat(0xf59e0b));
  rest.position.set(0, 0.35, 0.55);
  rest.rotation.x = -0.5;
  reader.add(rest);
  const body = new THREE.Group();
  body.position.set(0, 0.18, 0.25);
  body.rotation.x = -0.45;
  reader.add(body);
  const torso = mesh(new THREE.CapsuleGeometry(0.24, 0.5, 4, 8), mat(0x22b8a6));
  torso.position.y = 0.42;
  body.add(torso);
  const head = new THREE.Group();
  head.position.y = 1.02;
  body.add(head);
  head.add(mesh(new THREE.SphereGeometry(0.22, 12, 9), skin));
  const hair = mesh(new THREE.SphereGeometry(0.23, 12, 8, 0, Math.PI * 2, 0, Math.PI / 2), mat(0x3b2a20));
  hair.rotation.x = 0.3;
  head.add(hair);
  const brim = mesh(new THREE.CylinderGeometry(0.42, 0.42, 0.03, 18), mat(0xe9c97c));
  brim.position.y = 0.12;
  brim.rotation.x = 0.25;
  head.add(brim);
  const crown = mesh(new THREE.CylinderGeometry(0.17, 0.21, 0.18, 14), mat(0xe9c97c));
  crown.position.set(0, 0.22, -0.02);
  crown.rotation.x = 0.25;
  head.add(crown);
  const band = mesh(new THREE.CylinderGeometry(0.212, 0.212, 0.05, 14), mat(0xff5a4e));
  band.position.set(0, 0.16, -0.01);
  band.rotation.x = 0.25;
  head.add(band);
  for (const x of [-0.12, 0.12]) {
    const leg = mesh(new THREE.CapsuleGeometry(0.1, 0.9, 4, 6), skin);
    leg.rotation.x = Math.PI / 2 - 0.12;
    leg.position.set(x, 0.12, -0.45);
    reader.add(leg);
    const short = mesh(new THREE.CapsuleGeometry(0.115, 0.2, 4, 6), mat(0x2f4fa8));
    short.rotation.x = Math.PI / 2 - 0.12;
    short.position.set(x, 0.16, -0.05);
    reader.add(short);
  }
  // Book: two covers in a V, pages, and one page that turns now and then.
  const book = new THREE.Group();
  book.position.set(0, 0.62, -0.36);
  book.rotation.x = -0.95;
  body.add(book);
  for (const side of [-1, 1]) {
    const cover = mesh(new THREE.BoxGeometry(0.34, 0.46, 0.02), mat(0x4361ee));
    cover.position.set(side * 0.17, 0, 0);
    cover.rotation.y = side * 0.22;
    book.add(cover);
    const pages = mesh(new THREE.BoxGeometry(0.31, 0.43, 0.04), mat(0xfaf7ef));
    pages.position.set(side * 0.16, 0, 0.03);
    pages.rotation.y = side * 0.22;
    book.add(pages);
  }
  const pageGeo = new THREE.PlaneGeometry(0.3, 0.42).translate(0.15, 0, 0);
  const page = mesh(pageGeo, new THREE.MeshStandardMaterial({ color: 0xfffdf6, side: THREE.DoubleSide, roughness: 1 }), false);
  page.position.z = 0.06;
  book.add(page);
  for (const side of [-1, 1]) {
    const arm = mesh(new THREE.CapsuleGeometry(0.075, 0.48, 4, 6), mat(0x22b8a6));
    arm.position.set(side * 0.27, 0.6, -0.18);
    arm.rotation.set(-1.0, 0, side * -0.35);
    body.add(arm);
  }
  // A drink in the sand.
  const glass = mesh(new THREE.CylinderGeometry(0.11, 0.08, 0.28, 10), new THREE.MeshStandardMaterial({ color: 0xffb3c1, transparent: true, opacity: 0.85, roughness: 0.2 }));
  glass.position.set(2.75, top + 0.14, 0.2);
  island.add(glass);
  const straw = mesh(new THREE.CylinderGeometry(0.015, 0.015, 0.36, 5), mat(0x22c55e));
  straw.position.set(2.79, top + 0.36, 0.2);
  straw.rotation.z = -0.3;
  island.add(straw);

  // Clouds and gulls.
  const clouds: THREE.Group[] = [];
  const cloudMat = new THREE.MeshStandardMaterial({ color: dusk ? 0xffd2c2 : 0xffffff, flatShading: true, roughness: 1, transparent: true, opacity: 0.95 });
  for (let i = 0; i < 7; i++) {
    const c = new THREE.Group();
    for (let k = 0; k < 5; k++) {
      const puff = new THREE.Mesh(new THREE.IcosahedronGeometry(2 + Math.random() * 2.2, 0), cloudMat);
      puff.position.set(k * 2.6 - 5, Math.random() * 1.5, Math.random() * 2);
      c.add(puff);
    }
    c.position.set(-120 + i * 42, 34 + Math.random() * 22, -120 - Math.random() * 60);
    c.scale.setScalar(0.8 + Math.random() * 0.7);
    scene.add(c);
    clouds.push(c);
  }
  const gulls: THREE.Group[] = [];
  const wingGeo = new THREE.BufferGeometry().setAttribute("position", new THREE.Float32BufferAttribute([0, 0, 0, 0.9, 0.12, -0.15, 0.9, 0.12, 0.15], 3));
  wingGeo.computeVertexNormals();
  const gullMat = new THREE.MeshBasicMaterial({ color: dusk ? 0x3a2f4a : 0x4b5563, side: THREE.DoubleSide });
  for (let i = 0; i < 3; i++) {
    const g = new THREE.Group();
    const lw = new THREE.Mesh(wingGeo, gullMat);
    const rw = new THREE.Mesh(wingGeo, gullMat);
    rw.scale.x = -1;
    g.add(lw, rw);
    g.userData = { r: 14 + i * 5, h: 13 + i * 2.5, s: 0.12 + i * 0.03, p: i * 2 };
    scene.add(g);
    gulls.push(g);
  }

  // ---------------------------------------------------------------- camera, size, animation
  const look = new THREE.Vector3(2.6, 1.4, 0);
  const pointer = { x: 0, y: 0 };
  window.addEventListener("pointermove", (e) => {
    pointer.x = e.clientX / innerWidth - 0.5;
    pointer.y = e.clientY / innerHeight - 0.5;
  });
  function resize() {
    const w = innerWidth;
    const hgt = innerHeight;
    r.setSize(w, hgt, false);
    camera.aspect = w / hgt;
    // Narrow windows: step back so the island still fits beside the card.
    camera.fov = w / hgt < 1.1 ? 46 : 34;
    camera.updateProjectionMatrix();
  }
  resize();
  window.addEventListener("resize", resize);

  const clock = new THREE.Clock();
  let running = true;
  document.addEventListener("visibilitychange", () => {
    running = !document.hidden;
    if (running) loop();
  });

  function frame(time: number) {
    // Waves.
    const pos = seaGeo.attributes.position;
    for (let i = 0; i < pos.count; i++) {
      const x = base[i * 3];
      const z = base[i * 3 + 2];
      pos.setY(i, Math.sin(x * 0.09 + time * 0.9) * 0.28 + Math.cos(z * 0.11 + time * 0.7) * 0.22 + Math.sin((x + z) * 0.05 + time * 0.5) * 0.18 - 0.35);
    }
    pos.needsUpdate = true;
    seaGeo.computeVertexNormals();
    shallow.material.opacity = 0.5 + Math.sin(time * 0.8) * 0.06;
    // Palm and umbrella sway in the breeze.
    leaves.rotation.z = Math.sin(time * 0.9) * 0.05;
    leaves.rotation.x = Math.cos(time * 0.7) * 0.04;
    canopy.rotation.y = Math.sin(time * 0.6) * 0.03;
    // Reading: breathe, nod a little, turn a page every few seconds.
    torso.scale.set(1, 1 + Math.sin(time * 1.6) * 0.015, 1);
    head.rotation.x = 0.18 + Math.sin(time * 0.5) * 0.06;
    head.rotation.y = Math.sin(time * 0.23) * 0.08;
    const cycle = time % 7;
    page.rotation.y = cycle < 1.2 ? -Math.PI * (cycle / 1.2) : 0;
    // Sky life.
    for (const c of clouds) {
      c.position.x += 0.02;
      if (c.position.x > 160) c.position.x = -160;
    }
    for (const g of gulls) {
      const d = g.userData as { r: number; h: number; s: number; p: number };
      const a = time * d.s + d.p;
      g.position.set(island.position.x + Math.cos(a) * d.r, d.h + Math.sin(time * 0.6 + d.p) * 0.8, Math.sin(a) * d.r - 6);
      g.rotation.y = -a;
      const flap = Math.sin(time * 6 + d.p) * 0.5;
      g.children[0].rotation.z = flap;
      g.children[1].rotation.z = -flap;
    }
    // Camera: a slow drift and a little parallax from the pointer.
    const drift = still ? 0 : Math.sin(time * 0.06) * 0.18;
    const ang = 0.72 + drift + pointer.x * 0.12;
    const dist = 26;
    camera.position.set(look.x + Math.sin(ang) * dist, 7.4 - pointer.y * 1.4, Math.cos(ang) * dist);
    camera.lookAt(look);
    faceCamera();
    // Keep the island right of the card on wide windows.
    const shift = innerWidth > 900 ? -0.18 : 0;
    camera.setViewOffset(innerWidth, innerHeight, shift * innerWidth, 0, innerWidth, innerHeight);
    r.render(scene, camera);
  }

  function loop() {
    if (!running) return;
    frame(clock.getElapsedTime());
    if (!still) requestAnimationFrame(loop);
  }
  loop();
}
