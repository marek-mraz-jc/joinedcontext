// The renderer frame (ADR-N-053 §2.4, §2.5). It runs sandboxed to an opaque origin under a CSP
// that allows no connection, so a spec that got past every check still reads no cookie and calls
// no host. It takes one message from the host page, checks the spec again and draws it through
// Vega's expression interpreter, never through generated code.
(function () {
  "use strict";

  var chart = document.getElementById("chart");
  var host = location.origin; // the host page's origin: the URL's, ours is opaque
  var domain = location.hostname.replace(/^viz\./, "");

  function tell(message) {
    window.parent.postMessage(message, host);
  }

  function height() {
    tell({ type: "jc-viz-height", height: Math.ceil(document.body.getBoundingClientRect().height) });
  }

  // Vega asks its loader for nothing a checked spec names; any ask is refused all the same.
  var refused = function () {
    return Promise.reject(new Error("the renderer loads nothing"));
  };
  var loader = { load: refused, sanitize: refused, http: refused, file: refused };

  function draw(message) {
    var errors = self.jcRefuse(message.spec, domain);
    if (errors.length) throw new Error("The chart was refused: " + errors[0].pointer + " — " + errors[0].rule + ".");
    var data = message.data || {};
    var rows = data.format === "csv" ? vega.read(String(data.text || ""), { type: "csv", parse: "auto" }) : data.rows || [];
    return vegaEmbed(chart, withRows(message.spec, rows), {
      ast: true,
      expr: vega.expressionInterpreter,
      loader: loader,
      renderer: "svg",
      // Export only: no source, no compiled spec, no "open in the editor" (which posts the spec
      // to another site).
      actions: { export: true, source: false, compiled: false, editor: false },
      downloadFileName: "chart"
    });
  }

  // The rows go in as the named data set `source` before the first draw, so the chart never
  // draws empty first: Vega-Lite's top-level `datasets`, Vega's data entry of that name.
  function withRows(spec, rows) {
    var copy = Object.assign({}, spec);
    if (/vega-lite/.test(spec.$schema)) {
      copy.datasets = Object.assign({}, spec.datasets, { source: rows });
      // T-5415: on a phone the legend goes under the chart in two columns with whole labels; at
      // the right it took half the width and cut "Praha 2-Legerova" to "Praha 2-Legero".
      if (window.innerWidth < 480) {
        var config = spec.config || {};
        copy.config = Object.assign({}, config, { legend: Object.assign({}, config.legend, { orient: "bottom", columns: 2, labelLimit: 0 }) });
      }
    } else if (Array.isArray(spec.data)) {
      copy.data = spec.data.map(function (entry) {
        return entry.name === "source" ? Object.assign({}, entry, { values: rows }) : entry;
      });
    }
    return copy;
  }

  window.addEventListener("message", function (event) {
    if (event.source !== window.parent || event.origin !== host) return;
    var message = event.data || {};
    if (message.type !== "jc-viz-render") return;
    document.documentElement.lang = /^[a-z]{2}$/.test(message.lang) ? message.lang : "en";
    Promise.resolve()
      .then(function () { return draw(message); })
      .then(height, function (error) {
        tell({ type: "jc-viz-error", message: String((error && error.message) || error) });
      });
  });
  new ResizeObserver(height).observe(document.body);
  tell({ type: "jc-viz-ready" });
})();
