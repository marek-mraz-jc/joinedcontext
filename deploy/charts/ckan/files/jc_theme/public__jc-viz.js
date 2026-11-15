// The charts' frames (T-4641): the renderer's host page says how tall it is. Only a message from
// that frame itself, from the renderer's own origin, sets its height. The live data widget with a
// 25-row page and its map runs past 2000 px (T-5192: the map was cut off and the frame scrolled
// inside itself), so the cap is a guard against a runaway frame, not a layout limit.
var JC_VIZ_MAX = 20000;
window.addEventListener("message", function (event) {
  var data = event.data || {};
  if (data.type !== "jc-viz-height" || typeof data.height !== "number" || !isFinite(data.height)) {
    return;
  }
  document.querySelectorAll("iframe[data-jc-viz]").forEach(function (frame) {
    if (frame.contentWindow === event.source && frame.getAttribute("data-jc-viz") === event.origin) {
      frame.style.height = Math.min(Math.max(Math.round(data.height), 120), JC_VIZ_MAX) + "px";
    }
  });
});
// T-5192: a frame as tall as its content has no viewport of its own, so a dialog it centres lands
// off screen. Each frame hears which slice of it the reader sees, and opens its dialogs there.
function jcVizViewport() {
  document.querySelectorAll("iframe[data-jc-viz]").forEach(function (frame) {
    var rect = frame.getBoundingClientRect();
    var top = Math.max(0, -rect.top);
    var height = Math.min(window.innerHeight, rect.bottom) - Math.max(0, rect.top);
    if (frame.contentWindow && height > 0) {
      frame.contentWindow.postMessage({ type: "jc-viz-viewport", top: Math.round(top), height: Math.round(height) }, frame.getAttribute("data-jc-viz"));
    }
  });
}
var jcVizQueued = false;
function jcVizSoon() {
  if (jcVizQueued) return;
  jcVizQueued = true;
  window.requestAnimationFrame(function () { jcVizQueued = false; jcVizViewport(); });
}
window.addEventListener("scroll", jcVizSoon, { passive: true, capture: true });
window.addEventListener("resize", jcVizSoon);
window.addEventListener("message", function (event) { if ((event.data || {}).type === "jc-viz-height") jcVizSoon(); });
// T-5192, T-5415: while the live data widget or a chart is in view, the assistant's corner shrinks
// to its icon at the screen's edge.
if (typeof IntersectionObserver === "function") {
  document.addEventListener("DOMContentLoaded", function () {
    var frames = document.querySelectorAll("iframe.jc-viz-frame");
    if (!frames.length) return;
    var seen = new Set();
    var watch = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) { if (e.isIntersecting) seen.add(e.target); else seen.delete(e.target); });
      document.body.classList.toggle("jc-corner-compact", seen.size > 0);
    });
    frames.forEach(function (f) { watch.observe(f); });
  });
}
