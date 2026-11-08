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
