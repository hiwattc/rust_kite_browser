// Writes docs/architecture.svg from src/settings/diagram.ts (the picture the app shows), with the
// colors inlined (the app takes them from the theme).
// Run: node --experimental-strip-types scripts/diagram.mjs   (Node 22.6+)
import { writeFileSync } from "node:fs";

const { diagram } = await import("../src/settings/diagram.ts");
const style = `<style>
text{font-family:-apple-system,"Segoe UI","Apple SD Gothic Neo","Malgun Gothic",sans-serif;font-size:11px;fill:#1c2230}
.d-title{font-size:12px;font-weight:650;fill:#5d6576}.d-head{font-size:12.5px;font-weight:650}.d-dim{fill:#5d6576}
.d-label{fill:#8a92a3;font-size:10px}.d-mono{font-family:ui-monospace,"SF Mono",Consolas,monospace;font-weight:600;fill:#6d4aff}
.d-app{fill:#efebff;stroke:#c4b6ff}.d-box{fill:#fff;stroke:#c9ced8}.d-stack{opacity:.6}.d-pill{fill:#e9ecf1}
.d-ext{fill:#fff;stroke:#6d4aff;stroke-dasharray:4 3}.d-os{fill:#f0f2f5;stroke:#dde1e8}.d-line{stroke:#8a92a3;stroke-width:1.3}
.d-dash{stroke-dasharray:4 3}.d-headfill{fill:#8a92a3}</style>`;
const [, w, h] = /viewBox="0 0 (\d+) (\d+)"/.exec(diagram);
const svg = diagram
  .trim()
  .replace(/<svg class="diagram" /, `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" `)
  .replace(/(<svg[^>]*>)/, `$1${style}<rect width="${w}" height="${h}" fill="#f6f7f9"/>`);
writeFileSync("docs/architecture.svg", svg + "\n");
console.log(`docs/architecture.svg ${w}×${h}`);
