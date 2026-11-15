// The refusal list of ADR-N-053 §2.3 (EP-130), checked again in the browser: the host page checks a
// spec before it fetches a row, the sandboxed renderer before it draws. The publisher's check is
// not trusted alone. One function, no dependency, so node:test runs the same file (refuse.test.mjs).
(function (root) {
  "use strict";

  var MAX_BYTES = 200 * 1024;
  var SCHEMA = /^https:\/\/vega\.github\.io\/schema\/(vega-lite|vega)\/v5(\.\d+){0,2}\.json$/;
  // An image the spec names inline. SVG is left out: an SVG document can carry script.
  var DATA_IMAGE = /^data:image\/(png|jpeg|gif|webp);base64,[A-Za-z0-9+/=]+$/;
  // Words an expression has no business with. The interpreter refuses them too; this keeps a
  // spec that tries them from reaching it at all.
  var WORDS = /\b(__proto__|constructor|prototype)\b/;

  function onOurHosts(url, domain) {
    if (typeof url !== "string") return false;
    if (DATA_IMAGE.test(url)) return true;
    var parsed;
    try {
      parsed = new URL(url);
    } catch (e) {
      return false;
    }
    var host = parsed.hostname;
    // an App's origin `{name}.apps.{domain}` serves untrusted code (ADR-N-037): not ours
    if (host.slice(-(domain.length + 6)) === ".apps." + domain) return false;
    return parsed.protocol === "https:" && parsed.username === "" && parsed.password === "" &&
      (host === domain || host.slice(-(domain.length + 1)) === "." + domain);
  }

  // A member that can name a URL (`href`, `url`): a literal string on our hosts, or a channel
  // whose one constant `value` is. A field, a signal or an expression could name any host.
  function literalUrl(value, domain) {
    if (typeof value === "string") return onOurHosts(value, domain);
    if (value && typeof value === "object" && !Array.isArray(value)) {
      var keys = Object.keys(value);
      return keys.length === 1 && keys[0] === "value" && onOurHosts(value.value, domain);
    }
    return false;
  }

  // A `data` member says where rows come from: only the named source the renderer fills, inline
  // values, or (Vega) a data set derived from another by name. Never a URL.
  function dataAllowed(data) {
    var list = Array.isArray(data) ? data : [data];
    return list.every(function (entry) {
      if (!entry || typeof entry !== "object" || Array.isArray(entry)) return false;
      if ("url" in entry) return false;
      if (Array.isArray(data)) return typeof entry.name === "string"; // Vega: named entries
      return entry.name === "source" || "values" in entry || "sequence" in entry || "graticule" in entry || "sphere" in entry;
    });
  }

  function refuse(spec, domain) {
    var errors = [];
    function no(pointer, rule) {
      errors.push({ pointer: pointer || "/", rule: rule });
    }
    if (!spec || typeof spec !== "object" || Array.isArray(spec)) {
      no("", "a spec is a JSON object");
      return errors;
    }
    var text;
    try {
      text = JSON.stringify(spec);
    } catch (e) {
      no("", "a spec is plain JSON");
      return errors;
    }
    if (new TextEncoder().encode(text).length > MAX_BYTES) no("", "at most 200 KB");
    if (!SCHEMA.test(spec.$schema || "")) no("/$schema", "the Vega-Lite 5 or Vega 5 schema");

    function walk(node, pointer) {
      if (Array.isArray(node)) {
        node.forEach(function (item, index) {
          walk(item, pointer + "/" + index);
        });
        return;
      }
      if (typeof node === "string") {
        if (WORDS.test(node)) no(pointer, "no __proto__, constructor or prototype");
        if (/^\s*javascript:/i.test(node)) no(pointer, "no javascript: URL");
        return;
      }
      if (!node || typeof node !== "object") return;
      Object.keys(node).forEach(function (key) {
        var at = pointer + "/" + key.replace(/~/g, "~0").replace(/\//g, "~1");
        var value = node[key];
        if (WORDS.test(key)) {
          no(at, "no __proto__, constructor or prototype");
          return;
        }
        if (key === "usermeta") return no(at, "no usermeta");
        if (key === "loader") return no(at, "no loader options");
        if (key === "data" && !dataAllowed(value)) return no(at, "data only from the named source or inline values");
        if ((key === "url" || key === "href") && !literalUrl(value, domain)) {
          return no(at, "a " + key + " only as a literal on our hosts");
        }
        walk(value, at);
      });
    }
    walk(spec, "");
    return errors;
  }

  // The map spec of ADR-N-053 §2.9: GeoJSON and a field list, no expression and no style.
  // Every member is named here; anything else is refused.
  var MAP_SCHEMA = "urn:joinedcontext:map:v1";
  var MAP_FEATURES = 1000;
  var MAP_BYTES = 2 * 1024 * 1024;
  var MAP_MEMBERS = ["$schema", "title", "data", "label", "popup", "time", "link"];
  var GEOMETRIES = { Point: 0, MultiPoint: 1, LineString: 1, MultiLineString: 2, Polygon: 2, MultiPolygon: 3 };

  // A position is [longitude, latitude] within range; `depth` is how far the positions nest.
  function positions(value, depth) {
    if (!Array.isArray(value)) return false;
    if (depth === 0) {
      return value.length === 2 && typeof value[0] === "number" && typeof value[1] === "number" &&
        value[0] >= -180 && value[0] <= 180 && value[1] >= -90 && value[1] <= 90;
    }
    return value.length > 0 && value.every(function (item) { return positions(item, depth - 1); });
  }

  function plain(value) {
    return value === null || typeof value === "string" || typeof value === "boolean" ||
      (typeof value === "number" && isFinite(value));
  }

  function refuseMap(spec, domain) {
    var errors = [];
    function no(pointer, rule) {
      errors.push({ pointer: pointer, rule: rule });
    }
    if (!spec || typeof spec !== "object" || Array.isArray(spec)) return [{ pointer: "/", rule: "a spec is a JSON object" }];
    if (new TextEncoder().encode(JSON.stringify(spec)).length > MAP_BYTES) no("/", "at most 2 MB");
    if (spec.$schema !== MAP_SCHEMA) no("/$schema", "the map schema " + MAP_SCHEMA);
    Object.keys(spec).forEach(function (key) {
      if (MAP_MEMBERS.indexOf(key) < 0) no("/" + key, "no member of that name");
    });
    if (spec.title !== undefined && (typeof spec.title !== "string" || spec.title.length > 200)) no("/title", "a text of at most 200 characters");
    if (typeof spec.label !== "string") no("/label", "the name of the label property");
    if (spec.time !== undefined && typeof spec.time !== "string") no("/time", "the name of the time property");
    if (spec.link !== undefined && !onOurHosts(spec.link, domain)) no("/link", "a link only on our hosts");
    var fields = [spec.label, spec.time];
    var popup = Array.isArray(spec.popup) ? spec.popup : null;
    if (!popup || popup.length > 10) no("/popup", "a list of at most 10 fields");
    (popup || []).forEach(function (f, i) {
      var ok = f && typeof f === "object" && typeof f.field === "string" && typeof f.label === "string" &&
        (f.unit === undefined || f.unit === null || typeof f.unit === "string") &&
        Object.keys(f).every(function (k) { return k === "field" || k === "label" || k === "unit"; });
      if (!ok) no("/popup/" + i, "a field, its label and its unit, nothing else");
      else fields.push(f.field);
    });
    var data = spec.data;
    if (!data || data.type !== "FeatureCollection" || !Array.isArray(data.features) || Object.keys(data).length !== 2) {
      no("/data", "a GeoJSON FeatureCollection");
      return errors;
    }
    if (data.features.length > MAP_FEATURES) no("/data/features", "at most " + MAP_FEATURES + " features");
    data.features.slice(0, MAP_FEATURES).forEach(function (f, i) {
      var at = "/data/features/" + i;
      var g = f && f.geometry;
      if (!f || f.type !== "Feature" || !g || !(g.type in GEOMETRIES) || !positions(g.coordinates, GEOMETRIES[g.type])) {
        return no(at, "a Point, LineString, Polygon or Multi* feature in range");
      }
      var props = f.properties || {};
      Object.keys(props).forEach(function (k) {
        if (fields.indexOf(k) < 0 || !plain(props[k])) no(at + "/properties/" + k, "only the label, the popup fields and the time, as plain values");
      });
    });
    return errors;
  }

  root.jcRefuse = refuse;
  root.jcRefuseMap = refuseMap;
  if (typeof module === "object" && module.exports) {
    module.exports = refuse;
    module.exports.map = refuseMap;
  }
})(typeof self !== "undefined" ? self : this);
