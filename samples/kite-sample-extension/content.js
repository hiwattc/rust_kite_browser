// Content script on example.com: a small badge, so you can see it ran.
(() => {
  if (document.getElementById("kite-sample-badge")) return;
  const b = document.createElement("div");
  b.id = "kite-sample-badge";
  b.textContent = "🪁 Kite 샘플 확장 — 콘텐츠 스크립트 동작 중";
  Object.assign(b.style, {
    position: "fixed", right: "16px", bottom: "16px", zIndex: 2147483647, padding: "8px 14px",
    borderRadius: "18px", background: "linear-gradient(135deg, #7c5cff, #388cff)", color: "#fff",
    font: "600 13px/1.4 -apple-system, 'Segoe UI', sans-serif", boxShadow: "0 6px 18px rgba(0,0,0,.25)",
  });
  document.documentElement.append(b);
  setTimeout(() => b.remove(), 8000);
})();
