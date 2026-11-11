// The charts' frames (T-4641): the renderer's host page says how tall it is. Only a message from
// that frame itself, from the renderer's own origin, sets its height. A frame on the Portal's
// origin (jc) is sandboxed to an opaque origin: its data-jc-viz is "null", as its messages say. The live data widget with a
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
      // Where the frame's map starts (explore.js), for jc-viz-near below.
      frame.jcVizMap = typeof data.map === "number" && isFinite(data.map) ? data.map : null;
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
    // An opaque origin cannot be named as a target: the messages (numbers, a word) go to "*".
    var target = frame.getAttribute("data-jc-viz");
    if (!frame.contentWindow) return;
    // Brief 132: a frame's map loads once it comes within one viewport below the reader's window.
    // Inside the frame no observer can tell (its rootMargin counts for nothing across origins).
    if (frame.jcVizMap !== null && frame.jcVizMap !== undefined && !frame.jcVizNear && rect.top + frame.jcVizMap < 2 * window.innerHeight) {
      frame.jcVizNear = true;
      frame.contentWindow.postMessage({ type: "jc-viz-near" }, target === "null" ? "*" : target);
    }
    if (height > 0) {
      frame.contentWindow.postMessage({ type: "jc-viz-viewport", top: Math.round(top), height: Math.round(height) }, target === "null" ? "*" : target);
    }
  });
}
// T-6748: on a phone the closed assistant steps aside while it sits over a frame, so a table's last
// rows and its pager stay readable and tappable (the edge icon of T-5415 still covered them). It is
// back once the frame scrolls past it, and on a keyboard focus (jc-theme.css).
var jcPhone = window.matchMedia("(max-width: 40rem)");
function jcCornerAside() {
  var corner = document.querySelector(".jc-corner .jc-assistant:not([open]) > summary");
  var over = false;
  if (corner && jcPhone.matches) {
    var c = corner.getBoundingClientRect();
    over = Array.prototype.some.call(document.querySelectorAll("iframe[data-jc-viz]"), function (frame) {
      var r = frame.getBoundingClientRect();
      return r.left < c.right && c.left < r.right && r.top < c.bottom && c.top < r.bottom;
    });
  }
  document.body.classList.toggle("jc-corner-aside", over);
}
var jcVizQueued = false;
function jcVizSoon() {
  if (jcVizQueued) return;
  jcVizQueued = true;
  window.requestAnimationFrame(function () { jcVizQueued = false; jcVizViewport(); jcCornerAside(); });
}
window.addEventListener("scroll", jcVizSoon, { passive: true, capture: true });
window.addEventListener("resize", jcVizSoon);
// The assistant closing over a frame steps aside at once; `toggle` does not bubble, so it is caught.
document.addEventListener("toggle", jcVizSoon, true);
document.addEventListener("DOMContentLoaded", jcVizSoon);
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
// Brief 132 (END-16): the Subscribe menu's own calendar. The explorer frame says which type and q
// its table shows (jc-viz-query); the reader may type a q instead. The address is the dataset's
// calendar feed with them, which the gateway reads through the Endpoint's Policy as an NGSI-LD
// query; the preview reads that very address and lists its next five events.
(function () {
  var box = document.querySelector(".jc-subscribe-query");
  if (!box) return;
  var feed = box.getAttribute("data-jc-feed") || "";
  if (!/^https:\/\/[^\s?#]+\/ical\/v1\/[A-Za-z0-9_-]+\.ics$/.test(feed)) return;
  box.hidden = false;
  var field = document.getElementById("jc-subscribe-q");
  var out = document.getElementById("jc-subscribe-mine-url");
  var webcal = document.getElementById("jc-subscribe-mine-webcal");
  var ics = document.getElementById("jc-subscribe-mine-ics");
  var list = document.getElementById("jc-subscribe-preview");
  var status = document.getElementById("jc-subscribe-preview-status");
  var menu = box.closest("details");
  var lang = box.getAttribute("data-jc-lang") || "en";
  var frameType = "", typed = false, timer = null, asked = 0;
  function address() {
    var q = field.value.trim();
    var p = new URLSearchParams();
    if (frameType) p.set("type", frameType);
    if (q) p.set("q", q);
    var s = p.toString();
    return feed + (s ? "?" + s : "");
  }
  var copyLabel = box.querySelector(".jc-code-copy-label");
  // A refused q leaves no address to copy or subscribe to: the field is marked invalid and the
  // server's problem detail stands under it until the q reads again.
  function refused(on) {
    box.toggleAttribute("data-jc-refused", on);
    if (on) field.setAttribute("aria-invalid", "true"); else field.removeAttribute("aria-invalid");
  }
  function update() {
    var url = address();
    // "Copied" spoke of the previous address.
    if (copyLabel && copyLabel.hasAttribute("data-label")) { clearTimeout(copyLabel.jcTimer); copyLabel.textContent = copyLabel.getAttribute("data-label"); }
    out.value = url;
    webcal.href = url.replace(/^https:/, "webcal:");
    ics.href = url;
    clearTimeout(timer);
    if (menu && !menu.open) return;
    // Say "loading" at once, so the heading never stands over nothing while the debounce runs.
    asked++;
    list.textContent = "";
    status.textContent = box.getAttribute("data-jc-loading");
    timer = setTimeout(function () { preview(url); }, 400);
  }
  // One calendar line as its value: the line unfolded, its text escapes read (RFC 5545 3.3.11).
  function unescapeText(v) { return v.replace(/\\n/gi, " ").replace(/\\([,;\\])/g, "$1"); }
  function events(text) {
    var lines = text.replace(/\r?\n[ \t]/g, "").split(/\r?\n/), out = [], cur = null;
    lines.forEach(function (l) {
      if (l === "BEGIN:VEVENT") cur = {};
      else if (l === "END:VEVENT") { if (cur && cur.start) out.push(cur); cur = null; }
      else if (cur) {
        var m = /^(SUMMARY|DTSTART)(;[^:]*)?:(.*)$/.exec(l);
        if (m && m[1] === "SUMMARY") cur.summary = unescapeText(m[3]);
        if (m && m[1] === "DTSTART") cur.start = m[3];
      }
    });
    return out;
  }
  // 20261111, 20261111T180000 or 20261111T170000Z as a Date (a floating time read as local).
  function when(v) {
    var m = /^(\d{4})(\d{2})(\d{2})(T(\d{2})(\d{2})(\d{2})(Z)?)?$/.exec(v);
    if (!m) return null;
    if (!m[4]) return { date: new Date(+m[1], +m[2] - 1, +m[3]), allDay: true };
    var args = [+m[1], +m[2] - 1, +m[3], +m[5], +m[6], +m[7]];
    return { date: m[8] ? new Date(Date.UTC.apply(null, args)) : new Date(args[0], args[1], args[2], args[3], args[4], args[5]), allDay: false };
  }
  function say(key, vars) {
    return String(box.getAttribute(key) || "").replace(/\{(\w+)\}/g, function (_, k) { return vars && vars[k] !== undefined ? vars[k] : ""; });
  }
  function preview(url) {
    var mine = ++asked;
    list.textContent = "";
    status.textContent = box.getAttribute("data-jc-loading");
    fetch(url, { credentials: "omit", referrerPolicy: "no-referrer", headers: { Accept: "text/calendar" } }).then(function (r) {
      if (!r.ok) {
        return r.json().catch(function () { return {}; }).then(function (p) {
          if (mine !== asked) return;
          refused(r.status === 400 || r.status === 403);
          status.textContent = say("data-jc-failed", { status: r.status, detail: String((p && (p.detail || p.title)) || "").slice(0, 300) });
        });
      }
      return r.text().then(function (text) {
        if (mine !== asked) return;
        refused(false);
        var today = new Date(); today.setHours(0, 0, 0, 0);
        var next = events(text).map(function (e) { var w = when(e.start); return w && { summary: e.summary || "", at: w }; })
          .filter(function (e) { return e && e.at.date >= today; })
          .sort(function (a, b) { return a.at.date - b.at.date; }).slice(0, 5);
        status.textContent = next.length ? "" : box.getAttribute("data-jc-none");
        next.forEach(function (e) {
          var f = new Intl.DateTimeFormat(lang, e.at.allDay ? { dateStyle: "medium" } : { dateStyle: "medium", timeStyle: "short" });
          var li = document.createElement("li");
          var time = document.createElement("time");
          time.textContent = f.format(e.at.date);
          li.appendChild(time);
          li.appendChild(document.createTextNode(" " + e.summary));
          list.appendChild(li);
        });
      });
    }).catch(function () {
      if (mine === asked) status.textContent = say("data-jc-failed", { status: "–", detail: "" });
    });
  }
  // Only the explorer frame itself, from its own origin, names the table's type and q; both are
  // text that only ever goes into the address's query string.
  window.addEventListener("message", function (event) {
    var d = event.data || {};
    if (d.type !== "jc-viz-query" || typeof d.q !== "string" || typeof d.entityType !== "string") return;
    var frame = document.querySelector("iframe.jc-explorer-frame");
    if (!frame || frame.contentWindow !== event.source || frame.getAttribute("data-jc-viz") !== event.origin) return;
    if (d.q.length > 2000 || (d.entityType && !/^[A-Za-z][A-Za-z0-9_]{0,127}$/.test(d.entityType))) return;
    if (typed) return;
    frameType = d.entityType;
    field.value = d.q;
    update();
  });
  field.addEventListener("input", function () { typed = true; update(); });
  if (menu) menu.addEventListener("toggle", function () { if (menu.open) update(); });
  update();
})();
