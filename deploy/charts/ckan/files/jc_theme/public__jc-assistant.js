// The catalogue's assistant (T-4204, AG-114): Escape or the ✕ in the chat's header (T-4635)
// closes it and gives focus back to its button; the chat's expand button (T-4658) grows it to the
// window and back. On a phone, or expanded, where the open frame takes the screen (jc-theme.css),
// the page behind is inert so focus and a screen reader stay in the chat. Without script the
// button still opens and closes it.
// T-4685 (API/05 §1.6.1): on a wide screen the chat docks to the right edge and the page shrinks
// beside it; the visitor drags its edge. The panel stays open, docked and on the same assistant
// across the catalogue's links (this tab's sessionStorage, read again by jc-dock.js before the
// first paint), and a done answer about one dataset opens that dataset's page. The script runs
// right after the frame is parsed, before the page is first drawn.
(function () {
  var box = document.querySelector("details.jc-assistant");
  if (!box) {
    return;
  }
  // jc/dock.js gives the docked room back unless this ran (a failed load during a rollout).
  window.jcAssistant = true;
  var KEY = "jc-assistant";
  var WIDTH = "jc-dock-width";
  var root = document.documentElement;
  var phone = window.matchMedia("(max-width: 40rem)");
  var wide = window.matchMedia("(min-width: 1100px)");
  var frame = box.querySelector("iframe");
  var origin = frame ? new URL(frame.src, location.href).origin : null;
  var state = { open: false, mode: wide.matches ? "dock" : "panel", widget: null, focused: false };
  try {
    var held = JSON.parse(sessionStorage.getItem(KEY) || "null");
    if (held && typeof held === "object") {
      state.open = held.open === true;
      if (held.mode === "dock" || held.mode === "panel" || held.mode === "full") {
        state.mode = held.mode;
      }
      state.widget = typeof held.widget === "string" ? held.widget : null;
      state.focused = held.focused === true;
    }
  } catch (e) { /* a fresh panel */ }
  function save() {
    try { sessionStorage.setItem(KEY, JSON.stringify(state)); } catch (e) { /* this page only */ }
  }
  // The assistant the panel was open with stays while it stays open: the same host, one widget
  // path, never another address.
  function ours(url) {
    try {
      var to = new URL(url);
      return to.origin === origin && /^\/d\/[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\/widget$/.test(to.pathname) && !to.search && !to.hash;
    } catch (e) {
      return false;
    }
  }

  var silenced = [];
  function silence(on) {
    silenced.forEach(function (el) { el.inert = false; });
    silenced = [];
    if (!on || !(phone.matches || box.classList.contains("jc-full"))) {
      return;
    }
    for (var node = box; node && node !== document.body; node = node.parentElement) {
      Array.prototype.forEach.call(node.parentElement.children, function (el) {
        if (el !== node && !el.inert) {
          el.inert = true;
          silenced.push(el);
        }
      });
    }
  }
  // Docked on a wide screen the page scrolls in its body, not the window (T-4767, jc-theme.css):
  // a change of place keeps the reader where they were, and Back or a reload finds the body's
  // offset again, as the browser does for the window's.
  var SCROLL = "jc-assistant-scroll";
  function scroller() {
    return getComputedStyle(root).overflowY === "hidden" ? document.body : document.scrollingElement || root;
  }
  function place() {
    var top = scroller().scrollTop;
    box.classList.toggle("jc-full", state.mode === "full");
    root.classList.toggle("jc-docked", box.open && state.mode === "dock");
    scroller().scrollTop = top;
    silence(box.open);
  }
  function close() {
    box.open = false;
    box.querySelector("summary").focus();
  }

  // The dock's edge: dragged, or moved with the arrow keys, between 360 px and half the window.
  var edge = document.createElement("div");
  edge.className = "jc-dock-edge";
  edge.tabIndex = 0;
  edge.setAttribute("role", "separator");
  edge.setAttribute("aria-orientation", "vertical");
  var widths = { en: "Assistant width", cs: "Šířka asistenta", de: "Breite des Assistenten", hu: "Az asszisztens szélessége" };
  edge.setAttribute("aria-label", widths[String(root.lang).slice(0, 2)] || "Šírka asistenta");
  function width(px, keep) {
    var most = Math.max(360, Math.floor(window.innerWidth / 2));
    var w = Math.round(Math.min(Math.max(px, 360), most));
    root.style.setProperty("--jc-dock", w + "px");
    edge.setAttribute("aria-valuemin", "360");
    edge.setAttribute("aria-valuemax", String(most));
    edge.setAttribute("aria-valuenow", String(w));
    if (keep) {
      try { localStorage.setItem(WIDTH, String(w)); } catch (e) { /* this tab only */ }
    }
  }
  var kept = NaN;
  try { kept = parseInt(localStorage.getItem(WIDTH), 10); } catch (e) { /* the default */ }
  width(kept > 0 ? kept : window.innerWidth * 0.34, false);
  edge.addEventListener("pointerdown", function (event) {
    edge.setPointerCapture(event.pointerId);
    root.classList.add("jc-dragging");
  });
  edge.addEventListener("pointermove", function (event) {
    if (edge.hasPointerCapture(event.pointerId)) {
      width(window.innerWidth - event.clientX, false);
    }
  });
  edge.addEventListener("pointerup", function (event) {
    edge.releasePointerCapture(event.pointerId);
    root.classList.remove("jc-dragging");
    width(window.innerWidth - event.clientX, true);
  });
  edge.addEventListener("keydown", function (event) {
    var now = parseInt(edge.getAttribute("aria-valuenow"), 10);
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      width(now + (event.key === "ArrowLeft" ? 24 : -24), true);
    }
  });
  box.appendChild(edge);

  // Open on the page before: open again now, on the assistant it had, before the first paint.
  if (state.open && frame) {
    frame.loading = "eager";
    if (state.widget && ours(state.widget) && state.widget !== frame.src.split("?")[0]) {
      frame.src = state.widget + "?embed=1&dock=1&scheme=light";
    }
    box.open = true;
    // Focus stays in the chat across the page change, as it was.
    if (state.focused) {
      frame.focus();
    }
  }
  place();
  try {
    var kept = JSON.parse(sessionStorage.getItem(SCROLL) || "null");
    var arrival = (performance.getEntriesByType("navigation")[0] || {}).type;
    if (kept && kept.url === location.href && typeof kept.top === "number" && (arrival === "back_forward" || arrival === "reload") &&
        scroller() === document.body) {
      document.body.scrollTop = kept.top;
    }
  } catch (e) { /* the page opens at its top */ }
  window.addEventListener("pagehide", function () {
    state.focused = box.open && document.activeElement === frame;
    save();
    try {
      sessionStorage.setItem(SCROLL, JSON.stringify({ url: location.href, top: scroller() === document.body ? document.body.scrollTop : 0 }));
    } catch (e) { /* this page only */ }
  });

  box.addEventListener("toggle", function () {
    state.open = box.open;
    state.widget = box.open && frame ? frame.src.split("?")[0] : null;
    save();
    place();
  });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape" && box.open) {
      close();
    }
  });
  // T-4685: the one dataset a done answer is about, on this catalogue alone; never while the
  // visitor selects text or types in the page, never the page already open.
  function opens(url) {
    var to;
    try { to = new URL(url); } catch (e) { return null; }
    var here = location.pathname.replace(/\/$/, "");
    if (to.origin !== location.origin || !/^\/dataset\/[a-z0-9_-]{2,100}\/?$/.test(to.pathname) || to.pathname.replace(/\/$/, "") === here) {
      return null;
    }
    var active = document.activeElement;
    if (String(window.getSelection ? window.getSelection() : "") !== "" ||
        (active && active !== frame && (/^(INPUT|TEXTAREA|SELECT)$/.test(active.tagName) || active.isContentEditable))) {
      return null;
    }
    return to.pathname;
  }
  // The ✕ in the chat's header (T-4635), its expand button (T-4658), its dock button and its
  // dataset (T-4685): only the frame this page opened, from its own origin, is heard, and each
  // message carries nothing else.
  window.addEventListener("message", function (event) {
    var data = event.data;
    if (!frame || event.source !== frame.contentWindow || event.origin !== origin || !data) {
      return;
    }
    if (box.open && data.type === "jc-assistant-close") {
      close();
    } else if (data.type === "jc-assistant-size" && (data.size === "full" || data.size === "panel" || data.size === "dock")) {
      state.mode = data.size;
      save();
      place();
    } else if (box.open && data.type === "jc-assistant-dataset" && typeof data.url === "string") {
      var path = opens(data.url);
      if (path) {
        frame.contentWindow.postMessage({ type: "jc-assistant-opened", url: data.url }, origin);
        location.assign(path);
      }
    } else if (box.open && data.type === "jc-assistant-back") {
      history.back();
    }
  });
})();
