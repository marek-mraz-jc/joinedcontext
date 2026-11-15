// Embedding the live explorer on another site (T-5129): the address of /embed.html for one public
// Endpoint, the snippet the explorer's "Embed" dialog hands out, and <jc-data-explorer>, the web
// component variant of the same iframe. The iframe is the whole widget either way: it reads the
// Endpoint anonymously on viz.{domain}, sets no cookie and sees nothing of the page around it.
(function (root) {
  "use strict";

  var SLUG = /^[a-z0-9]{16,64}$/;
  var NAME = /^[A-Za-z][A-Za-z0-9_]{0,127}$/;
  var DATASET = /^[a-z0-9_-]{2,100}$/; // a CKAN dataset name
  var ORDER = /^(id|[A-Za-z][A-Za-z0-9_]{0,127})(;(asc|desc))?$/; // as explore.js parseHash reads it
  var MAX_Q = 2000;
  var VIEWS = ["both", "table"];
  var HEIGHT = { min: 240, max: 2000, fallback: 640 };
  var SANDBOX = "allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox";

  function attr(value) {
    return String(value).replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  // The checked settings of one embed, or an Error naming the first wrong one. `q` is the view's
  // NGSI-LD query and `sort` its order (attr or attr;desc); the public Endpoint checks q again and
  // the view keeps an order only for a column of the type.
  function settings(o) {
    o = o || {};
    if (!SLUG.test(o.slug || "")) throw new Error("endpoint: the Endpoint's slug, 16 to 64 lowercase letters and digits");
    if (o.dataset && !DATASET.test(o.dataset)) throw new Error("dataset: a catalogue dataset name, e.g. bike-stations");
    if (o.type && !NAME.test(o.type)) throw new Error("type: an entity type, e.g. BikeHireDockingStation");
    if (o.lang && o.lang !== "en" && o.lang !== "sk") throw new Error("lang: en or sk");
    if (o.view && VIEWS.indexOf(o.view) < 0) throw new Error("view: both or table");
    if (o.theme && o.theme !== "light" && o.theme !== "dark") throw new Error("theme: light or dark");
    var height = o.height === undefined || o.height === "" ? HEIGHT.fallback : Number(o.height);
    if (!(height >= HEIGHT.min && height <= HEIGHT.max && Math.floor(height) === height)) throw new Error("height: whole pixels from 240 to 2000, e.g. 640");
    var q = String(o.q || "").trim();
    if (q.length > MAX_Q) throw new Error("q: at most 2000 characters");
    if (o.sort && !ORDER.test(o.sort)) throw new Error("sort: a column, optionally ;desc, e.g. name;desc");
    return { slug: o.slug, dataset: o.dataset || null, type: o.type || null, lang: o.lang || null, view: o.view || "both", theme: o.theme || null, height: height, q: q, sort: o.sort || null };
  }

  // The fragment /embed.html reads (explore.js parseHash): `#{slug}&dataset=…&type=…&lang=…&view=…
  // &theme=…&q=…&sort=…`, q and sort URI-encoded, so `&`, `#` and `=` in a value stay in it.
  function hash(o) {
    var s = settings(o);
    var e = encodeURIComponent;
    var parts = [s.slug];
    if (s.dataset) parts.push("dataset=" + s.dataset);
    if (s.type) parts.push("type=" + s.type);
    if (s.lang) parts.push("lang=" + s.lang);
    if (s.view !== "both") parts.push("view=" + s.view);
    if (s.theme) parts.push("theme=" + s.theme);
    if (s.q) parts.push("q=" + e(s.q));
    if (s.sort) parts.push("sort=" + e(s.sort));
    return "#" + parts.join("&");
  }

  // The code a site pastes: `iframe` (no script, fits a WordPress "Custom HTML" block) or
  // `component` (our script plus <jc-data-explorer>, which grows the frame to its content).
  function snippet(kind, o, origin, title) {
    var s = settings(o);
    if (!/^https:\/\/viz\.[a-z0-9.-]+$/.test(origin || "")) throw new Error("origin: https://viz.{domain}");
    var name = attr(String(title || "Live data").slice(0, 200));
    if (kind === "component") {
      var a = ['endpoint="' + s.slug + '"'];
      if (s.dataset) a.push('dataset="' + s.dataset + '"');
      if (s.type) a.push('type="' + s.type + '"');
      if (s.lang) a.push('lang="' + s.lang + '"');
      if (s.view !== "both") a.push('view="' + s.view + '"');
      if (s.theme) a.push('theme="' + s.theme + '"');
      if (s.q) a.push('q="' + attr(s.q) + '"');
      if (s.sort) a.push('sort="' + s.sort + '"');
      a.push('height="' + s.height + '"', 'title="' + name + '"');
      return '<script src="' + origin + '/embed.js" defer></script>\n<jc-data-explorer ' + a.join(" ") + "></jc-data-explorer>";
    }
    return '<iframe src="' + origin + "/embed.html" + attr(hash(o)) + '" title="' + name + '" width="100%" height="' + s.height +
      '" style="border:0;max-width:100%" loading="lazy" referrerpolicy="no-referrer" sandbox="' + SANDBOX + '"></iframe>';
  }

  // The web component: reads its attributes once, refuses wrong ones with a visible message, and
  // follows the frame's own height messages (from viz.{domain} and that frame only).
  function define(win, origin) {
    if (!win.customElements || win.customElements.get("jc-data-explorer")) return;
    win.customElements.define("jc-data-explorer", class extends win.HTMLElement {
      connectedCallback() {
        if (this.shadowRoot) return;
        var shadow = this.attachShadow({ mode: "open" });
        var get = this.getAttribute.bind(this);
        var style = win.document.createElement("style");
        style.textContent = ":host{display:block}iframe{display:block;width:100%;border:0}p{font:15px system-ui,sans-serif;color:#991b1b;margin:0}";
        shadow.appendChild(style);
        var o;
        try {
          o = { slug: get("endpoint"), dataset: get("dataset"), type: get("type"), lang: get("lang"), view: get("view"), theme: get("theme"), height: get("height") || undefined, q: get("q"), sort: get("sort") };
          o.src = origin + "/embed.html" + hash(o);
        } catch (e) {
          var p = win.document.createElement("p");
          p.setAttribute("role", "alert");
          p.textContent = "This data widget is set up wrongly (" + e.message + "). Copy the snippet again from the dataset's Embed button.";
          return shadow.appendChild(p);
        }
        var frame = win.document.createElement("iframe");
        frame.src = o.src;
        frame.title = get("title") || "Live data";
        frame.loading = "lazy";
        frame.referrerPolicy = "no-referrer";
        frame.setAttribute("sandbox", SANDBOX);
        frame.style.height = settings(o).height + "px";
        shadow.appendChild(frame);
        this._listen = function (event) {
          var h = event.data && event.data.type === "jc-viz-height" && Number(event.data.height);
          if (event.origin === origin && event.source === frame.contentWindow && h > 0) frame.style.height = Math.min(Math.max(h, HEIGHT.min), 4000) + "px";
        };
        win.addEventListener("message", this._listen);
      }
      disconnectedCallback() {
        if (this._listen) win.removeEventListener("message", this._listen);
      }
    });
  }

  root.jcEmbed = { settings: settings, hash: hash, snippet: snippet, define: define, HEIGHT: HEIGHT };
  if (typeof module === "object" && module.exports) module.exports = root.jcEmbed;
  else if (typeof document !== "undefined" && document.currentScript && /\/embed\.js(\?|$)/.test(document.currentScript.src)) {
    // Loaded by another site: the frames come from the host this script was served by. The
    // explorer loads it too, for the snippets alone.
    var origin = new URL(document.currentScript.src).origin;
    if (origin !== location.origin) define(root, origin);
  }
})(typeof self !== "undefined" ? self : globalThis);
