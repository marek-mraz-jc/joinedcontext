// The map page on viz.{domain} (ADR-N-053 §2.9, T-4657). It draws one map spec, GeoJSON and a
// field list with no expression and no style, with MapLibre on our own basemap (tiles.{domain},
// T-4638). It takes the spec from its parent on this host alone, checks it again, and sets every
// value as text. Its CSP lets it connect to the tiles and nothing else.
(function (root) {
  "use strict";

  var MAPLIBRE = "/vendor/maplibre-gl-6.14.0.mjs";
  var WORKER = "/vendor/maplibre-gl-worker-6.14.0.mjs";
  var STYLES = ["sk", "cs", "en", "fi"];
  var COLOUR = "#1f5fae";
  var TEXT = {
    sk: { detail: "Detail", hint: "Vyberte bod alebo stlačte Enter.", yes: "áno", no: "nie", shown: "Zobrazené: {n}, spolu {total}", unlocated: "{none} bez polohy" },
    cs: { detail: "Detail", hint: "Vyberte bod nebo stiskněte Enter.", yes: "ano", no: "ne", shown: "Zobrazeno: {n}, celkem {total}", unlocated: "{none} bez polohy" },
    de: { detail: "Details", hint: "Punkt auswählen oder Enter drücken.", yes: "ja", no: "nein", shown: "{n} von {total} angezeigt", unlocated: "{none} ohne Standort" },
    fi: { detail: "Tiedot", hint: "Valitse kohde tai paina Enter.", yes: "kyllä", no: "ei", shown: "Näytetään {n} / {total}", unlocated: "{none} ilman sijaintia" },
    en: { detail: "Details", hint: "Select a point or press Enter.", yes: "yes", no: "no", shown: "{n} shown of {total}", unlocated: "{none} without a location" }
  };

  // Every position of a geometry, however deep it nests.
  function flat(coordinates) {
    if (typeof coordinates[0] === "number") return [coordinates];
    return coordinates.reduce(function (all, c) { return all.concat(flat(c)); }, []);
  }

  // [[west, south], [east, north]] of all features, or null for none.
  // One pass, no copies: a map of 20,000 points (T-7755) concatenated them in quadratic time.
  function bounds(features) {
    var box = null;
    features.forEach(function (f) {
      flat(f.geometry.coordinates).forEach(function (p) {
        if (!box) box = [[p[0], p[1]], [p[0], p[1]]];
        box[0][0] = Math.min(box[0][0], p[0]);
        box[0][1] = Math.min(box[0][1], p[1]);
        box[1][0] = Math.max(box[1][0], p[0]);
        box[1][1] = Math.max(box[1][1], p[1]);
      });
    });
    return box;
  }

  // The points a map draws as bubbles with their count once there are more than CLUSTER_FROM of
  // them (T-7755): MapLibre's own clustering, each point carrying its feature's index.
  var CLUSTER_FROM = 200;
  function clustered(features) {
    var points = features.filter(function (f) { return f.geometry.type === "Point"; });
    if (points.length <= CLUSTER_FROM) return null;
    return points.map(function (f) {
      return { type: "Feature", geometry: f.geometry, properties: { jcIndex: f.id } };
    });
  }

  // The feature whose first position lies closest to [lon, lat]: what Enter opens.
  function nearest(features, at) {
    var best = -1;
    var gap = Infinity;
    features.forEach(function (f, i) {
      var p = flat(f.geometry.coordinates)[0];
      var d = Math.pow(p[0] - at[0], 2) + Math.pow(p[1] - at[1], 2);
      if (d < gap) {
        gap = d;
        best = i;
      }
    });
    return best;
  }

  // A point on every line and polygon, with the feature's id: a bridge of 30 m is a dot at any
  // zoom instead of a stroke too short to see or click (T-4727). Its middle position, so it sits on
  // the line itself.
  function anchors(features) {
    return features.filter(function (f) { return !/Point$/.test(f.geometry.type); }).map(function (f) {
      var all = flat(f.geometry.coordinates);
      return { type: "Feature", id: f.id, geometry: { type: "Point", coordinates: all[Math.floor(all.length / 2)] }, properties: f.properties };
    });
  }

  // What a feature's popup says, as plain strings: the label, each field with its unit, the time
  // and the link. The DOM gets these through textContent alone.
  function lines(spec, props, lang) {
    var t = TEXT[lang] || TEXT.en;
    var numbers = new Intl.NumberFormat(lang, { maximumFractionDigits: 3 });
    var text = function (v, field) {
      if (v === null || v === undefined || v === "") return "–";
      // A year reads as a year, never "1 910" (T-7755).
      if (typeof v === "number" && /year|rok/i.test(field || "") && Number.isInteger(v) && v > 999 && v < 10000) return String(v);
      if (typeof v === "number") return numbers.format(v);
      if (typeof v === "boolean") return v ? t.yes : t.no;
      return String(v);
    };
    var when = props[spec.time];
    var stamp = typeof when === "string" ? Date.parse(when) : NaN;
    return {
      title: text(props[spec.label]),
      rows: spec.popup.map(function (f) {
        var v = text(props[f.field], f.field);
        return [f.label, f.unit && v !== "–" ? v + " " + f.unit : v];
      }),
      time: isNaN(stamp) ? null : new Intl.DateTimeFormat(lang, { dateStyle: "medium", timeStyle: "short" }).format(stamp),
      link: typeof spec.link === "string" ? { href: spec.link, text: t.detail } : null
    };
  }

  // What the map says on itself (T-7504): how many it draws of all, and how many of all have no
  // location. "" for a map that was not counted.
  function countText(spec, lang) {
    if (!spec.count) return "";
    var t = TEXT[lang] || TEXT.en;
    var numbers = new Intl.NumberFormat(lang);
    var said = t.shown.replace("{n}", numbers.format(spec.data.features.length)).replace("{total}", numbers.format(spec.count.total));
    if (!spec.count.unlocated) return said;
    return said + " · " + t.unlocated.replace("{none}", numbers.format(spec.count.unlocated));
  }

  function node(tag, className, text) {
    var el = document.createElement(tag);
    if (className) el.className = className;
    if (text !== undefined) el.textContent = text;
    return el;
  }

  function popupNode(said) {
    var box = node("div", "jc-pop");
    box.appendChild(node("strong", "", said.title));
    var list = node("dl");
    said.rows.forEach(function (row) {
      list.appendChild(node("dt", "", row[0]));
      list.appendChild(node("dd", "", row[1]));
    });
    box.appendChild(list);
    if (said.time) box.appendChild(node("p", "", said.time));
    if (said.link) {
      var a = node("a", "", said.link.text);
      a.href = said.link.href; // on our hosts only: refuse.js checked it
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      box.appendChild(a);
    }
    return box;
  }

  function tell(message) {
    window.parent.postMessage(message, location.origin);
  }

  function height() {
    tell({ type: "jc-viz-height", height: Math.ceil(document.body.getBoundingClientRect().height) });
  }

  async function draw(spec, lang) {
    var domain = location.hostname.replace(/^(?:embed|viz)\./, "");
    var errors = root.jcRefuseMap(spec, domain);
    if (errors.length) throw new Error("The map was refused: " + errors[0].pointer + " — " + errors[0].rule + ".");
    var count = document.getElementById("count");
    count.textContent = countText(spec, lang);
    count.hidden = !count.textContent;
    var maplibre = await import(MAPLIBRE);
    maplibre.setWorkerUrl(WORKER);
    maplibre.addProtocol("pmtiles", new root.pmtiles.Protocol().tile);
    var features = spec.data.features.map(function (f, i) {
      return { type: "Feature", id: i, geometry: f.geometry, properties: f.properties || {} };
    });
    var area = document.getElementById("map");
    var style = "https://tiles." + domain + "/styles/light-" + (STYLES.indexOf(lang) >= 0 ? lang : "en") + ".json";
    var map = new maplibre.Map({ container: area, style: style, attributionControl: false, center: [0, 0], zoom: 1 });
    map.addControl(new maplibre.NavigationControl({ showCompass: false }));
    map.addControl(new maplibre.AttributionControl({ compact: true }));
    document.getElementById("hint").textContent = (TEXT[lang] || TEXT.en).hint;
    var box = bounds(features);
    if (box && box[0][0] === box[1][0] && box[0][1] === box[1][1]) map.jumpTo({ center: box[0], zoom: 14 });
    else if (box) map.fitBounds(box, { padding: count.hidden ? 40 : { top: 56, right: 48, bottom: area.clientWidth <= 600 ? 84 : 56, left: 40 }, maxZoom: 15, animate: false });
    await new Promise(function (resolve) { map.once("load", resolve); });
    map.addSource("jc", { type: "geojson", data: { type: "FeatureCollection", features: features } });
    var is = function (types) { return ["in", ["geometry-type"], ["literal", types]]; };
    map.addLayer({ id: "jc-fill", type: "fill", source: "jc", filter: is(["Polygon", "MultiPolygon"]), paint: { "fill-color": COLOUR, "fill-opacity": 0.25 } });
    map.addLayer({ id: "jc-line", type: "line", source: "jc", filter: is(["LineString", "MultiLineString", "Polygon", "MultiPolygon"]), paint: { "line-color": COLOUR, "line-width": 3 } });
    map.addSource("jc-anchor", { type: "geojson", data: { type: "FeatureCollection", features: anchors(features) } });
    map.addLayer({ id: "jc-anchor", type: "circle", source: "jc-anchor", paint: { "circle-color": COLOUR, "circle-radius": 5, "circle-stroke-color": "#fff", "circle-stroke-width": 2 } });
    var dot = { "circle-color": COLOUR, "circle-radius": 7, "circle-stroke-color": "#fff", "circle-stroke-width": 2 };
    var many = clustered(features);
    if (many) {
      // Points cluster, MultiPoints stay as they were; a click on a bubble zooms into it.
      map.addLayer({ id: "jc-point", type: "circle", source: "jc", filter: is(["MultiPoint"]), paint: dot });
      map.addSource("jc-pts", { type: "geojson", data: { type: "FeatureCollection", features: many }, cluster: true, clusterRadius: 44, clusterMaxZoom: 14 });
      var bubble = { "circle-color": COLOUR, "circle-opacity": 0.85, "circle-radius": ["step", ["get", "point_count"], 13, 10, 16, 50, 20, 100, 24, 500, 30], "circle-stroke-color": "#fff", "circle-stroke-width": 2 };
      map.addLayer({ id: "jc-cluster", type: "circle", source: "jc-pts", filter: ["has", "point_count"], paint: bubble });
      // The count needs the style's glyphs; the basemap styles have them.
      if (map.getStyle().glyphs) map.addLayer({ id: "jc-cluster-count", type: "symbol", source: "jc-pts", filter: ["has", "point_count"], layout: { "text-field": ["get", "point_count_abbreviated"], "text-size": 12, "text-allow-overlap": true }, paint: { "text-color": "#fff" } });
      map.addLayer({ id: "jc-single", type: "circle", source: "jc-pts", filter: ["!", ["has", "point_count"]], paint: dot });
      map.on("click", "jc-cluster", function (e) {
        var hit = e.features[0];
        map.getSource("jc-pts").getClusterExpansionZoom(hit.properties.cluster_id).then(function (zoom) {
          map.easeTo({ center: hit.geometry.coordinates, zoom: zoom });
        });
      });
      map.on("mouseenter", "jc-cluster", function () { map.getCanvas().style.cursor = "pointer"; });
      map.on("mouseleave", "jc-cluster", function () { map.getCanvas().style.cursor = ""; });
    } else {
      map.addLayer({ id: "jc-point", type: "circle", source: "jc", filter: is(["Point", "MultiPoint"]), paint: dot });
    }
    var layers = (many ? ["jc-single"] : []).concat(["jc-point", "jc-anchor", "jc-line", "jc-fill"]);
    var open = function (i, at) {
      new maplibre.Popup({ maxWidth: "280px" }).setLngLat(at).setDOMContent(popupNode(lines(spec, features[i].properties, lang))).addTo(map);
    };
    layers.forEach(function (layer) {
      map.on("click", layer, function (e) {
        var f = e.features[0];
        open(layer === "jc-single" ? f.properties.jcIndex : f.id, e.lngLat);
      });
      map.on("mouseenter", layer, function () { map.getCanvas().style.cursor = "pointer"; });
      map.on("mouseleave", layer, function () { map.getCanvas().style.cursor = ""; });
    });
    // Enter opens the feature nearest the middle of the view; the arrows and +/- are MapLibre's.
    map.getCanvas().addEventListener("keydown", function (e) {
      if (e.key !== "Enter" || !features.length) return;
      var c = map.getCenter();
      var i = nearest(features, [c.lng, c.lat]);
      open(i, flat(features[i].geometry.coordinates)[0]);
    });
    // A few answers on one map are told apart by name; textContent alone, as in the popup.
    if (features.length > 1 && features.length <= 3) {
      features.forEach(function (f) {
        // A line's or an area's name stands at the dot that marks it (T-4811).
        var dot = anchors([f])[0] || f;
        var at = flat(dot.geometry.coordinates)[0];
        var east = at[0] > (box[0][0] + box[1][0]) / 2;
        var name = node("span", "jc-name", lines(spec, f.properties, lang).title);
        new maplibre.Marker({ element: name, anchor: east ? "right" : "left", offset: [east ? -12 : 12, 0] }).setLngLat(at).addTo(map);
      });
    }
    root.jcMap = map; // for the browser journey: what the layers hold
    root.jcMapFeatures = features.length;
    root.jcMapReady = true; // for the browser journey
  }

  if (typeof module === "object" && module.exports) {
    module.exports = { anchors: anchors, bounds: bounds, clustered: clustered, nearest: nearest, lines: lines, countText: countText };
    return;
  }
  window.addEventListener("message", function (event) {
    if (event.source !== window.parent || event.origin !== location.origin) return;
    var message = event.data || {};
    if (message.type !== "jc-viz-render") return;
    var lang = /^[a-z]{2}$/.test(message.lang) ? message.lang : "en";
    document.documentElement.lang = lang;
    document.documentElement.dataset.theme = message.theme === "dark" ? "dark" : "light";
    draw(message.spec, lang).then(height, function (error) {
      tell({ type: "jc-viz-error", message: String((error && error.message) || error) });
    });
  });
  tell({ type: "jc-viz-ready" });
})(typeof self !== "undefined" ? self : this);
