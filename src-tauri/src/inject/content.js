// Injected into every content tab (document start, main frame) and prepended to every
// automation script, so `window.__kite` always exists. Plain ES2017, no dependencies.
//
// The page can see and call `__kite` (same JS world). That is fine: it only reads and acts on
// the page itself. The only message it sends to the app is `content_event` with a fixed set of
// kinds; the app never trusts data from it (it reads URLs back from the webview itself).
(function () {
  if (window.__kite) return;
  var KEYS = __KITE_KEYMAP__;
  var lastCombo = "";
  // A popup window (window.open with a size) on macOS shares its opener tab's WebKit setup, so
  // this script and the tab's IPC come along: anything it sent would act on the opener tab
  // (⌘W would close it, a login would be saved for the tab's site). So in a popup: no messages
  // at all, and window.close() / ⌘W close the popup through a URL Kite catches.
  var popup = false;
  try {
    popup = window.top === window && !!window.opener;
  } catch (e) {}
  if (popup) {
    var closePopup = function () {
      location.href = "kite-popup-close:" + Date.now();
    };
    try {
      window.close = closePopup;
    } catch (e) {}
    window.addEventListener(
      "keydown",
      function (e) {
        if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.code === "KeyW") {
          e.preventDefault();
          closePopup();
        }
      },
      true
    );
  }
  // Looked up at send time: our init script may run before Tauri's IPC script.
  function send(kind, action) {
    if (popup) return;
    var t = window.__TAURI_INTERNALS__;
    if (t && typeof t.invoke === "function") {
      try {
        t.invoke("content_event", { kind: kind, action: action || "" }).catch(function () {});
      } catch (e) {}
    }
  }

  var kite = { version: 1, bootAt: Math.round(performance.now()), bootState: document.readyState };
  try {
    Object.defineProperty(window, "__kite", { value: kite, enumerable: false, configurable: false, writable: false });
  } catch (e) {
    window.__kite = kite;
  }

  // ---------- shortcuts ----------
  function combo(e) {
    var s = "";
    if (e.ctrlKey) s += "Ctrl+";
    if (e.altKey) s += "Alt+";
    if (e.shiftKey) s += "Shift+";
    if (e.metaKey) s += "Meta+";
    return s + e.code;
  }
  window.addEventListener(
    "keydown",
    function (e) {
      lastCombo = combo(e);
      var a = KEYS[lastCombo];
      if (!a || !e.isTrusted) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      send("shortcut", a);
    },
    true
  );

  // ---------- Ctrl + wheel = page zoom (like Chrome) ----------
  // WebView2's own Ctrl+wheel zoom is off (zoom hotkeys disabled) and WKWebView has none: the
  // app zooms the tab (same steps as Ctrl +/-). A page that handles Ctrl+wheel itself (maps,
  // editors) and cancels it keeps it, as in Chrome. Trackpads send many small deltas: they add
  // up to one step, and steps are spaced out.
  var wheelSum = 0;
  var wheelAt = 0;
  var stepAt = 0;
  window.addEventListener(
    "wheel",
    function (e) {
      if (!e.ctrlKey || !e.isTrusted || e.defaultPrevented || !e.deltaY) return;
      e.preventDefault();
      var now = Date.now();
      if (now - wheelAt > 300) wheelSum = 0;
      wheelAt = now;
      wheelSum += e.deltaMode === 1 ? e.deltaY * 40 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
      if (Math.abs(wheelSum) < 30 || now - stepAt < 70) return;
      send("shortcut", wheelSum < 0 ? "zoom_in" : "zoom_out");
      wheelSum = 0;
      stepAt = now;
    },
    { passive: false }
  );

  // ---------- SPA url changes (the app reads the real URL from the webview) ----------
  var urlTimer = 0;
  function urlChanged() {
    clearTimeout(urlTimer);
    urlTimer = setTimeout(function () {
      send("url");
    }, 50);
  }
  ["pushState", "replaceState"].forEach(function (m) {
    var orig = history[m];
    if (typeof orig !== "function") return;
    history[m] = function () {
      var r = orig.apply(this, arguments);
      urlChanged();
      return r;
    };
  });
  window.addEventListener("popstate", urlChanged);
  window.addEventListener("hashchange", urlChanged);

  // ---------- console + network log (for the agent) ----------
  var MAX_LOG = 300;
  kite.console = [];
  kite.network = [];
  function pushLog(list, item) {
    list.push(item);
    if (list.length > MAX_LOG) list.splice(0, list.length - MAX_LOG);
  }
  function fmt(a) {
    if (typeof a === "string") return a;
    if (a instanceof Error) return a.stack || String(a);
    try {
      return JSON.stringify(a);
    } catch (e) {
      return String(a);
    }
  }
  ["log", "info", "warn", "error", "debug"].forEach(function (level) {
    var orig = console[level];
    if (typeof orig !== "function") return;
    console[level] = function () {
      try {
        var text = Array.prototype.map.call(arguments, fmt).join(" ");
        pushLog(kite.console, { level: level, text: text.slice(0, 2000), time: Date.now() });
      } catch (e) {}
      return orig.apply(this, arguments);
    };
  });
  window.addEventListener("error", function (e) {
    pushLog(kite.console, { level: "error", text: "Uncaught " + (e.message || "error") + (e.filename ? " (" + e.filename + ":" + e.lineno + ")" : ""), time: Date.now() });
  });
  window.addEventListener("unhandledrejection", function (e) {
    pushLog(kite.console, { level: "error", text: "Unhandled rejection: " + fmt(e.reason), time: Date.now() });
  });
  if (typeof window.fetch === "function") {
    var origFetch = window.fetch;
    window.fetch = function (input, init) {
      var started = Date.now();
      var method = (init && init.method) || (input && input.method) || "GET";
      var url = typeof input === "string" ? input : input && input.url ? input.url : String(input);
      var entry = { type: "fetch", method: String(method).toUpperCase(), url: url, status: 0, ms: 0, time: started };
      pushLog(kite.network, entry);
      return origFetch.apply(this, arguments).then(
        function (r) {
          entry.status = r.status;
          entry.ms = Date.now() - started;
          return r;
        },
        function (err) {
          entry.error = String(err);
          entry.ms = Date.now() - started;
          throw err;
        }
      );
    };
  }
  if (window.XMLHttpRequest) {
    var xo = XMLHttpRequest.prototype.open;
    var xs = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function (method, url) {
      this.__kiteEntry = { type: "xhr", method: String(method).toUpperCase(), url: String(url), status: 0, ms: 0, time: 0 };
      return xo.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function () {
      var x = this;
      var entry = x.__kiteEntry;
      if (entry) {
        entry.time = Date.now();
        pushLog(kite.network, entry);
        x.addEventListener("loadend", function () {
          entry.status = x.status;
          entry.ms = Date.now() - entry.time;
        });
      }
      return xs.apply(this, arguments);
    };
  }

  // ---------- dialogs ----------
  // Native alert/confirm/prompt stay as they are until an agent sets a policy for this page
  // (browser_handle_dialog); then they are answered automatically and logged.
  kite.dialogPolicy = null;
  kite.dialogs = [];
  ["alert", "confirm", "prompt"].forEach(function (kind) {
    var orig = window[kind];
    if (typeof orig !== "function") return;
    window[kind] = function (msg, def) {
      var p = kite.dialogPolicy;
      if (!p) return orig.apply(window, arguments);
      var accept = p.accept !== false;
      var ret = kind === "alert" ? undefined : kind === "confirm" ? accept : accept ? (p.promptText != null ? String(p.promptText) : def != null ? String(def) : "") : null;
      pushLog(kite.dialogs, { kind: kind, message: String(msg == null ? "" : msg).slice(0, 1000), answer: kind === "alert" ? "ok" : ret, time: Date.now() });
      return ret;
    };
  });

  // ---------- element refs ----------
  var refs = new Map();
  var refOfEl = new WeakMap();
  var seq = 0;
  function refOf(el) {
    var r = refOfEl.get(el);
    if (!r) {
      r = "e" + ++seq;
      refOfEl.set(el, r);
    }
    refs.set(r, typeof WeakRef === "function" ? new WeakRef(el) : { deref: function () { return el; } });
    return r;
  }
  function byRef(r) {
    var w = refs.get(String(r));
    var el = w && w.deref();
    if (!el || !el.isConnected) throw new Error("ref " + r + "를 찾을 수 없습니다(페이지가 바뀌었을 수 있음). browser_snapshot을 다시 호출하세요.");
    return el;
  }

  // ---------- accessibility-ish snapshot ----------
  var INTERACTIVE = {
    link: 1, button: 1, textbox: 1, searchbox: 1, checkbox: 1, radio: 1, combobox: 1, listbox: 1, option: 1,
    slider: 1, spinbutton: 1, switch: 1, tab: 1, menuitem: 1, menuitemcheckbox: 1, menuitemradio: 1, treeitem: 1, clickable: 1,
  };
  // Roles whose name comes from their own text: children are not listed separately.
  var NAME_FROM_CONTENT = { link: 1, button: 1, heading: 1, tab: 1, option: 1, menuitem: 1, menuitemcheckbox: 1, menuitemradio: 1, treeitem: 1, clickable: 1, switch: 1, checkbox: 1, radio: 1 };
  var SKIP_TAGS = { SCRIPT: 1, STYLE: 1, NOSCRIPT: 1, TEMPLATE: 1, HEAD: 1, META: 1, LINK: 1, BR: 1, WBR: 1 };

  function clean(s, max) {
    s = String(s == null ? "" : s).replace(/\s+/g, " ").trim();
    max = max || 120;
    return s.length > max ? s.slice(0, max - 1) + "…" : s;
  }
  function isVisible(el) {
    if (typeof el.checkVisibility === "function") {
      if (!el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return false;
    } else {
      var st = getComputedStyle(el);
      if (st.display === "none" || st.visibility === "hidden" || st.opacity === "0") return false;
    }
    if (el.getAttribute("aria-hidden") === "true") return false;
    return true;
  }
  function hasSize(el) {
    var r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  }
  function implicitRole(el) {
    var tag = el.tagName;
    switch (tag) {
      case "A":
        return el.hasAttribute("href") ? "link" : null;
      case "BUTTON":
      case "SUMMARY":
        return "button";
      case "INPUT": {
        var t = (el.getAttribute("type") || "text").toLowerCase();
        if (t === "hidden") return "hidden";
        if (t === "checkbox") return "checkbox";
        if (t === "radio") return "radio";
        if (t === "range") return "slider";
        if (t === "number") return "spinbutton";
        if (t === "search") return "searchbox";
        if (t === "submit" || t === "reset" || t === "button" || t === "image") return "button";
        if (t === "file") return "button";
        return "textbox";
      }
      case "TEXTAREA":
        return "textbox";
      case "SELECT":
        return el.multiple || el.size > 1 ? "listbox" : "combobox";
      case "OPTION":
        return "option";
      case "IMG":
        return el.getAttribute("alt") === "" ? null : "img";
      case "H1": case "H2": case "H3": case "H4": case "H5": case "H6":
        return "heading";
      case "NAV":
        return "navigation";
      case "MAIN":
        return "main";
      case "ASIDE":
        return "complementary";
      case "FORM":
        return "form";
      case "TABLE":
        return "table";
      case "TR":
        return "row";
      case "TH":
        return "columnheader";
      case "TD":
        return "cell";
      case "UL": case "OL": case "MENU":
        return "list";
      case "LI":
        return "listitem";
      case "DIALOG":
        return "dialog";
      case "P":
        return "paragraph";
      case "IFRAME":
        return "iframe";
      case "PRE":
        return "code";
      case "BLOCKQUOTE":
        return "blockquote";
      case "FIELDSET": case "DETAILS":
        return "group";
      case "PROGRESS":
        return "progressbar";
      case "VIDEO":
        return "video";
      case "AUDIO":
        return "audio";
      case "HEADER":
        return el.closest("article,section,main") ? null : "banner";
      case "FOOTER":
        return el.closest("article,section,main") ? null : "contentinfo";
      case "ARTICLE":
        return "article";
    }
    if (el.isContentEditable && (!el.parentElement || !el.parentElement.isContentEditable)) return "textbox";
    return null;
  }
  function roleOf(el) {
    var r = el.getAttribute("role");
    if (r) {
      r = r.trim().split(/\s+/)[0];
      if (r === "presentation" || r === "none") return null;
      return r;
    }
    r = implicitRole(el);
    if (r) return r;
    // Divs/spans made clickable by script: tabindex or a pointer cursor that starts here.
    var ti = el.getAttribute("tabindex");
    if (ti !== null && +ti >= 0) return "clickable";
    if (el.onclick || el.hasAttribute("onclick")) return "clickable";
    var cur = getComputedStyle(el).cursor;
    if (cur === "pointer") {
      var p = el.parentElement;
      if (!p || getComputedStyle(p).cursor !== "pointer") return "clickable";
    }
    return null;
  }
  function textOf(el) {
    return el.innerText != null ? el.innerText : el.textContent;
  }
  function labelText(el) {
    if (el.labels && el.labels.length) return Array.prototype.map.call(el.labels, function (l) { return textOf(l); }).join(" ");
    return "";
  }
  function nameOf(el, role) {
    var by = el.getAttribute("aria-labelledby");
    if (by) {
      var t = by.split(/\s+/).map(function (id) {
        var x = document.getElementById(id);
        return x ? textOf(x) : "";
      }).join(" ");
      if (clean(t)) return clean(t);
    }
    var a = el.getAttribute("aria-label");
    if (a && clean(a)) return clean(a);
    var tag = el.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
      var t2 = (el.type === "submit" || el.type === "button" || el.type === "reset") ? el.value : "";
      return clean(t2 || labelText(el) || el.getAttribute("placeholder") || el.getAttribute("title") || el.getAttribute("name") || "");
    }
    if (tag === "IMG") return clean(el.getAttribute("alt") || el.getAttribute("title") || "");
    if (tag === "IFRAME") return clean(el.getAttribute("title") || el.getAttribute("src") || "");
    if (NAME_FROM_CONTENT[role]) {
      var c = clean(textOf(el), 100);
      if (!c) {
        var img = el.querySelector("img[alt],svg[aria-label],[aria-label]");
        if (img) c = clean(img.getAttribute("alt") || img.getAttribute("aria-label") || "");
      }
      return c || clean(el.getAttribute("title") || "");
    }
    return clean(el.getAttribute("title") || "");
  }
  function stateOf(el, role) {
    var s = [];
    if (role === "heading") {
      var lv = el.getAttribute("aria-level") || (/^H(\d)$/.exec(el.tagName) || [])[1];
      if (lv) s.push("level=" + lv);
    }
    if (role === "checkbox" || role === "radio" || role === "switch" || role === "menuitemcheckbox") {
      var ch = el.getAttribute("aria-checked");
      if (el.checked || ch === "true") s.push("checked");
      else if (ch === "mixed" || el.indeterminate) s.push("mixed");
    }
    if (el.disabled || el.getAttribute("aria-disabled") === "true") s.push("disabled");
    var exp = el.getAttribute("aria-expanded");
    if (exp) s.push(exp === "true" ? "expanded" : "collapsed");
    if (el.getAttribute("aria-selected") === "true" || (role === "option" && el.selected)) s.push("selected");
    if (el.getAttribute("aria-current") && el.getAttribute("aria-current") !== "false") s.push("current");
    if (el.required) s.push("required");
    if (document.activeElement === el) s.push("focused");
    return s;
  }
  function valueOf(el, role) {
    if (el.tagName === "SELECT") {
      var o = el.selectedOptions && el.selectedOptions[0];
      return o ? clean(o.textContent, 80) : "";
    }
    if (role === "textbox" || role === "searchbox" || role === "spinbutton" || role === "slider" || role === "combobox") {
      if (el.type === "password") return el.value ? "••••" : "";
      if ("value" in el) return clean(el.value, 120);
      if (el.isContentEditable) return clean(textOf(el), 120);
    }
    return "";
  }
  function q(s) {
    return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"';
  }

  // Builds lines with two-space indentation. Generic elements are transparent: their children
  // are lifted. Text runs merge into one `- text:` line.
  function snapshot(opts) {
    opts = opts || {};
    var interactiveOnly = !!opts.interactiveOnly;
    var maxChars = opts.maxChars || 40000;
    var lines = [];
    var size = 0;
    var truncated = false;
    var count = 0;

    function emit(depth, s) {
      if (truncated) return;
      var line = new Array(depth + 1).join("  ") + s;
      size += line.length + 1;
      if (size > maxChars) {
        truncated = true;
        return;
      }
      lines.push(line);
    }

    function kids(node) {
      if (node.shadowRoot) return node.shadowRoot.childNodes;
      if (node.tagName === "SLOT" && typeof node.assignedNodes === "function") {
        var a = node.assignedNodes({ flatten: true });
        if (a.length) return a;
      }
      return node.childNodes;
    }

    // Returns true when anything was emitted for this subtree.
    function walk(node, depth, textBuf) {
      if (truncated || ++count > 20000) return;
      if (node.nodeType === 3) {
        var t = node.nodeValue;
        if (t && t.trim()) textBuf.push(t);
        return;
      }
      if (node.nodeType !== 1 && node.nodeType !== 11) return;
      var el = node;
      if (node.nodeType === 1) {
        if (SKIP_TAGS[el.tagName]) return;
        if (!isVisible(el)) return;
      }
      var role = node.nodeType === 1 ? roleOf(el) : null;
      if (role === "hidden") return;
      var listed = role && role !== "generic" && role !== "group" && role !== "clickable" ? true : role === "clickable";
      if (role === "group" && !el.getAttribute("aria-label")) listed = false;
      if (listed && INTERACTIVE[role] && !hasSize(el) && el.tagName !== "OPTION") listed = false;

      if (!listed) {
        // Transparent: block-level elements break text runs.
        var block = node.nodeType === 1 && /^(DIV|SECTION|HEADER|FOOTER|ARTICLE|ASIDE|DD|DT|DL|FIGURE|FIGCAPTION|LABEL|SPAN)$/.test(el.tagName) && el.tagName !== "SPAN" && el.tagName !== "LABEL";
        if (block) flush(textBuf, depth);
        var ch = kids(node);
        for (var i = 0; i < ch.length; i++) walk(ch[i], depth, textBuf);
        if (block) flush(textBuf, depth);
        return;
      }

      flush(textBuf, depth);
      var isInter = !!INTERACTIVE[role];
      var name = nameOf(el, role);
      if (interactiveOnly && !isInter) {
        var ch2 = kids(node);
        var buf2 = [];
        for (var j = 0; j < ch2.length; j++) walk(ch2[j], depth, buf2);
        return;
      }
      var head = "- " + role + (name ? " " + q(name) : "");
      var st = stateOf(el, role);
      for (var k = 0; k < st.length; k++) head += " [" + st[k] + "]";
      if (isInter || el.tagName === "IFRAME") head += " [ref=" + refOf(el) + "]";
      if (role === "link" && !interactiveOnly) {
        var href = el.getAttribute("href") || "";
        if (href && href.charAt(0) !== "#" && href.indexOf("javascript:") !== 0) head += " [url=" + clean(href, 100) + "]";
      }
      var val = valueOf(el, role);
      if (role === "img" || NAME_FROM_CONTENT[role]) {
        emit(depth, head + (val ? ": " + val : ""));
        // Interactive descendants inside a link/button (rare) still get listed.
        if (!interactiveOnly && el.querySelector) {
          var inner = el.querySelectorAll("input,select,textarea,button,a[href]");
          for (var m = 0; m < inner.length && m < 10; m++) walk(inner[m], depth + 1, []);
        }
        return;
      }
      if (role === "iframe") {
        var doc = null;
        try {
          doc = el.contentDocument;
        } catch (e) {}
        if (!doc || !doc.body) {
          emit(depth, head + " (다른 출처: 내용 접근 불가)");
          return;
        }
        emit(depth, head + ":");
        var b3 = [];
        walk(doc.body, depth + 1, b3);
        flush(b3, depth + 1);
        return;
      }
      if (el.tagName === "SELECT") {
        emit(depth, head + (val ? ": " + val : ""));
        var opts2 = el.options;
        for (var o = 0; o < opts2.length && o < 30; o++) {
          emit(depth + 1, "- option " + q(clean(opts2[o].textContent, 80)) + (opts2[o].selected ? " [selected]" : ""));
        }
        if (opts2.length > 30) emit(depth + 1, "- … " + (opts2.length - 30) + "개 더");
        return;
      }
      if (val || isInter) {
        emit(depth, head + (val ? ": " + val : ""));
        return;
      }
      // Containers: text-only ones go on one line.
      var start = lines.length;
      emit(depth, head + ":");
      var buf = [];
      var ch3 = kids(node);
      for (var n = 0; n < ch3.length; n++) walk(ch3[n], depth + 1, buf);
      flush(buf, depth + 1);
      if (lines.length === start + 1) {
        lines[start] = lines[start].slice(0, -1); // no children: drop the colon
      } else if (lines.length === start + 2 && /^\s*- text: /.test(lines[start + 1])) {
        lines[start] = lines[start] + " " + lines[start + 1].replace(/^\s*- text: /, "");
        lines.splice(start + 1, 1);
      }
    }

    function flush(buf, depth) {
      if (!buf.length) return;
      var t = clean(buf.join(" "), 300);
      buf.length = 0;
      if (t && !interactiveOnly) emit(depth, "- text: " + t);
    }

    var root = document.body || document.documentElement;
    if (opts.selector) {
      root = document.querySelector(opts.selector);
      if (!root) throw new Error("selector에 맞는 요소가 없습니다: " + opts.selector);
    }
    var top = [];
    walk(root, 0, top);
    flush(top, 0);
    if (truncated) lines.push("- … (잘림: 너무 김. selector로 범위를 줄이거나 interactive_only=true 사용)");
    return lines.join("\n");
  }

  // ---------- targets ----------
  function resolve(t) {
    t = t || {};
    if (t.ref) return byRef(t.ref);
    if (t.selector) {
      var el = document.querySelector(t.selector);
      if (!el) throw new Error("selector에 맞는 요소가 없습니다: " + t.selector);
      return el;
    }
    if (t.text) return byText(t.text);
    throw new Error("대상이 없습니다: ref, selector, text 중 하나를 주세요.");
  }
  // Smallest visible element whose text contains `text`, interactive ones first.
  function byText(text) {
    var needle = clean(text).toLowerCase();
    var all = document.querySelectorAll("a,button,input,select,textarea,label,summary,[role],[tabindex],[onclick],h1,h2,h3,h4,h5,h6,li,td,th,p,span,div");
    var best = null;
    var bestScore = 1e9;
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      var hay = clean(nameOf(el, roleOf(el) || "clickable") || textOf(el), 2000).toLowerCase();
      if (hay.indexOf(needle) < 0) continue;
      if (!isVisible(el) || !hasSize(el)) continue;
      var r = roleOf(el);
      var s = hay.length + (r && INTERACTIVE[r] ? 0 : 1000) + (hay === needle ? -500 : 0);
      if (s < bestScore) {
        best = el;
        bestScore = s;
      }
    }
    if (!best) throw new Error("텍스트를 포함한 요소가 없습니다: " + text);
    return best;
  }
  function describe(el) {
    var role = roleOf(el) || el.tagName.toLowerCase();
    var name = nameOf(el, role) || clean(textOf(el), 60);
    return role + (name ? " " + q(name) : "");
  }

  // ---------- visual feedback ----------
  function flash(el) {
    try {
      var r = el.getBoundingClientRect();
      var d = document.createElement("div");
      d.setAttribute("data-kite-flash", "");
      d.style.cssText =
        "position:fixed;z-index:2147483647;pointer-events:none;border:2px solid #7c5cff;border-radius:4px;" +
        "background:rgba(124,92,255,.15);transition:opacity .5s;left:" + (r.left - 2) + "px;top:" + (r.top - 2) + "px;width:" +
        (r.width + 4) + "px;height:" + (r.height + 4) + "px";
      (document.body || document.documentElement).appendChild(d);
      setTimeout(function () { d.style.opacity = "0"; }, 400);
      setTimeout(function () { d.remove(); }, 950);
    } catch (e) {}
  }

  // ---------- actions ----------
  function center(el) {
    var r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  }
  function mouse(el, type, p, extra) {
    var init = { bubbles: true, cancelable: true, composed: true, view: window, clientX: p.x, clientY: p.y, button: (extra && extra.button) || 0, buttons: type.indexOf("down") >= 0 ? 1 : 0, detail: (extra && extra.detail) || 1 };
    var Ev = type.indexOf("pointer") === 0 && typeof PointerEvent === "function" ? PointerEvent : MouseEvent;
    if (Ev === PointerEvent) {
      init.pointerId = 1;
      init.pointerType = "mouse";
      init.isPrimary = true;
    }
    return el.dispatchEvent(new Ev(type, init));
  }
  function prepare(el) {
    el.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
    flash(el);
    var p = center(el);
    var hit = document.elementFromPoint(p.x, p.y);
    var covered = hit && hit !== el && !el.contains(hit) && !hit.contains(el);
    return { p: p, hit: covered ? hit : el, covered: covered ? describe(hit) : null };
  }
  kite.click = function (t, opts) {
    opts = opts || {};
    var el = resolve(t);
    if (el.disabled) throw new Error("비활성화된 요소입니다: " + describe(el));
    var pr = prepare(el);
    var target = pr.hit;
    var button = opts.button === "right" ? 2 : opts.button === "middle" ? 1 : 0;
    var times = opts.double ? 2 : 1;
    for (var i = 1; i <= times; i++) {
      mouse(target, "pointerover", pr.p);
      mouse(target, "mouseover", pr.p);
      mouse(target, "pointerdown", pr.p, { button: button, detail: i });
      mouse(target, "mousedown", pr.p, { button: button, detail: i });
      if (typeof target.focus === "function" && i === 1) target.focus({ preventScroll: true });
      mouse(target, "pointerup", pr.p, { button: button, detail: i });
      mouse(target, "mouseup", pr.p, { button: button, detail: i });
      if (button === 2) {
        mouse(target, "contextmenu", pr.p, { button: 2 });
      } else if (button === 0) {
        // click() runs default actions too (follow links, toggle checkboxes, submit).
        if (typeof target.click === "function") target.click();
        else mouse(target, "click", pr.p, { detail: i });
      }
    }
    if (opts.double) mouse(target, "dblclick", pr.p, { detail: 2 });
    return { element: describe(el), covered: pr.covered };
  };
  kite.hover = function (t) {
    var el = resolve(t);
    var pr = prepare(el);
    ["pointerover", "pointerenter", "mouseover", "mouseenter", "pointermove", "mousemove"].forEach(function (ty) {
      mouse(pr.hit, ty, pr.p);
    });
    return { element: describe(el) };
  };
  function setNativeValue(el, v) {
    var proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : el.tagName === "SELECT" ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    var d = Object.getOwnPropertyDescriptor(proto, "value");
    if (d && d.set) d.set.call(el, v);
    else el.value = v;
  }
  function key(el, type, k) {
    var code = k.length === 1 ? (/[a-z]/i.test(k) ? "Key" + k.toUpperCase() : /\d/.test(k) ? "Digit" + k : "") : k;
    var codes = { Enter: 13, Tab: 9, Escape: 27, Backspace: 8, Delete: 46, ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39, " ": 32, Space: 32, Home: 36, End: 35, PageUp: 33, PageDown: 34 };
    var kc = codes[k] || (k.length === 1 ? k.toUpperCase().charCodeAt(0) : 0);
    return el.dispatchEvent(new KeyboardEvent(type, { key: k === "Space" ? " " : k, code: code || k, keyCode: kc, which: kc, bubbles: true, cancelable: true, composed: true }));
  }
  function insertText(el, text) {
    var ok = false;
    try {
      ok = document.execCommand("insertText", false, text);
    } catch (e) {}
    if (!ok) {
      if ("value" in el && el.tagName !== "SELECT") {
        var s = el.selectionStart, e2 = el.selectionEnd;
        var v = el.value || "";
        if (typeof s === "number" && typeof e2 === "number") setNativeValue(el, v.slice(0, s) + text + v.slice(e2));
        else setNativeValue(el, v + text);
      } else if (el.isContentEditable) {
        el.textContent += text;
      }
      el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: text }));
    }
  }
  function submitFrom(el) {
    var form = el.form || (el.closest && el.closest("form"));
    if (form) {
      if (typeof form.requestSubmit === "function") form.requestSubmit();
      else form.submit();
      return true;
    }
    return false;
  }
  kite.type = function (t, text, opts) {
    opts = opts || {};
    var el = resolve(t);
    var pr = prepare(el);
    if (pr.covered && pr.hit && (pr.hit.tagName === "INPUT" || pr.hit.tagName === "TEXTAREA")) el = pr.hit;
    el.focus({ preventScroll: true });
    if (opts.clear !== false) {
      if ("value" in el && el.tagName !== "SELECT") {
        if (typeof el.select === "function") el.select();
        if (el.value) {
          var cleared = false;
          try {
            cleared = document.execCommand("delete", false);
          } catch (e) {}
          if (!cleared || el.value) {
            setNativeValue(el, "");
            el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "deleteContentBackward" }));
          }
        }
      } else if (el.isContentEditable) {
        var range = document.createRange();
        range.selectNodeContents(el);
        var sel = getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
        try { document.execCommand("delete", false); } catch (e) {}
      }
    }
    text = String(text == null ? "" : text);
    if (opts.slowly) {
      for (var i = 0; i < text.length; i++) {
        var ch = text.charAt(i);
        if (key(el, "keydown", ch)) {
          key(el, "keypress", ch);
          insertText(el, ch);
        }
        key(el, "keyup", ch);
      }
    } else if (text) {
      insertText(el, text);
    }
    el.dispatchEvent(new Event("change", { bubbles: true }));
    if (opts.submit) {
      var go = key(el, "keydown", "Enter");
      key(el, "keypress", "Enter");
      key(el, "keyup", "Enter");
      if (go) submitFrom(el);
    }
    return { element: describe(el), value: el.type === "password" ? "••••" : valueOf(el, roleOf(el) || "textbox") };
  };
  kite.select = function (t, values) {
    var el = resolve(t);
    if (el.tagName !== "SELECT") throw new Error("select 요소가 아닙니다: " + describe(el));
    flash(el);
    values = (Array.isArray(values) ? values : [values]).map(String);
    var picked = [];
    for (var i = 0; i < el.options.length; i++) {
      var o = el.options[i];
      var hit = values.indexOf(o.value) >= 0 || values.indexOf(clean(o.textContent, 1000)) >= 0;
      if (el.multiple) o.selected = hit;
      else if (hit && !picked.length) o.selected = true;
      if (hit) picked.push(clean(o.textContent, 80));
    }
    if (!picked.length) throw new Error("맞는 옵션이 없습니다: " + values.join(", "));
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return { element: describe(el), selected: picked };
  };
  kite.check = function (t, checked) {
    var el = resolve(t);
    var now = el.checked != null ? !!el.checked : el.getAttribute("aria-checked") === "true";
    if (now !== !!checked) kite.click(t);
    return { element: describe(el), checked: !!checked };
  };
  kite.press = function (k, t) {
    var el = t && (t.ref || t.selector || t.text) ? resolve(t) : document.activeElement || document.body;
    if (t && (t.ref || t.selector || t.text)) el.focus({ preventScroll: true });
    // "Control+a" style combos: modifiers are reported, only the last key is pressed.
    var parts = String(k).split("+");
    var main = parts.pop();
    var mods = {};
    parts.forEach(function (p) { mods[p.toLowerCase()] = true; });
    var init = function (type) {
      var codes = { Enter: 13, Tab: 9, Escape: 27, Backspace: 8, Delete: 46, ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39 };
      var kc = codes[main] || (main.length === 1 ? main.toUpperCase().charCodeAt(0) : 0);
      return new KeyboardEvent(type, {
        key: main, code: main.length === 1 && /[a-z]/i.test(main) ? "Key" + main.toUpperCase() : main, keyCode: kc, which: kc,
        ctrlKey: !!(mods.control || mods.ctrl), shiftKey: !!mods.shift, altKey: !!mods.alt, metaKey: !!(mods.meta || mods.cmd),
        bubbles: true, cancelable: true, composed: true,
      });
    };
    var go = el.dispatchEvent(init("keydown"));
    if (go && main.length === 1 && !mods.control && !mods.ctrl && !mods.meta) el.dispatchEvent(init("keypress"));
    var done = "";
    if (go) {
      if (main === "Enter") {
        if (el.tagName === "TEXTAREA" || el.isContentEditable) insertText(el, "\n");
        else if (el.tagName === "INPUT" && submitFrom(el)) done = "form submitted";
        else if (el.tagName === "BUTTON" || el.tagName === "A") { el.click(); done = "activated"; }
      } else if (main === "Tab") {
        var f = Array.prototype.filter.call(document.querySelectorAll("a[href],button,input,select,textarea,[tabindex]"), function (x) {
          return !x.disabled && x.tabIndex >= 0 && isVisible(x) && hasSize(x);
        });
        var i = f.indexOf(document.activeElement);
        var nx = f[(i + (mods.shift ? f.length - 1 : 1)) % f.length];
        if (nx) { nx.focus(); done = "focus → " + describe(nx); }
      } else if (main === "Backspace" || main === "Delete") {
        try { document.execCommand(main === "Backspace" ? "delete" : "forwardDelete", false); } catch (e) {}
      } else if ((mods.control || mods.ctrl || mods.meta) && main.toLowerCase() === "a") {
        if (typeof el.select === "function") el.select();
        else document.execCommand("selectAll", false);
      } else if (main.length === 1 && !mods.control && !mods.ctrl && !mods.meta) {
        insertText(el, main);
      } else if (main === "Escape" && document.activeElement && document.activeElement.blur) {
        // let the page handle it; nothing native to do
      } else if (main === "PageDown" || main === "PageUp" || main === "End" || main === "Home") {
        var h = window.innerHeight * 0.9;
        if (main === "PageDown") scrollBy(0, h);
        else if (main === "PageUp") scrollBy(0, -h);
        else if (main === "End") scrollTo(0, document.documentElement.scrollHeight);
        else scrollTo(0, 0);
      }
    }
    el.dispatchEvent(init("keyup"));
    return { key: k, target: describe(el), result: done || (go ? "dispatched" : "prevented by page") };
  };
  kite.scroll = function (t, dx, dy, to) {
    var el = t && (t.ref || t.selector || t.text) ? resolve(t) : null;
    var box = el || document.scrollingElement || document.documentElement;
    if (el && !to && !dx && !dy) {
      el.scrollIntoView({ block: "center", behavior: "instant" });
    } else if (to === "top") {
      box.scrollTo(0, 0);
    } else if (to === "bottom") {
      box.scrollTo(0, box.scrollHeight);
    } else {
      box.scrollBy(dx || 0, dy || 0);
    }
    var s = document.scrollingElement || document.documentElement;
    return { scrollY: Math.round(s.scrollTop), scrollHeight: s.scrollHeight, viewport: window.innerHeight };
  };
  kite.text = function (selector, max) {
    var root = selector ? document.querySelector(selector) : document.body;
    if (!root) throw new Error("selector에 맞는 요소가 없습니다: " + selector);
    var s = String(textOf(root) || "").replace(/\n{3,}/g, "\n\n");
    max = max || 30000;
    return s.length > max ? s.slice(0, max) + "\n…(잘림, 전체 " + s.length + "자)" : s;
  };
  kite.html = function (selector, max) {
    var root = selector ? document.querySelector(selector) : document.documentElement;
    if (!root) throw new Error("selector에 맞는 요소가 없습니다: " + selector);
    var s = root.outerHTML;
    max = max || 50000;
    return s.length > max ? s.slice(0, max) + "\n<!-- …잘림, 전체 " + s.length + "자 -->" : s;
  };
  kite.rect = function (t) {
    var el = resolve(t);
    el.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
    var r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height, vw: window.innerWidth, vh: window.innerHeight, dpr: window.devicePixelRatio || 1 };
  };
  kite.check_state = function (o) {
    var body = String((document.body && textOf(document.body)) || "");
    if (o.text != null && body.indexOf(o.text) < 0) return false;
    if (o.textGone != null && body.indexOf(o.textGone) >= 0) return false;
    if (o.selector) {
      var el = document.querySelector(o.selector);
      if (!el || !isVisible(el)) return false;
    }
    if (o.selectorGone) {
      var g = document.querySelector(o.selectorGone);
      if (g && isVisible(g)) return false;
    }
    return document.readyState !== "loading";
  };
  kite.info = function () {
    return { url: location.href, title: document.title, readyState: document.readyState };
  };
  kite.favicon = function () {
    var l = document.querySelector('link[rel~="icon"][href],link[rel="shortcut icon"][href],link[rel="apple-touch-icon"][href]');
    if (l) return l.href;
    return /^https?:$/.test(location.protocol) ? location.origin + "/favicon.ico" : "";
  };
  kite.snapshot = snapshot;
  // ---------- blue light filter ----------
  // A warm tint over the page (multiply: takes out blue, keeps the page usable; never in the way
  // of clicks). Level 0..3 from Kite: in this script at creation, then `warm(n)` on change.
  var WARM = [0, 0.12, 0.22, 0.34];
  kite.warm = function (n) {
    if (window.top !== window) return;
    var root = document.documentElement;
    if (!root) return;
    var el = document.getElementById("__kite_warm");
    var a = WARM[n | 0] || 0;
    if (!a) {
      if (el) el.remove();
      return;
    }
    if (!el) {
      el = document.createElement("div");
      el.id = "__kite_warm";
      el.setAttribute("aria-hidden", "true");
      el.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483645;background:rgb(255,147,41);mix-blend-mode:multiply;transition:opacity .25s;";
      root.appendChild(el);
    }
    el.style.opacity = String(a);
  };
  (function startWarm(n) {
    if (!n) return;
    if (document.documentElement) kite.warm(n);
    else document.addEventListener("readystatechange", function once() {
      if (!document.documentElement) return;
      document.removeEventListener("readystatechange", once);
      kite.warm(n);
    });
  })(Number("__KITE_WARM__") || 0);

  // ---------- split view ----------
  // A click in a page that is one pane of a split view (it gets focus): Rust makes its tab the
  // active one (ignored when the tab is not in a split view). Frames report too: a click in an
  // iframe focuses the frame, not this window.
  window.addEventListener("focus", function () { send("pane"); });
  // Kite's split view tells each pane its part (null: not a pane any more):
  // { mark: active pane (purple frame), left / top: boundary ids ("v:<row>:<col>", "h:<row>:0")
  // of the edges this pane shares with its neighbour, "" for none }. Those edges get a see-through
  // grab strip; dragging it reports the pointer travel (as a share of this pane's size, which
  // stays right while the pane resizes) and Rust moves the boundary. Scrollbars turn see-through
  // too. Only the left/top edges: the right and bottom ones hold the page's scrollbars.
  var paneGrips = [];
  var paneKey = "";
  kite.pane = function (o) {
    if (window.top !== window) return;
    var root = document.documentElement;
    if (!root) return;
    // Theme accent from Kite (the tab strip's split line); only plain color text.
    var color = o && /^[#(),. %a-zA-Z0-9]{1,40}$/.test(o.color || "") ? o.color : "#6d4aff";
    var frame = document.getElementById("__kite_pane");
    if (o && o.mark && !frame) {
      frame = document.createElement("div");
      frame.id = "__kite_pane";
      frame.setAttribute("aria-hidden", "true");
      frame.style.cssText = "position:fixed;inset:0;border:2px solid;pointer-events:none;z-index:2147483646;box-sizing:border-box;";
      root.appendChild(frame);
    } else if (!(o && o.mark) && frame) frame.remove();
    if (frame && o && o.mark) frame.style.borderColor = color;
    // Same edges (a click in this pane just made it the active one, maybe mid-drag): keep them.
    var key = o ? (o.left || "") + "|" + (o.top || "") + "|" + color : "";
    var keep = o && key === paneKey && paneGrips.every(function (g) { return g.isConnected; });
    if (!keep) {
      paneGrips.forEach(function (g) { g.remove(); });
      paneGrips = [];
    }
    paneKey = key;
    var css = document.getElementById("__kite_pane_css");
    if (!o) {
      if (css) css.remove();
      return;
    }
    if (!css) {
      css = document.createElement("style");
      css.id = "__kite_pane_css";
      css.textContent =
        "html{scrollbar-width:thin;scrollbar-color:rgba(128,128,128,.4) transparent}" +
        "::-webkit-scrollbar{width:8px;height:8px;background:transparent}" +
        "::-webkit-scrollbar-track,::-webkit-scrollbar-corner{background:transparent}" +
        "::-webkit-scrollbar-thumb{background:rgba(128,128,128,.4);border-radius:4px}";
      root.appendChild(css);
    }
    if (keep) return;
    [["left", o.left], ["top", o.top]].forEach(function (e) {
      if (!e[1]) return;
      var g = document.createElement("div");
      var v = e[0] === "left";
      g.setAttribute("aria-hidden", "true");
      // The boundary: a 2px line in the accent along the edge (the strip's split line color),
      // inside a 6px strip that takes the drag.
      var line = v ? "linear-gradient(to right," + color + " 2px,transparent 2px)" : "linear-gradient(to bottom," + color + " 2px,transparent 2px)";
      g.style.cssText = "position:fixed;z-index:2147483647;transition:opacity .12s;opacity:.75;" + (v ? "left:0;top:0;bottom:0;width:6px;cursor:col-resize;" : "left:0;right:0;top:0;height:6px;cursor:row-resize;");
      g.style.background = line;
      var drag = null;
      var queued = false;
      var hot = function (on) {
        g.style.background = on ? color : line;
        g.style.opacity = on ? ".55" : ".75";
      };
      g.addEventListener("pointerenter", function () { hot(true); });
      g.addEventListener("pointerleave", function () { if (!drag) hot(false); });
      g.addEventListener("pointerdown", function (ev) {
        if (ev.button !== 0) return;
        ev.preventDefault();
        ev.stopPropagation();
        try { g.setPointerCapture(ev.pointerId); } catch (x) {}
        drag = { x: ev.screenX, y: ev.screenY, w: window.innerWidth || 1, h: window.innerHeight || 1, nx: 0, ny: 0 };
        send("split", "start:" + e[1]);
      });
      g.addEventListener("pointermove", function (ev) {
        if (!drag) return;
        drag.nx = (ev.screenX - drag.x) / drag.w;
        drag.ny = (ev.screenY - drag.y) / drag.h;
        if (queued) return;
        queued = true;
        requestAnimationFrame(function () {
          queued = false;
          if (drag) send("split", "move:" + drag.nx.toFixed(4) + ":" + drag.ny.toFixed(4));
        });
      });
      var end = function () {
        if (!drag) return;
        drag = null;
        hot(false);
        send("split", "end");
      };
      g.addEventListener("pointerup", end);
      g.addEventListener("pointercancel", end);
      g.addEventListener("lostpointercapture", end);
      root.appendChild(g);
      paneGrips.push(g);
    });
  };
  kite.keys = function () { return { keys: KEYS, last: lastCombo }; };
  kite.describe = describe;
  kite.resolve = resolve;

  // ---------- results of async scripts (polled by the app) ----------
  var results = {};
  kite.serialize = function (v) {
    var seen = [];
    function ser(x, depth) {
      if (x === undefined) return null;
      if (x === null || typeof x === "number" || typeof x === "boolean" || typeof x === "string") return x;
      if (typeof x === "bigint") return String(x);
      if (typeof x === "function") return "[function]";
      if (typeof Node === "function" && x instanceof Node) return x.nodeType === 1 ? clean(x.outerHTML, 500) : clean(x.textContent, 500);
      if (depth > 6) return "[…]";
      if (seen.indexOf(x) >= 0) return "[circular]";
      seen.push(x);
      if (Array.isArray(x) || (typeof NodeList === "function" && x instanceof NodeList)) {
        var arr = [];
        for (var i = 0; i < x.length && i < 500; i++) arr.push(ser(x[i], depth + 1));
        return arr;
      }
      if (x instanceof Error) return { error: String(x.message || x) };
      var o = {};
      var keys = Object.keys(x).slice(0, 200);
      for (var j = 0; j < keys.length; j++) {
        try { o[keys[j]] = ser(x[keys[j]], depth + 1); } catch (e) { o[keys[j]] = "[error]"; }
      }
      return o;
    }
    return ser(v, 0);
  };
  kite.start = function (id, fn) {
    results[id] = null;
    Promise.resolve()
      .then(fn)
      .then(
        function (v) { results[id] = JSON.stringify({ ok: true, value: kite.serialize(v) }); },
        function (e) { results[id] = JSON.stringify({ ok: false, error: String((e && e.message) || e) }); }
      );
  };

  // ---------- passwords ----------
  // A password field on the page: tell the app (it may fill a saved login through
  // `kite.fillLogin`). A login sent: give the app what was typed; it reads the site from the
  // webview itself and asks the user before saving anything.
  if (/^https?:$/.test(location.protocol)) {
    var visible = function (el) {
      if (el.disabled || el.readOnly) return false;
      var r = el.getBoundingClientRect();
      if (r.width < 2 || r.height < 2) return false;
      var cs = getComputedStyle(el);
      return cs.visibility !== "hidden" && cs.display !== "none";
    };
    var pwFields = function (root) {
      return Array.prototype.filter.call((root || document).querySelectorAll('input[type="password"]'), visible);
    };
    // The username field: autocomplete=username, else the last text-like input before the password.
    var userFieldFor = function (pw) {
      var scope = pw.form || document;
      var tagged = scope.querySelector('input[autocomplete~="username"]');
      if (tagged && visible(tagged)) return tagged;
      var inputs = scope.querySelectorAll("input");
      var found = null;
      for (var i = 0; i < inputs.length; i++) {
        var el = inputs[i];
        if (el === pw) break;
        var t = (el.type || "text").toLowerCase();
        if ((t === "text" || t === "email" || t === "tel") && visible(el)) found = el;
      }
      return found;
    };
    // Through the native setter + input/change events, so React/Vue see the value.
    var setValue = function (el, v) {
      var proto = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
      el.focus();
      if (proto && proto.set) proto.set.call(el, v);
      else el.value = v;
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      el.blur();
    };
    var announced = [];
    var announce = function () {
      var fresh = pwFields().filter(function (f) { return announced.indexOf(f) < 0 && f.autocomplete !== "new-password"; });
      if (!fresh.length || announced.length > 20) return;
      announced = announced.concat(fresh);
      send("password", "form:" + location.origin);
    };
    var lastSent = "";
    var capture = function (root) {
      var pws = pwFields(root).filter(function (f) { return f.value; });
      if (!pws.length && root !== document) pws = pwFields(document).filter(function (f) { return f.value; });
      // Change-password forms (3 fields) are skipped; sign-up forms repeat one value.
      if (!pws.length || pws.length > 2 || (pws.length === 2 && pws[0].value !== pws[1].value)) return;
      var pw = pws[0];
      var uf = userFieldFor(pw);
      var key = (uf ? uf.value : "") + "\u0000" + pw.value;
      if (key === lastSent) return;
      lastSent = key;
      send("password", JSON.stringify({ o: location.origin, u: uf ? uf.value : "", p: pw.value }));
    };
    document.addEventListener("submit", function (e) { capture(e.target); }, true);
    // Many sites log in with a button and fetch(), without a real submit.
    document.addEventListener(
      "click",
      function (e) {
        var btn = e.target && e.target.closest && e.target.closest('button, input[type="submit"], input[type="button"], [role="button"]');
        if (btn) capture(btn.form || btn.closest("form") || document);
      },
      true
    );
    document.addEventListener(
      "keydown",
      function (e) {
        if (e.key === "Enter" && e.target && e.target.tagName === "INPUT") capture(e.target.form || document);
      },
      true
    );
    kite.fillLogin = function (origin, user, pass) {
      if (location.origin !== origin) return false;
      var pw = pwFields().filter(function (f) { return f.autocomplete !== "new-password"; })[0];
      if (!pw) return false;
      var uf = userFieldFor(pw);
      if (uf && user && !uf.value) setValue(uf, user);
      if (!pw.value) setValue(pw, pass);
      lastSent = (uf ? uf.value : "") + "\u0000" + pw.value;
      return true;
    };
    var timer = 0;
    var schedule = function () {
      clearTimeout(timer);
      timer = setTimeout(announce, 400);
    };
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", schedule);
    else schedule();
    window.addEventListener("load", schedule);
    new MutationObserver(schedule).observe(document.documentElement, { childList: true, subtree: true });
  }

  kite.take = function (id) {
    if (!(id in results)) return "__missing__";
    var r = results[id];
    if (r !== null) delete results[id];
    return r;
  };
})();
