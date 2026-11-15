// T-4685 (API/05 §1.6.1): before the first paint, a page whose assistant was open on the page
// before makes room for it, docked at its width, and holds its first render until the panel is
// parsed, so the panel never shows empty and the page never jumps. jc-assistant.js opens it.
// A catalogue rollout that fails a static file (owner report 2026-10-19 05:55Z) never leaves the
// page shrunk for a panel that does not come: when jc-assistant.js did not run by the time the
// page is parsed, the room is given back.
(function () {
  try {
    var held = JSON.parse(sessionStorage.getItem("jc-assistant") || "null");
    if (!held || held.open !== true) {
      return;
    }
    var root = document.documentElement;
    if (held.mode === "dock") {
      root.classList.add("jc-docked");
    }
    var kept = parseInt(localStorage.getItem("jc-dock-width"), 10);
    if (kept > 0) {
      root.style.setProperty("--jc-dock", kept + "px");
    }
    var expect = document.createElement("link");
    expect.rel = "expect";
    expect.href = "#jc-assistant";
    expect.setAttribute("blocking", "render");
    document.head.appendChild(expect);
    document.addEventListener("DOMContentLoaded", function () {
      if (window.jcAssistant !== true) {
        root.classList.remove("jc-docked");
        expect.remove();
      }
    });
  } catch (e) { /* storage off: the panel opens closed */ }
})();
// T-6748, T-6758, T-7030: the closed assistant corner never covers what a visitor came for. While
// it would sit over a connect card, a code block, a form control or an open Subscribe menu, and on
// a phone over any text, picture or frame of the page, it steps aside (jc-theme.css): the
// launcher fades, a keyboard still reaches it, and it is back over empty room and in its own row
// above the footer. Its area is measured while it shows, so the
// bar going does not shrink the area and flicker it back.
window.jcCornerAside = (function () {
  var phone = window.matchMedia("(max-width: 40rem)");
  var under = ".jc-connect, .jc-code, .jc-copy, .jc-address, input:not([type=hidden]), select, textarea, .jc-subscribe[open] > .jc-subscribe-menu";
  var text = "#content :is(p, li, dt, dd, h1, h2, h3, h4, pre, table, blockquote, figure, img, iframe, .btn)";
  var size = null;
  var queued = false;
  function check() {
    queued = false;
    var corner = document.querySelector(".jc-corner");
    var body = document.body;
    if (!corner || !body || corner.querySelector(".jc-assistant[open]")) {
      if (body) body.classList.remove("jc-corner-aside");
      return;
    }
    var c = corner.getBoundingClientRect();
    if (!body.classList.contains("jc-corner-aside") || size === null) {
      size = { width: c.width, height: c.height };
    }
    var zone = { left: c.right - size.width, right: c.right, top: c.bottom - size.height, bottom: c.bottom };
    var over = Array.prototype.some.call(document.querySelectorAll(under + (phone.matches ? ", " + text : "")), function (el) {
      if (corner.contains(el)) return false;
      var r = el.getBoundingClientRect();
      return r.width > 0 && r.left < zone.right && zone.left < r.right && r.top < zone.bottom && zone.top < r.bottom;
    });
    body.classList.toggle("jc-corner-aside", over);
  }
  function soon() {
    if (queued) return;
    queued = true;
    window.requestAnimationFrame(check);
  }
  window.addEventListener("scroll", soon, { passive: true, capture: true });
  window.addEventListener("resize", soon);
  // The assistant or a Subscribe menu opening or closing; `toggle` does not bubble.
  document.addEventListener("toggle", soon, true);
  document.addEventListener("DOMContentLoaded", soon);
  return soon;
})();
