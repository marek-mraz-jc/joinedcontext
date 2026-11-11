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
// T-6758: the Subscribe menu's own calendar, built from choices and never typed. The Endpoint's
// facets (EP-178) give each attribute's values with counts; the choices and a date range become
// one NGSI-LD q, every value quoted by jcFeedLink.quote, and the link is the stored output's
// feeds/{filter}/{output}.ics with its RSS, Atom and CSV endings. The preview reads that link.
window.jcFeedLink = (function () {
  // A Query Term value as an RFC 8259 string, so ";|()" and quotes stay inside one value.
  function quote(v) { return JSON.stringify(String(v)); }
  // address.addressLocality is the member address[addressLocality] in q.
  function path(attr) { var p = attr.split("."); return p.length === 2 ? p[0] + "[" + p[1] + "]" : p[0]; }
  // Every byte but the unreserved ones percent-encoded: the whole q is one path segment value.
  function encode(v) { return encodeURIComponent(v).replace(/[!'()*]/g, function (c) { return "%" + c.charCodeAt(0).toString(16).toUpperCase(); }); }
  // choices: [[attr, value]], from/to: YYYY-MM-DD or "". The filter query string, "" for none.
  function filter(choices, from, to) {
    var terms = choices.filter(function (c) { return c[1] !== ""; }).map(function (c) { return path(c[0]) + "==" + quote(c[1]); });
    if (/^\d{4}-\d{2}-\d{2}$/.test(from)) terms.push("startDate>=" + from + "T00:00:00Z");
    if (/^\d{4}-\d{2}-\d{2}$/.test(to)) terms.push("startDate<=" + to + "T23:59:59Z");
    return terms.length ? "q=" + encode(terms.join(";")) : "";
  }
  // The links of one filter: the calendar by path, the other formats by the query aliases.
  function links(base, output, f) {
    var ics = base + "/feeds/" + (f ? f + "/" : "") + output + ".ics";
    var webcal = ics.replace(/^https:/, "webcal:");
    var tail = f ? "?" + f : "";
    return {
      ics: ics, webcal: webcal,
      google: "https://calendar.google.com/calendar/r?cid=" + encodeURIComponent(webcal),
      rss: base + "/entities.rss" + tail, atom: base + "/entities.atom" + tail, csv: base + "/entities.csv" + tail,
    };
  }
  return { quote: quote, filter: filter, links: links };
})();
(function () {
  var box = document.querySelector(".jc-subscribe-query");
  if (!box) return;
  var base = box.getAttribute("data-jc-base") || "", output = box.getAttribute("data-jc-output") || "";
  if (!/^https:\/\/[^\s?#]+\/api\/endpoint\/[A-Za-z0-9]+$/.test(base) || !/^[A-Za-z0-9_-]{1,63}$/.test(output)) return;
  box.hidden = false;
  var fields = document.getElementById("jc-subscribe-facets");
  var noFacets = document.getElementById("jc-subscribe-no-facets");
  var from = document.getElementById("jc-subscribe-from"), to = document.getElementById("jc-subscribe-to");
  var out = document.getElementById("jc-subscribe-mine-url");
  var anchors = {};
  ["webcal", "google", "ics", "rss", "atom", "csv"].forEach(function (k) { anchors[k] = document.getElementById("jc-subscribe-mine-" + k); });
  var list = document.getElementById("jc-subscribe-preview");
  var status = document.getElementById("jc-subscribe-preview-status");
  var menu = box.closest("details");
  var lang = box.getAttribute("data-jc-lang") || "en";
  var facets = [], timer = null, asked = 0, loaded = false;
  var copyLabel = box.querySelector(".jc-code-copy-label");
  function say(key, vars) {
    return String(box.getAttribute(key) || "").replace(/\{(\w+)\}/g, function (_, k) { return vars && vars[k] !== undefined ? vars[k] : ""; });
  }
  function read(url) {
    return fetch(url, { credentials: "omit", referrerPolicy: "no-referrer", headers: { Accept: "application/json" } })
      .then(function (r) { return r.ok ? r.json() : null; }).catch(function () { return null; });
  }
  function label(attr) {
    var leaf = attr.split(".").pop();
    if (/locality|town|city|municipality/i.test(leaf)) return box.getAttribute("data-jc-town");
    if (/categor|genre/i.test(leaf)) return box.getAttribute("data-jc-genre");
    return leaf.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/^./, function (c) { return c.toUpperCase(); });
  }
  // One facet: a text field over a list of its values with their counts; only a listed value counts.
  function field(attr, values, i) {
    var id = "jc-subscribe-facet-" + i, wrap = document.createElement("div");
    var l = document.createElement("label"); l.htmlFor = id; l.textContent = label(attr);
    var input = document.createElement("input");
    input.id = id; input.className = "form-control"; input.type = "text"; input.autocomplete = "off";
    input.setAttribute("list", id + "-values"); input.setAttribute("aria-describedby", id + "-hint");
    var options = document.createElement("datalist"); options.id = id + "-values";
    values.forEach(function (v) {
      var o = document.createElement("option"); o.value = v.value;
      o.label = say("data-jc-count", { count: Number(v.count).toLocaleString(lang) });
      options.appendChild(o);
    });
    var hint = document.createElement("p"); hint.id = id + "-hint"; hint.className = "jc-subscribe-note";
    hint.textContent = say("data-jc-hint", { example: values[0].value });
    wrap.appendChild(l); wrap.appendChild(input); wrap.appendChild(options); wrap.appendChild(hint);
    fields.appendChild(wrap);
    var known = values.map(function (v) { return v.value; });
    return { attr: attr, input: input, hint: hint, values: known, example: values[0].value };
  }
  // The listed value a field holds, matched without regard to case; "" when empty or not listed.
  function chosen(f) {
    var typed = f.input.value.trim(), hit = "";
    f.values.forEach(function (v) { if (!hit && v.toLocaleLowerCase(lang) === typed.toLocaleLowerCase(lang)) hit = v; });
    var bad = typed !== "" && !hit;
    if (bad) f.input.setAttribute("aria-invalid", "true"); else f.input.removeAttribute("aria-invalid");
    f.hint.textContent = bad ? say("data-jc-not-listed", { value: typed }) : say("data-jc-hint", { example: f.example });
    f.hint.classList.toggle("jc-subscribe-error", bad);
    return hit;
  }
  function update() {
    var picked = facets.map(function (f) { return [f.attr, chosen(f)]; });
    // A value outside the list leaves no link to copy: the copy row and links hide until it reads.
    var invalid = facets.some(function (f) { return f.input.getAttribute("aria-invalid") === "true"; });
    box.toggleAttribute("data-jc-refused", invalid);
    if (invalid) { clearTimeout(timer); asked++; list.textContent = ""; status.textContent = ""; return; }
    var l = jcFeedLink.links(base, output, jcFeedLink.filter(picked, from.value, to.value));
    if (copyLabel && copyLabel.hasAttribute("data-label")) { clearTimeout(copyLabel.jcTimer); copyLabel.textContent = copyLabel.getAttribute("data-label"); }
    out.value = l.ics;
    Object.keys(anchors).forEach(function (k) { if (anchors[k]) anchors[k].href = l[k]; });
    clearTimeout(timer);
    if (menu && !menu.open) return;
    asked++;
    list.textContent = "";
    status.textContent = box.getAttribute("data-jc-loading");
    timer = setTimeout(function () { preview(l.ics); }, 400);
  }
  // The facets the anonymous reader may see, each with its most frequent string values.
  function load() {
    if (loaded) return;
    loaded = true;
    read(base + "/facets").then(function (answer) {
      var names = (answer && Array.isArray(answer.facets) ? answer.facets : []).filter(function (a) { return typeof a === "string"; });
      return Promise.all(names.map(function (a) { return read(base + "/facets?attr=" + encodeURIComponent(a) + "&limit=100").then(function (r) { return [a, r]; }); }));
    }).then(function (answers) {
      answers.forEach(function (pair) {
        var values = (pair[1] && Array.isArray(pair[1].values) ? pair[1].values : [])
          .filter(function (v) { return v && typeof v.value === "string" && v.value !== ""; });
        if (values.length) facets.push(field(pair[0], values, facets.length + 1));
      });
      noFacets.hidden = facets.length > 0;
      facets.forEach(function (f) { f.input.addEventListener("input", update); f.input.addEventListener("change", update); });
      update();
    });
  }
  // One calendar line as its value: the line unfolded, its text escapes read (RFC 5545 3.3.11).
  function unescapeText(v) { return v.replace(/\\n/gi, " ").replace(/\\([,;\\])/g, "$1"); }
  function events(text) {
    var lines = text.replace(/\r?\n[ \t]/g, "").split(/\r?\n/), found = [], cur = null;
    lines.forEach(function (l) {
      if (l === "BEGIN:VEVENT") cur = {};
      else if (l === "END:VEVENT") { if (cur && cur.start) found.push(cur); cur = null; }
      else if (cur) {
        var m = /^(SUMMARY|DTSTART)(;[^:]*)?:(.*)$/.exec(l);
        if (m && m[1] === "SUMMARY") cur.summary = unescapeText(m[3]);
        if (m && m[1] === "DTSTART") cur.start = m[3];
      }
    });
    return found;
  }
  // 20261111, 20261111T180000 or 20261111T170000Z as a Date (a floating time read as local).
  function when(v) {
    var m = /^(\d{4})(\d{2})(\d{2})(T(\d{2})(\d{2})(\d{2})(Z)?)?$/.exec(v);
    if (!m) return null;
    if (!m[4]) return { date: new Date(+m[1], +m[2] - 1, +m[3]), allDay: true };
    var args = [+m[1], +m[2] - 1, +m[3], +m[5], +m[6], +m[7]];
    return { date: m[8] ? new Date(Date.UTC.apply(null, args)) : new Date(args[0], args[1], args[2], args[3], args[4], args[5]), allDay: false };
  }
  function preview(url) {
    var mine = ++asked;
    list.textContent = "";
    status.textContent = box.getAttribute("data-jc-loading");
    fetch(url, { credentials: "omit", referrerPolicy: "no-referrer", headers: { Accept: "text/calendar" } }).then(function (r) {
      if (!r.ok) {
        return r.json().catch(function () { return {}; }).then(function (p) {
          if (mine !== asked) return;
          box.toggleAttribute("data-jc-refused", true);
          status.textContent = say("data-jc-failed", { status: r.status, detail: String((p && (p.detail || p.title)) || r.status).slice(0, 300) });
        });
      }
      return r.text().then(function (text) {
        if (mine !== asked) return;
        box.toggleAttribute("data-jc-refused", false);
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
  from.addEventListener("change", update);
  to.addEventListener("change", update);
  if (menu) menu.addEventListener("toggle", function () { if (menu.open) { load(); update(); } });
  if (!menu || menu.open) load();
  update();
})();
