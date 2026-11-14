// A CSV text read without generated code (T-6865). d3-dsv, which `vega.read(text, {type: "csv"})`
// parses with, builds its row objects with `new Function`, which the renderer's CSP (no
// 'unsafe-eval') refuses: every chart over a `csv` source failed to draw. This reads RFC 4180
// (quoted fields, doubled quotes, CRLF) into header-keyed objects of strings; the renderer types
// them with `vega.read(records, {type: "json", parse: "auto"})`, which evaluates nothing. One
// function, no dependency, so node runs the same file (crates/portal/ui/tests/viz_render_js.test.ts).
(function (root) {
  "use strict";

  function csvRecords(text) {
    var rows = [];
    var row = [];
    var field = "";
    var quoted = false;
    for (var i = 0; i < text.length; i++) {
      var c = text[i];
      if (quoted) {
        if (c === '"' && text[i + 1] === '"') {
          field += '"';
          i++;
        } else if (c === '"') {
          quoted = false;
        } else {
          field += c;
        }
      } else if (c === '"') {
        quoted = true;
      } else if (c === ",") {
        row.push(field);
        field = "";
      } else if (c === "\n" || c === "\r") {
        if (c === "\r" && text[i + 1] === "\n") i++;
        row.push(field);
        rows.push(row);
        row = [];
        field = "";
      } else {
        field += c;
      }
    }
    if (field !== "" || row.length > 0) {
      row.push(field);
      rows.push(row);
    }
    var header = rows[0];
    if (!header) return [];
    return rows.slice(1)
      .filter(function (cells) { return cells.length > 1 || cells[0] !== ""; })
      .map(function (cells) {
        var record = {};
        header.forEach(function (name, k) { record[name] = k < cells.length ? cells[k] : ""; });
        return record;
      });
  }

  root.jcCsv = csvRecords;
  if (typeof module === "object" && module.exports) module.exports = csvRecords;
})(typeof self !== "undefined" ? self : this);
