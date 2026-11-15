// The live explorer on viz.{domain} (T-5059, T-5082): the catalogue's view of one public Endpoint.
// The table is the Portal's Endpoint data view, the same component (explorer/explorer.js, built
// from the Portal by scripts/viz-explorer-bundle.sh): filters, server-side order, paging, Fields,
// full window and CSV. This script holds the address (type, q and order in the hash, so a link
// opens the same view), the map of the same query and the query's API addresses. Everything is
// read from api.{domain} anonymously (no token, no cookie) and nothing is copied.
(function (root) {
  "use strict";

  var MAPLIBRE = "/vendor/maplibre-gl-6.14.0.mjs";
  var WORKER = "/vendor/maplibre-gl-worker-6.14.0.mjs";
  var MAP_DWELL = 500;
  var SLUG = /^[a-z0-9]{16,64}$/;
  var NAME = /^[A-Za-z][A-Za-z0-9_]{0,127}$/;
  var DATE = /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})?)?$/;
  var TIMEOUT_MS = 15000;
  var MAP_LIMIT = 1000;
  var MVT = "application/vnd.mapbox-vector-tile";
  var COLOUR = "#1f5fae";
  var TEXT = {
    en: {
      title: "Live data", loading: "Loading live data…", dataApi: "Data API", close: "Close", map: "Map", detail: "Details",
      failed: { 403: "This Endpoint is not public: the catalogue shows only what anyone may read.", 404: "The Endpoint was not found. It may have been unpublished.", 502: "The data service did not answer. Try again in a minute.", other: "The data could not be loaded (HTTP {status}). Try again in a minute." },
      timeout: "The data took longer than 15 seconds. Narrow the search or try again.", noAddress: "This page shows the live data of an open-data Endpoint; the address names none.",
      noWidget: "The table could not start. Reload the page; if it stays, the catalogue's administrators can see why in the browser console.",
      mapHint: "Click a point to open its details. The table lists the same entities.", mapCap: "The map shows the first {n} of {total} in view; zoom in for the rest.",
      mapNone: "No entity of this query has a location.", links: "This query elsewhere", entityLink: "This entity in NGSI-LD", queryLink: "This query in NGSI-LD",
      viewLink: "This view (type, filter and order)", yes: "yes", no: "no",
      cardYes: "Yes", cardNo: "No", dev: "For developers", onMap: "Show on the map", idLabel: "Identifier", typeLabel: "Type", placeMap: "Location map",
      embed: "Embed", embedLead: "Put this live table on your own website. It keeps reading the current data; nothing is copied.",
      embedKind: "Code", iframe: "Frame (recommended)", component: "Web component", kindHint: "A frame needs no script and works in a WordPress \u201cCustom HTML\u201d block; the web component grows to its content.",
      viewLabel: "Shows", viewBoth: "Table and map", viewTable: "Table only", viewHint: "The map appears only where the entities have a location.",
      langLabel: "Language", langHint: "The widget's labels, e.g. Slovenčina for a Slovak site.", themeLabel: "Colours", themeAuto: "The visitor's setting", themeLight: "Light", themeDark: "Dark", themeHint: "Light or dark, or as the visitor's device is set.",
      heightLabel: "Height in pixels", heightHint: "From 240 to 2000, e.g. 640.", keep: "Keep the current filter and order", keepHint: "Visitors start from this view: {view}", keepNone: "No filter or order is set: visitors see every entity.",
      filterIs: "filter {q}", orderIs: "ordered by {col}", desc: " (descending)",
      code: "Code to paste", copy: "Copy code", copied: "Copied. Paste it into your page's HTML.", copySelect: "Selected: press Ctrl+C to copy.", preview: "Open the widget alone",
      invalid: "Check this setting: {why}.", source: "Source", licence: "Licence", licenceSee: "see the dataset", catalogue: "open data catalogue",
      count: "On the map {n} of {total} · {none} without a location", countTiles: " · drawn as tiles", other: "Other",
      panelOpen: "Map settings", panel: "Map settings", geoLabel: "Location from", geoHint: "Which place of the entity the map draws, e.g. Location.",
      colorLabel: "Colour by", colorNone: "One colour", colorType: "Type", colorHint: "Points take a colour per value, e.g. Status; the legend is under the map.",
      popLabel: "In the bubble", popHint: "What a pointer over a point shows besides the title, e.g. Start and Web.",
      clusterLabel: "Group points", clusterHint: "Above 200 points in view, nearby points merge into a bubble with their count.",
      reset: "Reset to default", fallback: "The link names a map setting this type does not have ({what}); the map shows its default.", remove: "Remove {what}",
      chipGeo: "Location from: {v}", chipColor: "Colour by: {v}", chipPop: "In the bubble: {v}", chipCluster: "Group points: {v}", popNone: "title only", detailOpen: "Click for details"
    },
    sk: {
      title: "Živé dáta", loading: "Načítavam živé dáta…", dataApi: "Dátové API", close: "Zavrieť", map: "Mapa", detail: "Detail",
      failed: { 403: "Tento Endpoint nie je verejný: katalóg ukazuje len to, čo môže čítať ktokoľvek.", 404: "Endpoint sa nenašiel. Možno ho zrušili.", 502: "Dátová služba neodpovedala. Skúste to o minútu.", other: "Dáta sa nepodarilo načítať (HTTP {status}). Skúste to o minútu." },
      timeout: "Dáta sa načítavali dlhšie ako 15 sekúnd. Zúžte hľadanie alebo to skúste znova.", noAddress: "Táto stránka ukazuje živé dáta Endpointu otvorených dát; adresa žiadny neuvádza.",
      noWidget: "Tabuľka sa nespustila. Načítajte stránku znova; ak to pretrváva, správcovia katalógu vidia príčinu v konzole prehliadača.",
      mapHint: "Kliknite na bod a otvorí sa jeho detail. Tabuľka uvádza tie isté entity.", mapCap: "Mapa ukazuje prvých {n} z {total} vo výreze; priblížte pre ďalšie.",
      mapNone: "Žiadna entita tohto dopytu nemá polohu.", links: "Tento dopyt inde", entityLink: "Táto entita v NGSI-LD", queryLink: "Tento dopyt v NGSI-LD",
      viewLink: "Toto zobrazenie (typ, filter a poradie)", yes: "áno", no: "nie",
      cardYes: "Áno", cardNo: "Nie", dev: "Pre vývojárov", onMap: "Zobraziť na mape", idLabel: "Identifikátor", typeLabel: "Typ", placeMap: "Mapa miesta",
      embed: "Vložiť na web", embedLead: "Dajte túto živú tabuľku na svoj web. Stále číta aktuálne dáta; nič sa nekopíruje.",
      embedKind: "Kód", iframe: "Rámec (odporúčané)", component: "Webový komponent", kindHint: "Rámec nepotrebuje skript a funguje vo WordPress bloku \u201eVlastné HTML\u201c; webový komponent sa prispôsobí výške obsahu.",
      viewLabel: "Zobrazí", viewBoth: "Tabuľku a mapu", viewTable: "Len tabuľku", viewHint: "Mapa sa zobrazí, len ak entity majú polohu.",
      langLabel: "Jazyk", langHint: "Popisy widgetu, napr. Slovenčina pre slovenský web.", themeLabel: "Farby", themeAuto: "Podľa návštevníka", themeLight: "Svetlé", themeDark: "Tmavé", themeHint: "Svetlé alebo tmavé, alebo podľa nastavenia zariadenia návštevníka.",
      heightLabel: "Výška v pixeloch", heightHint: "Od 240 do 2000, napr. 640.", keep: "Zachovať aktuálny filter a poradie", keepHint: "Návštevníci začnú s týmto zobrazením: {view}", keepNone: "Nie je nastavený filter ani poradie: návštevníci uvidia všetky entity.",
      filterIs: "filter {q}", orderIs: "zoradené podľa {col}", desc: " (zostupne)",
      code: "Kód na vloženie", copy: "Kopírovať kód", copied: "Skopírované. Vložte ho do HTML svojej stránky.", copySelect: "Označené: skopírujte stlačením Ctrl+C.", preview: "Otvoriť samotný widget",
      invalid: "Skontrolujte toto nastavenie: {why}.", source: "Zdroj", licence: "Licencia", licenceSee: "pozri dataset", catalogue: "katalóg otvorených dát",
      count: "Na mape {n} z {total} · {none} bez polohy", countTiles: " · kreslené ako dlaždice", other: "Iné",
      panelOpen: "Nastaviť mapu", panel: "Nastavenie mapy", geoLabel: "Poloha z", geoHint: "Ktoré miesto entity mapa kreslí, napr. Poloha.",
      colorLabel: "Farba podľa", colorNone: "Jedna farba", colorType: "Typ", colorHint: "Body dostanú farbu podľa hodnoty, napr. Stav; legenda je pod mapou.",
      popLabel: "V bubline", popHint: "Čo ukáže kurzor nad bodom okrem názvu, napr. Začiatok a Web.",
      clusterLabel: "Zoskupovať body", clusterHint: "Nad 200 bodov vo výreze sa blízke body spoja do bubliny s ich počtom.",
      reset: "Obnoviť predvolené", fallback: "Odkaz uvádza nastavenie mapy, ktoré tento typ nemá ({what}); mapa ukazuje predvolené.", remove: "Odstrániť {what}",
      chipGeo: "Poloha z: {v}", chipColor: "Farba podľa: {v}", chipPop: "V bubline: {v}", chipCluster: "Zoskupovať body: {v}", popNone: "len názov", detailOpen: "Kliknutím otvoríte detail"
    }
  };

  var ORDER = /^(id|[A-Za-z][A-Za-z0-9_]{0,127})(;(asc|desc))?$/;
  var MAX_Q = 2000;

  // `#{slug}`, then `&lang=sk`, `&type=T`, `&inline`, `&all` (the reader removed the default filter, DM-135), `&theme=dark|light`, `&q=…` (the NGSI-LD
  // query, URI-encoded), `&sort=attr;desc`, the reader's map (T-5233: `&geo=slot`, `&color=none|type|slot`,
  // `&pop=a,b` or `&pop=-`, `&cluster=0|1`; their slots are checked against the model by mapChoice) and,
  // as embed.js writes them (T-5129), `&dataset=name` and `&view=table` in any order. The view checks the order against the type's columns again;
  // the Endpoint checks q.
  function parseHash(hash) {
    var parts = String(hash || "").replace(/^#/, "").split("&");
    if (!SLUG.test(parts[0])) return null;
    var out = { slug: parts[0], lang: null, type: null, inline: false, theme: null, q: null, sort: null, dataset: null, view: null, all: false, geo: null, color: null, pop: null, cluster: null };
    parts.slice(1).forEach(function (p) {
      var m = /^(lang|type|theme|q|sort|dataset|view|geo|color|pop|cluster)=(.+)$/.exec(p);
      var v = null;
      try { v = m && decodeURIComponent(m[2]); } catch (e) { v = null; }
      if (p === "inline") out.inline = true;
      else if (p === "all") out.all = true;
      else if (!m || v === null) return;
      else if (m[1] === "lang" && /^[a-z]{2}$/.test(v)) out.lang = v;
      else if (m[1] === "type" && NAME.test(v)) out.type = v;
      else if (m[1] === "theme" && /^(dark|light)$/.test(v)) out.theme = v;
      else if (m[1] === "q" && v.length <= MAX_Q) out.q = v;
      else if (m[1] === "sort" && ORDER.test(v)) out.sort = v;
      else if (m[1] === "dataset" && /^[a-z0-9_-]{2,100}$/.test(v)) out.dataset = v;
      else if (m[1] === "view" && v === "table") out.view = v;
      else if (m[1] === "geo" && NAME.test(v)) out.geo = v;
      else if (m[1] === "color" && NAME.test(v)) out.color = v;
      else if (m[1] === "pop" && v === "-") out.pop = [];
      else if (m[1] === "pop" && /^[A-Za-z][A-Za-z0-9_]{0,127}(,[A-Za-z][A-Za-z0-9_]{0,127}){0,19}$/.test(v)) out.pop = v.split(",");
      else if (m[1] === "cluster" && /^[01]$/.test(v)) out.cluster = v === "1";
    });
    return out;
  }

  // The hash for a view: what parseHash reads back.
  function hashOf(where) {
    var out = "#" + where.slug;
    if (where.inline) out += "&inline";
    if (where.all) out += "&all";
    ["dataset", "lang", "theme", "view", "type", "q", "sort", "geo", "color"].forEach(function (k) {
      if (where[k]) out += "&" + k + "=" + encodeURIComponent(where[k]);
    });
    if (Array.isArray(where.pop)) out += "&pop=" + (where.pop.length ? where.pop.map(encodeURIComponent).join(",") : "-");
    if (typeof where.cluster === "boolean") out += "&cluster=" + (where.cluster ? "1" : "0");
    return out;
  }

  // viz.{domain} → the Endpoint's base on api.{domain}: named by this page's host, never by a parameter.
  function base(hostname, slug) {
    return "https://api." + String(hostname).replace(/^viz\./, "") + "/api/endpoint/" + slug + "/";
  }

  function params(map) {
    return Object.keys(map)
      .filter(function (k) { return map[k] !== undefined && map[k] !== null && map[k] !== ""; })
      .map(function (k) { return encodeURIComponent(k) + "=" + encodeURIComponent(map[k]); })
      .join("&");
  }

  function entitiesUrl(api, query) {
    return api + "ngsi-ld/v1/entities?" + params(query);
  }

  // The map's view as an NGSI-LD `within` polygon, west/south/east/north clamped to the globe.
  function bboxQuery(b) {
    var c = function (v, lim) { return Math.max(-lim, Math.min(lim, Math.round(v * 1e6) / 1e6)); };
    var w = c(b[0], 180), s = c(b[1], 90), e = c(b[2], 180), n = c(b[3], 90);
    return { georel: "within", geometry: "Polygon", coordinates: JSON.stringify([[[w, s], [e, s], [e, n], [w, n], [w, s]]]) };
  }

  // The collection's tileset address under the Endpoint (T-5189, Antares antares-ogc tiles).
  function tilesetUrl(api, type) {
    return api + "ogc/features/collections/" + encodeURIComponent(type) + "/tiles/WebMercatorQuad";
  }

  // A tileset's MVT item template as MapLibre's {z}/{y}/{x}, or null when it is missing or leaves
  // the tileset's own path (another host, the broker's address, another collection, a query).
  function templateOf(tileset, at) {
    var item = ((tileset && tileset.links) || []).filter(function (l) { return l && l.rel === "item" && l.type === MVT && typeof l.href === "string"; })[0];
    if (!item) return null;
    var tail = "{z}/{y}/{x}";
    var href = item.href.replace("{tileMatrix}", "{z}").replace("{tileRow}", "{y}").replace("{tileCol}", "{x}");
    if (href.slice(-tail.length) !== tail || /[{}]/.test(href.slice(0, -tail.length))) return null;
    var url;
    try { url = new URL(href.slice(0, -tail.length), at); } catch (e) { return null; }
    var own = new URL(at);
    if (url.origin !== own.origin || url.pathname !== own.pathname + "/" || url.search || url.hash) return null;
    return url.href + tail;
  }

  // The same rows elsewhere: the entity, the query, OData, OGC API Features and CSV of its type.
  function links(api, type, q, order, id) {
    var out = [];
    if (id) out.push({ key: "entityLink", href: api + "ngsi-ld/v1/entities/" + encodeURIComponent(id) });
    out.push({ key: "queryLink", href: entitiesUrl(api, { type: type, q: q, orderBy: order }) });
    out.push({ key: "OData v4", href: api + "odata/v4/" + type });
    out.push({ key: "OGC API - Features", href: api + "ogc/features/collections/" + type + "/items" });
    out.push({ key: "CSV", href: api + "file.csv?" + params({ type: type, q: q, orderBy: order }) });
    out.push({ key: "OGC API - Tiles", href: tilesetUrl(api, type) });
    return out;
  }

  // A keyValues value as a person reads it: text, and a link when it is a web address. GeoJSON
  // becomes its kind and first position, a list its items, an object its members; never raw JSON.
  function show(value, t) {
    if (value === null || value === undefined || value === "") return { text: "–" };
    if (typeof value === "boolean") return { text: value ? t.yes : t.no };
    if (typeof value === "number") return { text: new Intl.NumberFormat(t.locale, { maximumFractionDigits: 6 }).format(value) };
    if (typeof value === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(value) && DATE.test(value) && t.locale) {
      return { text: new Intl.DateTimeFormat(t.locale, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" }).format(Date.parse(value)) + " UTC" };
    }
    if (typeof value === "string") return /^https?:\/\/[^\s]+$/.test(value) ? { text: value, href: value } : { text: value };
    if (Array.isArray(value)) return { text: value.map(function (v) { return show(v, t).text; }).join("; ") };
    if (typeof value.type === "string" && Array.isArray(value.coordinates)) {
      var p = value.coordinates;
      while (Array.isArray(p[0])) p = p[0];
      return { text: value.type + " " + p.slice(0, 2).map(function (n) { return Number(n).toFixed(5); }).join(", ") };
    }
    if (value.languageMap && typeof value.languageMap === "object") {
      var m = value.languageMap;
      return { text: String(m[t.lang] || m[Object.keys(m)[0]] || "–") };
    }
    if (value["@value"] !== undefined) return show(value["@value"], t);
    if (namedLink(value)) return { text: value.name.trim(), href: value.url.trim() };
    return { text: Object.keys(value).map(function (k) { return k + ": " + show(value[k], t).text; }).join(", ") };
  }

  function say(text, vars) {
    return String(text).replace(/\{(\w+)\}/g, function (_, k) { return vars && vars[k] !== undefined ? vars[k] : ""; });
  }

  // ---- the row card (DM-135, T-5231) ----

  // What a reader never needs in the card's body: the NGSI-LD members and the record's own stamps.
  var SYSTEM = { id: 1, type: 1, "@context": 1, dateCreated: 1, dateModified: 1 };

  // A key the model gives no label: dailyDepartures -> Daily departures.
  function humanise(key) {
    var words = String(key).replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").trim().toLowerCase();
    return words ? words.charAt(0).toUpperCase() + words.slice(1) : String(key);
  }

  function isGeo(value) {
    return !!value && typeof value === "object" && typeof value.type === "string" && Array.isArray(value.coordinates);
  }

  // A web address as its host (data.slovensko.sk), or null for anything but http(s).
  function hostOf(value) {
    if (typeof value !== "string" || !/^https?:\/\/[^\s]+$/i.test(value.trim())) return null;
    try { return new URL(value.trim()).hostname.replace(/^www\./, ""); } catch (e) { return null; }
  }

  // A `{name, url}` object (an Event's organizer, T-5134): read as its name, linked to its web site.
  function namedLink(value) {
    return typeof value.name === "string" && value.name.trim() !== "" && hostOf(value.url) !== null;
  }

  // One value as a reader reads it: local dates, numbers and units of the page's language, enum
  // titles, Áno/Nie, a web address by its host. An address object reads as its parts, never its keys.
  function cardValue(value, field, t) {
    field = field || {};
    if (value === null || value === undefined || value === "") return null;
    if (typeof value === "boolean") return { text: value ? t.cardYes : t.cardNo };
    if (typeof value === "number") {
      var n = new Intl.NumberFormat(t.locale, { maximumFractionDigits: 6 }).format(value);
      return { text: field.unit ? n + "\u00a0" + field.unit : n };
    }
    if (typeof value === "string") {
      if (field.titles && Object.prototype.hasOwnProperty.call(field.titles, value)) return { text: field.titles[value] };
      if (DATE.test(value) && !isNaN(Date.parse(value))) {
        var timed = value.indexOf("T") > 0;
        var o = timed ? { dateStyle: "medium", timeStyle: "short" } : { dateStyle: "medium", timeZone: "UTC" };
        if (timed && t.timeZone) o.timeZone = t.timeZone;
        return { text: new Intl.DateTimeFormat(t.locale, o).format(Date.parse(timed || value.length > 10 ? value : value + "T00:00:00Z")) };
      }
      var host = hostOf(value);
      return host ? { text: host, href: value.trim() } : { text: field.unit ? value + "\u00a0" + field.unit : value };
    }
    if (Array.isArray(value)) {
      var items = value.map(function (v) { return cardValue(v, field, t); }).filter(Boolean);
      return items.length ? { text: items.map(function (i) { return i.text; }).join(", "), href: items.length === 1 ? items[0].href : undefined } : null;
    }
    if (value.languageMap && typeof value.languageMap === "object") {
      var m = value.languageMap;
      return cardValue(m[t.lang] || m[Object.keys(m)[0]], field, t);
    }
    if (value["@value"] !== undefined) return cardValue(value["@value"], field, t);
    if (isGeo(value)) return null;
    if (namedLink(value)) return { text: value.name.trim(), href: value.url.trim() };
    var parts = Object.keys(value).map(function (k) { var v = cardValue(value[k], {}, t); return v && v.text; }).filter(Boolean);
    return parts.length ? { text: parts.join(", ") } : null;
  }

  // The card of one row (keyValues): the type's label and the title, the values in the model's
  // order under its labels, the first geometry for the small map, the source line, and what goes
  // under "For developers". `card` is the Portal view's RowCard of this type, or null.
  function cardOf(row, card, t) {
    var fields = {};
    ((card && card.fields) || []).forEach(function (f) { if (f && typeof f.name === "string") fields[f.name] = f; });
    var titleKey = (card && card.title) || "name";
    var title = cardValue(row[titleKey], fields[titleKey], t);
    var keys = ((card && card.fields) || []).map(function (f) { return f.name; }).filter(function (k) { return Object.prototype.hasOwnProperty.call(row, k); });
    Object.keys(row).forEach(function (k) { if (keys.indexOf(k) < 0) keys.push(k); });
    var out = {
      typeLabel: (card && card.type === row.type && card.typeLabel) || humanise(row.type || ""),
      title: (title && title.text) || String(row.id || ""),
      fields: [], geo: null, source: null,
      dev: [{ label: t.idLabel, text: String(row.id || "") }, { label: t.typeLabel, text: String(row.type || "") }]
    };
    var label = function (k) { return (fields[k] && fields[k].label) || humanise(k); };
    keys.forEach(function (k) {
      var v = row[k];
      if (k === "id" || k === "type" || k === "@context") return;
      if (isGeo(v)) { if (!out.geo) out.geo = v; return; }
      var said = cardValue(v, fields[k], t);
      if (!said) return;
      if (SYSTEM[k] || (fields[k] && fields[k].hidden)) return out.dev.push({ label: label(k), text: said.text });
      if (k === titleKey) return;
      if (k === "source" || k === "dataProvider") {
        out.source = out.source || {};
        if (k === "dataProvider") out.source.provider = said.text;
        else if (said.href) { out.source.href = said.href; out.source.host = said.text; }
        else out.source.text = said.text;
        return;
      }
      out.fields.push({ label: label(k), text: said.text, href: said.href });
    });
    return out;
  }

  // ---- the map's default mapping and the reader's choice (T-5233, DM-136, API/11 "Map fallback") ----

  var PALETTE = ["#1f5fae", "#c2410c", "#15803d", "#7e22ce", "#b91c1c", "#0e7490", "#a16207", "#be185d"];
  var GREY = "#6b7280";
  var CLUSTER_ABOVE = 200;
  var PLACE = ["address", "venue", "addressLocality", "place", "locationName"];

  // The GeoProperties of a type as the broker lists them (GET /types/{type}, attributeDetails), in the
  // mapping's order: the core location, then the model's order, then by name. A broker that lists only
  // attributeNames yields location when it has one.
  function geoProps(info, card) {
    var names = ((info && info.attributeDetails) || []).filter(function (a) {
      return a && NAME.test(a.attributeName || "") && Array.isArray(a.attributeTypes) && a.attributeTypes.indexOf("GeoProperty") >= 0;
    }).map(function (a) { return a.attributeName; });
    if (!names.length && info && (info.attributeNames || []).indexOf("location") >= 0) names = ["location"];
    var order = ((card && card.fields) || []).map(function (f) { return f.name; });
    var rank = function (n) { return n === "location" ? -1 : order.indexOf(n) < 0 ? order.length : order.indexOf(n); };
    return names.sort(function (a, b) { return rank(a) - rank(b) || (a < b ? -1 : a > b ? 1 : 0); });
  }

  // What a reader may pick on a type's map, from the model's slots alone, and the default the map
  // starts from: the publisher's (`card.map`, API/11 EP-163) for each member the model offers, else
  // the built-in one: the first geometry, a colour per type, the card's roles (time, place, link) in
  // the bubble, clusters above 200 in view.
  function mapOptions(geos, card) {
    var fields = ((card && card.fields) || []).filter(function (f) {
      return f && NAME.test(f.name || "") && !SYSTEM[f.name] && !f.hidden && geos.indexOf(f.name) < 0 && f.name !== (card.title || "name");
    });
    var named = function (test) { return fields.filter(test).map(function (f) { return f.name; })[0]; };
    var pop = [
      named(function (f) { return f.kind === "date-time" || f.kind === "date"; }),
      named(function (f) { return PLACE.indexOf(f.name) >= 0; }),
      named(function (f) { return f.kind === "uri" || f.name === "url"; })
    ].filter(Boolean);
    var o = {
      geos: geos,
      colors: ["none", "type"].concat(fields.filter(function (f) { return f.titles || f.kind === "boolean"; }).map(function (f) { return f.name; })),
      pops: fields.map(function (f) { return f.name; }),
      labels: ((card && card.fields) || []).reduce(function (m, f) { if (f && f.name) m[f.name] = f.label || humanise(f.name); return m; }, {}),
      defaults: { geo: geos[0] || null, color: "type", pop: pop, cluster: true }
    };
    var p = (card && card.map) || {}, d = o.defaults;
    if (typeof p.geometry === "string" && geos.indexOf(p.geometry) >= 0) d.geo = p.geometry;
    if (typeof p.colour === "string" && p.colour !== "none" && p.colour !== "type" && o.colors.indexOf(p.colour) >= 0) d.color = p.colour;
    if (Array.isArray(p.popup) && p.popup.length && p.popup.length <= 10 && p.popup.every(function (n) { return o.pops.indexOf(n) >= 0; })) d.pop = p.popup.slice();
    if (typeof p.cluster === "boolean") d.cluster = p.cluster;
    return o;
  }

  // The reader's choice from the address, each value kept only when the type's model offers it;
  // `unknown` names the settings that fell back to the default.
  function mapChoice(raw, o) {
    var d = o.defaults, out = { geo: d.geo, color: d.color, pop: d.pop.slice(), cluster: d.cluster }, unknown = [];
    raw = raw || {};
    if (raw.geo) { if (o.geos.indexOf(raw.geo) >= 0) out.geo = raw.geo; else unknown.push("geo"); }
    if (raw.color) { if (o.colors.indexOf(raw.color) >= 0) out.color = raw.color; else unknown.push("color"); }
    if (Array.isArray(raw.pop)) {
      if (raw.pop.every(function (n) { return o.pops.indexOf(n) >= 0; })) out.pop = raw.pop.slice(); else unknown.push("pop");
    }
    if (typeof raw.cluster === "boolean") out.cluster = raw.cluster;
    return { choice: out, unknown: unknown };
  }

  // A feature's colour class: its type, or the reader's field read as the card reads it; the first
  // eight values in view get the palette, the rest grey under "Other".
  function colourClasses(features, color, card, t) {
    var field = ((card && card.fields) || []).filter(function (f) { return f.name === color; })[0];
    var seen = [], legend = [];
    var classOf = function (props) {
      if (color === "none") return "";
      var v = color === "type" ? props.type : props[color];
      var said = color === "type" ? { text: (card && card.type === v && card.typeLabel) || humanise(v || "") } : cardValue(v, field, t);
      return said ? said.text : "–";
    };
    var classes = features.map(function (f) {
      var c = classOf(f.properties || {});
      if (seen.indexOf(c) < 0) seen.push(c);
      return c;
    });
    var colours = {};
    seen.forEach(function (c, i) { colours[c] = i < PALETTE.length ? PALETTE[i] : GREY; });
    seen.slice(0, PALETTE.length).forEach(function (c) { legend.push({ label: c, colour: colours[c] }); });
    if (seen.length > PALETTE.length) legend.push({ label: t.other, colour: GREY });
    return { colours: classes.map(function (c) { return colours[c]; }), legend: seen.length > 1 ? legend : [] };
  }

  // MultiPoints become one point each, so every point can join a cluster; the rest stay whole.
  function splitPoints(features) {
    var points = [], shapes = [];
    features.forEach(function (f) {
      var g = f.geometry;
      if (g.type === "Point") points.push(f);
      else if (g.type === "MultiPoint") g.coordinates.forEach(function (c) { points.push(Object.assign({}, f, { geometry: { type: "Point", coordinates: c } })); });
      else shapes.push(f);
    });
    return { points: points, shapes: shapes };
  }

  // "Na mape 412 z 1 680 · 96 bez polohy", numbers in the page's language.
  function countLine(t, c) {
    var n = function (v) { return new Intl.NumberFormat(t.locale).format(v); };
    return say(t.count, { n: n(c.inView), total: n(c.total), none: n(Math.max(0, c.none)) }) + (c.tiled ? t.countTiles : "");
  }

  // ---- the page ----

  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (k) {
      if (k === "text") node.textContent = attrs[k];
      else if (k === "class") node.className = attrs[k];
      else if (attrs[k] !== false && attrs[k] !== null && attrs[k] !== undefined) node.setAttribute(k, attrs[k] === true ? "" : attrs[k]);
    });
    (children || []).forEach(function (c) { if (c) node.appendChild(c); });
    return node;
  }

  // Text, or a link through the one safe path: http(s) only, a new tab, no opener and no referrer.
  function linkOrText(said) {
    var safe = said.href && hostOf(said.href) ? said.href : null;
    if (!safe) return document.createTextNode(said.text);
    return el("a", { href: safe, target: "_blank", rel: "noopener noreferrer", text: said.text, title: safe });
  }

  function main() {
    var where = parseHash(location.hash);
    var lang = (where && where.lang) || ((navigator.language || "en").slice(0, 2) === "sk" ? "sk" : "en");
    var t = TEXT[lang] || TEXT.en;
    t.lang = lang;
    t.locale = lang === "sk" ? "sk-SK" : "en-GB";
    document.documentElement.lang = lang;
    if (where && where.theme) document.documentElement.setAttribute("data-theme", where.theme);
    if (where && where.inline) document.body.classList.add("inline");
    var $ = function (id) { return document.getElementById(id); };
    // /embed.html is this page on another site (T-5129): it names its source and licence, and
    // offers no Embed button of its own.
    var embedded = /\/embed\.html$/.test(location.pathname);
    $("embed-open").hidden = embedded || !where;
    var status = $("status");
    Array.prototype.forEach.call(document.querySelectorAll("[data-t]"), function (n) { n.textContent = t[n.getAttribute("data-t")]; });
    Array.prototype.forEach.call(document.querySelectorAll("[data-t-label]"), function (n) { n.setAttribute("aria-label", t[n.getAttribute("data-t-label")]); });

    // The catalogue sizes the frame from this message. The content's own box, not the document's:
    // the view's stylesheet makes html as tall as the frame, which would hold the frame at its size.
    function height() {
      var h = Math.ceil(document.querySelector("main").getBoundingClientRect().bottom + window.scrollY);
      if (window.parent !== window) window.parent.postMessage({ type: "jc-viz-height", height: h }, "*");
    }
    if (typeof ResizeObserver === "function") new ResizeObserver(height).observe(document.querySelector("main"));
    // The slice of this frame the reader sees, from the page around it (T-5192): dialogs open there.
    window.addEventListener("message", function (event) {
      var d = event.data || {};
      if (event.source !== window.parent || window.parent === window || d.type !== "jc-viz-viewport") return;
      if (typeof d.top !== "number" || typeof d.height !== "number" || !isFinite(d.top) || !isFinite(d.height)) return;
      var top = Math.max(0, Math.min(d.top, document.documentElement.scrollHeight));
      var h = Math.max(120, Math.min(d.height, 4000));
      document.documentElement.style.setProperty("--jc-vv-top", Math.round(top) + "px");
      document.documentElement.style.setProperty("--jc-vv-height", Math.round(h) + "px");
    });

    function tell(text, kind) {
      status.textContent = text || "";
      status.className = "status" + (kind ? " " + kind : "");
      height();
    }

    if (!where) {
      $("app").hidden = true;
      return tell(t.noAddress, "error");
    }
    var api = base(location.hostname, where.slug);
    var state = { types: [], type: where.type, q: where.q || "", sort: where.sort, all: where.all, card: null, map: null, mapData: null, mapAsk: 0, tiled: null, tilesets: {},
      infos: {}, mapping: null, mapRaw: {}, countKey: null, total: null, none: null, inView: null };
    ["geo", "color", "pop", "cluster"].forEach(function (k) { if (where[k] !== null) state.mapRaw[k] = where[k]; });

    // Every request: anonymous, no referrer, given up after 15 seconds; a non-2xx is an Error with its status.
    function get(url, accept) {
      var controller = new AbortController();
      var timer = setTimeout(function () { controller.abort(); }, TIMEOUT_MS);
      return fetch(url, { credentials: "omit", referrerPolicy: "no-referrer", signal: controller.signal, headers: { Accept: accept || "application/json" } })
        .then(function (r) {
          if (!r.ok) {
            var e = new Error("HTTP " + r.status);
            e.status = r.status;
            throw e;
          }
          return r.json().then(function (body) { return { body: body, count: r.headers.get("NGSILD-Results-Count") }; });
        })
        .finally(function () { clearTimeout(timer); });
    }

    function failure(error) {
      if (error && error.name === "AbortError") return t.timeout;
      var s = error && error.status;
      return s ? t.failed[s] || say(t.failed.other, { status: s }) : say(t.failed.other, { status: "–" });
    }

    // One row as a reader's card (DM-135): the type's label above its title, the values under the
    // model's labels, a small map of its place, one source line, and the rest folded for developers.
    function detail(row, from) {
      var card = cardOf(row, state.card, t);
      $("detail-type").textContent = card.typeLabel;
      $("detail-type").hidden = !card.typeLabel;
      $("detail-title").textContent = card.title;
      var list = $("detail-list");
      list.textContent = "";
      card.fields.forEach(function (f) {
        list.appendChild(el("div", { class: "card-row" }, [el("dt", { text: f.label }), el("dd", {}, [linkOrText(f)])]));
      });
      var source = $("detail-source");
      source.textContent = "";
      source.hidden = !card.source;
      if (card.source) {
        source.appendChild(document.createTextNode(t.source + ": "));
        var bits = [card.source.provider, card.source.text].filter(Boolean);
        if (bits.length) source.appendChild(document.createTextNode(bits.join(" · ")));
        if (card.source.href) {
          if (bits.length) source.appendChild(document.createTextNode(" · "));
          source.appendChild(linkOrText({ text: card.source.host, href: card.source.href }));
        }
      }
      var dev = $("detail-dev-list");
      dev.textContent = "";
      card.dev.forEach(function (d) { dev.appendChild(el("div", { class: "card-row" }, [el("dt", { text: d.label }), el("dd", { text: d.text })])); });
      $("detail-dev").hidden = false;
      $("detail-dev").open = false;
      linkList(row.id);
      placeMap(card.geo);
      dialog(from);
    }

    // The small map of the card: the main map's style, one marker, no gestures; "Show on the map"
    // closes the card and brings the main map to the place.
    function placeMap(geo) {
      var box = $("detail-map");
      var go = $("detail-onmap");
      if (state.cardMap) { state.cardMap.remove(); state.cardMap = null; }
      var at = null;
      if (geo) { at = geo.coordinates; while (Array.isArray(at[0])) at = at[0]; }
      var ok = !!at && typeof at[0] === "number" && typeof at[1] === "number" && Math.abs(at[0]) <= 180 && Math.abs(at[1]) <= 90;
      box.hidden = !ok;
      go.hidden = !ok || $("map-section").hidden || !state.mapLoading;
      go.onclick = ok ? function () {
        $("detail").close();
        $("map-section").scrollIntoView({ block: "center" });
        // A map still waiting for the scroll opens at the place once it has drawn its first view.
        if (state.map) state.map.jumpTo({ center: [at[0], at[1]], zoom: Math.max(state.map.getZoom(), 15) });
        else state.mapAt = [at[0], at[1]];
      } : null;
      if (!ok) return;
      import(MAPLIBRE).then(function (maplibre) {
        if (box.hidden || !$("detail").open) return;
        maplibre.setWorkerUrl(WORKER);
        var domain = location.hostname.replace(/^viz\./, "");
        var m = new maplibre.Map({ container: box, style: "https://tiles." + domain + "/styles/light-" + (lang === "sk" ? "sk" : "en") + ".json", center: [at[0], at[1]], zoom: 15, interactive: false, attributionControl: false });
        m.addControl(new maplibre.AttributionControl({ compact: true }));
        new maplibre.Marker({ color: COLOUR }).setLngLat([at[0], at[1]]).addTo(m);
        state.cardMap = m;
      }, function () { box.hidden = true; });
    }

    // Open the card in the middle of what the reader sees (explore.css) and give focus back on close.
    function dialog(from) {
      $("detail").returnFocus = from;
      if (!$("detail").open) $("detail").showModal();
      $("detail").scrollTop = 0;
    }

    // A row the table opened: read it as keyValues in the page's language, as a tile click does.
    function openRow(id) {
      var from = document.activeElement;
      get(api + "ngsi-ld/v1/entities/" + encodeURIComponent(id) + "?" + params({ format: "keyValues", lang: lang })).then(function (answer) {
        if (answer.body && answer.body.id === id) detail(answer.body, from);
      }, function (err) { tell(failure(err), "error"); });
    }

    function linkList(id) {
      var out = $("detail-links");
      out.textContent = "";
      links(api, state.type, state.q, state.sort, id).concat(id ? [] : [{ key: "viewLink", href: location.href }]).forEach(function (l) {
        out.appendChild(el("li", {}, [el("a", { href: l.href, target: "_blank", rel: "noopener noreferrer", text: t[l.key] || l.key })]));
      });
    }

    // The type's own description (GET /types/{type}): which of its attributes are GeoProperties.
    function typeInfo(type) {
      return state.infos[type] || (state.infos[type] = get(api + "ngsi-ld/v1/types/" + encodeURIComponent(type)).then(function (a) { return a.body; }, function () { return null; }));
    }

    // The map of the same query within the view, drawn by the default mapping or the reader's choice
    // (T-5233): the map shows only when the type has a GeoProperty.
    function drawMap() {
      var section = $("map-section");
      var type = state.type;
      var listed = state.types.filter(function (x) { return x.typeName === type; })[0];
      if (where.view === "table" || !listed) { section.hidden = true; return; }
      typeInfo(type).then(function (info) {
        if (state.type !== type) return;
        var card = state.card && state.card.type === type ? state.card : null;
        var geos = geoProps(info && info.attributeDetails ? info : listed, card);
        section.hidden = !geos.length;
        if (!geos.length) return;
        var o = mapOptions(geos, card);
        var picked = mapChoice(state.mapRaw, o);
        var moved = state.mapping && state.mapping.choice.geo !== picked.choice.geo;
        state.mapping = { options: o, choice: picked.choice, card: card };
        // Only with the model in hand is a slot the address names known to be wrong: drop it, say so.
        if (card && picked.unknown.length) {
          picked.unknown.forEach(function (k) { delete state.mapRaw[k]; });
          $("map-fallback").textContent = say(t.fallback, { what: picked.unknown.map(function (k) { return t[k + "Label"]; }).join(", ") });
          $("map-fallback").hidden = false;
          address();
        }
        panel();
        // Another geometry: the map opens where it is, as the first view did.
        if (state.map && moved && state.map.getSource("jc")) return fitFirst().then(mapMoved);
        if (state.map) return mapMoved();
        startMap();
      });
    }

    function startMap() {
      // One map: a second call while MapLibre still loads waits for the first (T-5192 drew two).
      if (state.mapLoading) return;
      state.mapLoading = true;
      // MapLibre and its tiles cost seconds of a phone's main thread (T-5794: 6.5 s of a dataset
      // page's blocking time): they load once the map has stayed in view for MAP_DWELL ms. Inside a
      // frame the observer judges by the reader's window, so a map below the page's fold waits for
      // the scroll; the dwell lets the table's first page push the map down before it counts.
      if (typeof IntersectionObserver !== "function") return loadMap();
      var timer = null;
      var seen = new IntersectionObserver(function (entries) {
        var inView = entries[entries.length - 1].isIntersecting;
        clearTimeout(timer);
        if (inView) timer = setTimeout(function () { seen.disconnect(); loadMap(); }, MAP_DWELL);
      });
      seen.observe($("map"));
    }

    function loadMap() {
      import(MAPLIBRE).then(function (maplibre) {
        maplibre.setWorkerUrl(WORKER);
        if (root.pmtiles) maplibre.addProtocol("pmtiles", new root.pmtiles.Protocol().tile);
        // The Endpoint's vector tiles, read like every other request here: anonymous, no referrer.
        maplibre.addProtocol("jc-tile", function (request, controller) {
          return fetch(request.url.replace(/^jc-tile:/, "https:"), { credentials: "omit", referrerPolicy: "no-referrer", signal: controller.signal, headers: { Accept: MVT } })
            .then(function (r) {
              if (r.status === 204) return { data: new ArrayBuffer(0) };
              if (!r.ok) throw new Error("HTTP " + r.status);
              return r.arrayBuffer().then(function (data) { return { data: data }; });
            });
        });
        var domain = location.hostname.replace(/^viz\./, "");
        var map = new maplibre.Map({ container: "map", style: "https://tiles." + domain + "/styles/light-" + (lang === "sk" ? "sk" : "en") + ".json", attributionControl: false, center: [17.1, 48.15], zoom: 4 });
        state.map = map;
        root.jcExploreMap = map; // for the browser journey
        state.maplibre = maplibre;
        map.addControl(new maplibre.NavigationControl({ showCompass: false }));
        map.addControl(new maplibre.AttributionControl({ compact: true }));
        map.once("load", function () {
          // Lines and areas in "jc", points in "jc-pts", which clusters above CLUSTER_ABOVE in view.
          var empty = { type: "FeatureCollection", features: [] };
          var colour = ["coalesce", ["get", "jcColour"], COLOUR];
          map.addSource("jc", { type: "geojson", data: empty });
          map.addSource("jc-pts", { type: "geojson", data: empty, cluster: false, clusterRadius: 50, clusterMaxZoom: 14 });
          map.addLayer({ id: "jc-fill", type: "fill", source: "jc", filter: ["in", ["geometry-type"], ["literal", ["Polygon", "MultiPolygon"]]], paint: { "fill-color": colour, "fill-opacity": 0.25 } });
          map.addLayer({ id: "jc-line", type: "line", source: "jc", paint: { "line-color": colour, "line-width": 2 } });
          map.addLayer({ id: "jc-cluster", type: "circle", source: "jc-pts", filter: ["has", "point_count"], paint: { "circle-color": COLOUR, "circle-opacity": 0.85, "circle-radius": ["step", ["get", "point_count"], 14, 50, 18, 200, 24], "circle-stroke-color": "#fff", "circle-stroke-width": 2 } });
          // The count on a cluster needs the style's glyphs; the basemap styles have them.
          if (map.getStyle().glyphs) map.addLayer({ id: "jc-cluster-count", type: "symbol", source: "jc-pts", filter: ["has", "point_count"], layout: { "text-field": ["get", "point_count_abbreviated"], "text-size": 12 }, paint: { "text-color": "#fff" } });
          map.addLayer({ id: "jc-point", type: "circle", source: "jc-pts", filter: ["!", ["has", "point_count"]], paint: { "circle-color": colour, "circle-radius": 6, "circle-stroke-color": "#fff", "circle-stroke-width": 2 } });
          ["jc-point", "jc-line", "jc-fill"].forEach(function (layer) {
            map.on("click", layer, function (e) {
              var f = (state.mapData || []).filter(function (x) { return x.id === e.features[0].properties.jcId; })[0];
              if (f) detail(Object.assign({ id: f.id, location: f.geometry }, f.properties), map.getCanvas());
            });
            map.on("mouseenter", layer, function (e) { map.getCanvas().style.cursor = "pointer"; bubble(e); });
            map.on("mouseleave", layer, function () { map.getCanvas().style.cursor = ""; if (state.popup) state.popup.remove(); });
          });
          map.on("click", "jc-cluster", function (e) {
            var c = e.features[0];
            Promise.resolve(map.getSource("jc-pts").getClusterExpansionZoom(c.properties.cluster_id)).then(function (z) {
              map.easeTo({ center: c.geometry.coordinates, zoom: z });
            }, function () {});
          });
          map.on("mouseenter", "jc-cluster", function () { map.getCanvas().style.cursor = "pointer"; });
          map.on("mouseleave", "jc-cluster", function () { map.getCanvas().style.cursor = ""; });
          TILE_LAYERS.forEach(function (layer) {
            map.on("click", layer, function (e) {
              var id = e.features[0].properties.id;
              if (typeof id !== "string") return;
              get(api + "ngsi-ld/v1/entities/" + encodeURIComponent(id) + "?" + params({ format: "keyValues", lang: lang })).then(function (answer) {
                if (answer.body && answer.body.id === id) detail(answer.body, map.getCanvas());
              }, function (err) { $("map-note").textContent = failure(err); });
            });
            map.on("mouseenter", layer, function () { map.getCanvas().style.cursor = "pointer"; });
            map.on("mouseleave", layer, function () { map.getCanvas().style.cursor = ""; });
          });
          map.on("moveend", mapMoved);
          fitFirst().then(function () {
            if (state.mapAt) map.jumpTo({ center: state.mapAt, zoom: 15 });
            state.mapAt = null;
          }).then(mapMoved);
        });
      }, function () { $("map-note").textContent = failure(null); });
    }

    // The first view: the bounds of the query's first page, so the map opens where the data is.
    function fitFirst() {
      var url = entitiesUrl(api, Object.assign({ type: state.type, q: state.q, limit: 25, lang: lang, format: "keyValues" }, ownGeo()));
      return get(url, "application/geo+json").then(function (answer) {
        var all = [];
        ((answer.body && answer.body.features) || []).forEach(function (f) {
          var walk = function (c) { if (typeof c[0] === "number") all.push(c); else c.forEach(walk); };
          if (f.geometry && f.geometry.coordinates) walk(f.geometry.coordinates);
        });
        if (!all.length) return;
        var lon = all.map(function (p) { return p[0]; }), lat = all.map(function (p) { return p[1]; });
        var box = [[Math.min.apply(null, lon), Math.min.apply(null, lat)], [Math.max.apply(null, lon), Math.max.apply(null, lat)]];
        if (box[0][0] === box[1][0] && box[0][1] === box[1][1]) state.map.jumpTo({ center: box[0], zoom: 14 });
        else state.map.fitBounds(box, { padding: 30, maxZoom: 15, animate: false });
      }, function () {});
    }

    // Past MAP_LIMIT and with no filter (a tile takes no query), the map draws the type's vector
    // tiles: the Endpoint cuts every view to its tiles, so 100 000 entities pan like 100.
    var TILE_LAYERS = ["jc-tile-fill", "jc-tile-line", "jc-tile-point"];
    function untile() {
      var map = state.map;
      TILE_LAYERS.forEach(function (l) { if (map.getLayer(l)) map.removeLayer(l); });
      if (map.getSource("jc-tiles")) map.removeSource("jc-tiles");
      state.tiled = null;
    }
    function tile(type) {
      var at = tilesetUrl(api, type);
      var known = state.tilesets[type] || (state.tilesets[type] = get(at).then(function (a) { return templateOf(a.body, at); }, function () { return null; }));
      return known.then(function (template) {
        var map = state.map;
        if (!template || state.type !== type || state.q || geoOf() !== "location" || state.tiled === type) return !!template && state.tiled === type;
        untile();
        var is = function (types) { return ["in", ["geometry-type"], ["literal", types]]; };
        map.addSource("jc-tiles", { type: "vector", tiles: [template.replace(/^https:/, "jc-tile:")], minzoom: 0, maxzoom: 16 });
        var of = function (layer) { return Object.assign(layer, { source: "jc-tiles", "source-layer": type }); };
        map.addLayer(of({ id: "jc-tile-fill", type: "fill", filter: is(["Polygon", "MultiPolygon"]), paint: { "fill-color": COLOUR, "fill-opacity": 0.25 } }));
        map.addLayer(of({ id: "jc-tile-line", type: "line", filter: ["!", is(["Point", "MultiPoint"])], paint: { "line-color": COLOUR, "line-width": 2 } }));
        map.addLayer(of({ id: "jc-tile-point", type: "circle", filter: is(["Point", "MultiPoint"]), paint: { "circle-color": COLOUR, "circle-radius": 5, "circle-stroke-color": "#fff", "circle-stroke-width": 1.5 } }));
        map.getSource("jc").setData({ type: "FeatureCollection", features: [] });
        map.getSource("jc-pts").setData({ type: "FeatureCollection", features: [] });
        legend([]);
        state.mapData = [];
        state.tiled = type;
        showCount();
        return true;
      });
    }

    // The geometry the map draws; a GeoProperty other than location is named to the broker (6.3.15).
    function geoOf() { return state.mapping ? state.mapping.choice.geo : "location"; }
    function ownGeo() { var g = geoOf(); return g === "location" ? {} : { geoproperty: g, geometryProperty: g }; }

    // The count line's totals for the query: every row, and the rows without the drawn geometry.
    function counts() {
      var geo = geoOf(), key = [state.type, state.q, geo].join("\u0000");
      if (state.countKey === key) return;
      state.countKey = key;
      state.total = state.none = null;
      var ask = function (q) { return get(entitiesUrl(api, { type: state.type, q: q, limit: 0, count: "true" })).then(function (a) { return a.count === null ? null : +a.count; }); };
      Promise.all([ask(state.q), ask((state.q ? "(" + state.q + ");" : "") + "!" + geo)]).then(function (c) {
        if (state.countKey !== key) return;
        state.total = c[0];
        state.none = c[1];
        showCount();
      }, function () {});
    }
    function showCount() {
      var ok = [state.inView, state.total, state.none].every(function (v) { return typeof v === "number" && isFinite(v); });
      $("map-count").hidden = !ok;
      if (ok) $("map-count").textContent = countLine(t, { inView: state.inView, total: state.total, none: state.none, tiled: !!state.tiled });
    }

    // The features of the view, coloured by the reader's field, points clustered when they are many.
    function paint(features) {
      var m = state.mapping, map = state.map;
      var c = colourClasses(features, m.choice.color, m.card, t);
      var parts = splitPoints(features.map(function (f, i) { return { type: "Feature", geometry: f.geometry, properties: { jcId: f.id, jcColour: c.colours[i] } }; }));
      var fc = function (list) { return { type: "FeatureCollection", features: list }; };
      map.getSource("jc").setData(fc(parts.shapes));
      var pts = map.getSource("jc-pts");
      if (pts.setClusterOptions) pts.setClusterOptions({ cluster: m.choice.cluster && parts.points.length > CLUSTER_ABOVE, clusterRadius: 50, clusterMaxZoom: 14 });
      pts.setData(fc(parts.points));
      legend(c.legend);
    }
    function legend(items) {
      var list = $("map-legend");
      list.textContent = "";
      items.forEach(function (i) {
        var swatch = el("span", { class: "swatch", "aria-hidden": "true" });
        swatch.style.backgroundColor = i.colour; // CSSOM, which the page's CSP allows where a style attribute is not
        list.appendChild(el("li", {}, [swatch, document.createTextNode(i.label)]));
      });
      list.hidden = !items.length;
    }

    // The bubble over a feature: its title and the reader's fields, read as the card reads them.
    function bubble(e) {
      var m = state.mapping;
      var f = m && (state.mapData || []).filter(function (x) { return x.id === e.features[0].properties.jcId; })[0];
      if (!f) return;
      var props = f.properties || {}, fields = {};
      ((m.card && m.card.fields) || []).forEach(function (x) { fields[x.name] = x; });
      var titleKey = (m.card && m.card.title) || "name";
      var title = cardValue(props[titleKey], fields[titleKey], t);
      var box = el("div", { class: "map-bubble" }, [el("strong", { text: (title && title.text) || String(f.id) })]);
      m.choice.pop.forEach(function (k) {
        var v = cardValue(props[k], fields[k], t);
        if (v) box.appendChild(el("div", {}, [el("span", { class: "muted", text: (m.options.labels[k] || humanise(k)) + ": " }), document.createTextNode(v.text)]));
      });
      box.appendChild(el("div", { class: "muted", text: t.detailOpen }));
      state.popup = state.popup || new state.maplibre.Popup({ closeButton: false, closeOnClick: false, offset: 10, maxWidth: "280px" });
      state.popup.setLngLat(e.lngLat).setDOMContent(box).addTo(state.map);
    }

    function mapMoved() {
      var map = state.map;
      if (!map || !map.getSource("jc") || !state.mapping) return;
      if (state.tiled && (state.tiled !== state.type || state.q || geoOf() !== "location")) untile();
      counts();
      var b = map.getBounds();
      var box = Object.assign(bboxQuery([b.getWest(), b.getSouth(), b.getEast(), b.getNorth()]), ownGeo());
      var asked = ++state.mapAsk;
      var shown = function (n) { if (asked === state.mapAsk) { state.inView = n; showCount(); } };
      // Tiles draw the view themselves; the count in view still comes from the query.
      if (state.tiled) {
        get(entitiesUrl(api, Object.assign({ type: state.type, limit: 0, count: "true" }, box))).then(function (a) { if (a.count !== null) shown(+a.count); }, function () {});
        return;
      }
      var query = Object.assign({ type: state.type, q: state.q, limit: MAP_LIMIT, count: "true", lang: lang, format: "keyValues" }, box);
      get(entitiesUrl(api, query), "application/geo+json").then(function (answer) {
        if (asked !== state.mapAsk) return;
        var features = ((answer.body && answer.body.features) || []).filter(function (f) { return f && f.geometry && typeof f.geometry.type === "string"; });
        state.mapData = features;
        paint(features);
        var total = answer.count === null ? features.length : +answer.count;
        shown(total);
        $("map-note").textContent = !features.length ? t.mapNone : total > features.length ? say(t.mapCap, { n: features.length, total: total }) : t.mapHint;
        if (total > features.length && !state.q) {
          tile(state.type).then(function (on) { if (on && asked === state.mapAsk) $("map-note").textContent = t.mapHint; });
        }
      }, function (e) { if (asked === state.mapAsk) $("map-note").textContent = failure(e); });
    }
    state.mapAsk = 0;

    // The address of the view: the table's type, query and order, and the reader's map choices.
    function address() {
      var r = state.mapRaw;
      history.replaceState(null, "", hashOf({ slug: where.slug, inline: where.inline, all: state.all, lang: where.lang, theme: where.theme, dataset: where.dataset, view: where.view,
        type: state.type, q: state.q, sort: state.sort, geo: r.geo, color: r.color, pop: r.pop, cluster: r.cluster }));
    }

    // The Mapa panel (T-5233): the reader's geometry, colour, bubble fields and clustering, each a
    // chip with × beside the map and in the address; reset brings back the default.
    function panel() {
      var m = state.mapping, o = m.options, c = m.choice, r = state.mapRaw;
      var name = function (k) { return o.labels[k] || humanise(k); };
      var colourName = function (v) { return v === "none" ? t.colorNone : v === "type" ? t.colorType : name(v); };
      var fill = function (select, values, label, value) {
        select.textContent = "";
        values.forEach(function (v) { select.appendChild(el("option", { value: v, text: label(v) })); });
        select.value = value;
      };
      fill($("map-geo"), o.geos, name, c.geo);
      fill($("map-color"), o.colors, colourName, c.color);
      var pops = $("map-pop-list");
      pops.textContent = "";
      o.pops.forEach(function (k) {
        pops.appendChild(el("label", { class: "check" }, [el("input", { type: "checkbox", value: k, checked: c.pop.indexOf(k) >= 0, "aria-describedby": "map-pop-hint" }), document.createTextNode(" " + name(k))]));
      });
      $("map-cluster").checked = c.cluster;
      var chips = $("map-chips");
      chips.textContent = "";
      var said = {
        geo: function () { return say(t.chipGeo, { v: name(c.geo) }); },
        color: function () { return say(t.chipColor, { v: colourName(c.color) }); },
        pop: function () { return say(t.chipPop, { v: c.pop.length ? c.pop.map(name).join(", ") : t.popNone }); },
        cluster: function () { return say(t.chipCluster, { v: c.cluster ? t.cardYes : t.cardNo }); }
      };
      ["geo", "color", "pop", "cluster"].filter(function (k) { return r[k] !== undefined && r[k] !== null; }).forEach(function (k) {
        var text = said[k]();
        var chip = el("button", { type: "button", class: "chip", "aria-label": say(t.remove, { what: text }) }, [document.createTextNode(text + " "), el("span", { "aria-hidden": "true", text: "×" })]);
        chip.addEventListener("click", function () { delete state.mapRaw[k]; changed(); $("map-panel-open").focus(); });
        chips.appendChild(chip);
      });
      $("map-reset").hidden = !chips.childNodes.length;
    }

    // A control moved: keep what differs from the default, write the address, redraw.
    function changed() {
      address();
      drawMap();
    }
    function fromPanel() {
      var d = state.mapping.options.defaults, r = {};
      var pop = Array.prototype.filter.call($("map-pop-list").querySelectorAll("input"), function (i) { return i.checked; }).map(function (i) { return i.value; });
      if ($("map-geo").value !== d.geo) r.geo = $("map-geo").value;
      if ($("map-color").value !== d.color) r.color = $("map-color").value;
      if (pop.join(",") !== d.pop.join(",")) r.pop = pop;
      if ($("map-cluster").checked !== d.cluster) r.cluster = $("map-cluster").checked;
      state.mapRaw = r;
      $("map-fallback").hidden = true;
      changed();
    }
    $("map-panel").addEventListener("change", fromPanel);
    $("map-panel-open").addEventListener("click", function () {
      var open = $("map-panel").hidden;
      $("map-panel").hidden = !open;
      $("map-panel-open").setAttribute("aria-expanded", String(open));
      height();
    });
    $("map-reset").addEventListener("click", function () {
      state.mapRaw = {};
      $("map-fallback").hidden = true;
      changed();
      $("map-panel-open").focus();
    });

    // The view says which type, query and order it shows: the address, the map and the API links follow.
    function onLink(next) {
      var moved = next.type !== state.type || (next.q || "") !== state.q;
      // A map choice names the slots of one type: another type starts from its own default.
      if (state.type && next.type !== state.type) state.mapRaw = {};
      state.type = next.type || null;
      state.q = next.q || "";
      state.sort = next.sort || null;
      state.all = next.noDefault === true;
      address();
      if (moved) drawMap();
    }

    // The query's API addresses: the same dialog, developers' content open.
    $("data-api").addEventListener("click", function () {
      $("detail-type").hidden = true;
      $("detail-title").textContent = t.dataApi;
      $("detail-list").textContent = "";
      $("detail-list").appendChild(el("div", { class: "card-row" }, [el("dt", { text: "q" }), el("dd", { text: state.q || "–" })]));
      $("detail-source").hidden = true;
      $("detail-dev-list").textContent = "";
      $("detail-dev").open = true;
      linkList(null);
      placeMap(null);
      dialog($("data-api"));
    });
    // The source line of an embed: the dataset's title, publisher and licence from the catalogue,
    // shown only when that dataset names this very Endpoint; else the catalogue itself.
    function attribute() {
      var catalogue = "https://opendata." + location.hostname.replace(/^viz\./, "");
      var line = $("attribution");
      var draw = function (title, href, publisher, licence, licenceHref) {
        line.textContent = "";
        line.appendChild(document.createTextNode(t.source + ": "));
        line.appendChild(el("a", { href: href, target: "_blank", rel: "noopener noreferrer", text: title }));
        if (publisher) line.appendChild(document.createTextNode(" · " + publisher));
        line.appendChild(document.createTextNode(" · " + t.licence + ": "));
        line.appendChild(licenceHref ? el("a", { href: licenceHref, target: "_blank", rel: "noopener noreferrer", text: licence }) : document.createTextNode(licence));
        line.hidden = false;
        height();
      };
      draw(catalogue.replace("https://", "") + " – " + t.catalogue, catalogue + "/", null, t.licenceSee, null);
      if (!where.dataset) return;
      get(catalogue + "/api/3/action/package_show?id=" + encodeURIComponent(where.dataset)).then(function (answer) {
        var pkg = answer.body && answer.body.success && answer.body.result;
        var extra = ((pkg && pkg.extras) || []).filter(function (x) { return x && x.key === "endpoint"; })[0];
        if (!pkg || !extra || String(extra.value).replace(/\/?$/, "/") !== api) return;
        var href = catalogue + "/dataset/" + where.dataset;
        var licenceHref = /^https?:\/\//.test(pkg.license_url || "") ? pkg.license_url : null;
        draw(String(pkg.title || where.dataset), href, pkg.organization && pkg.organization.title ? String(pkg.organization.title) : null,
          pkg.license_title ? String(pkg.license_title) : t.licenceSee, licenceHref || (pkg.license_title ? null : href));
      }, function () {});
    }

    // The Embed dialog (T-5129): the code for this Endpoint, type and, if kept, the view's filter and order.
    function embedCode() {
      var E = root.jcEmbed;
      var keep = $("embed-keep").checked;
      var o = { slug: where.slug, dataset: where.dataset, type: state.type, lang: $("embed-lang").value, view: $("embed-view").value,
        theme: $("embed-theme").value || null, height: $("embed-height").value, q: keep ? state.q : "", sort: keep ? state.sort : null };
      var words = [];
      if (state.q) words.push(say(t.filterIs, { q: state.q }));
      if (state.sort) words.push(say(t.orderIs, { col: state.sort.split(";")[0] }) + (/;desc$/.test(state.sort) ? t.desc : ""));
      $("embed-keep-hint").textContent = words.length ? say(t.keepHint, { view: words.join("; ") }) : t.keepNone;
      $("embed-copied").textContent = "";
      try {
        $("embed-code").value = E.snippet($("embed-form").querySelector("input[name=kind]:checked").value, o, location.origin, t.title + (state.type ? ": " + state.type : ""));
        $("embed-preview").href = location.origin + "/embed.html" + E.hash(o);
        $("embed-error").hidden = true;
        $("embed-height").removeAttribute("aria-invalid");
        $("embed-copy").disabled = false;
      } catch (e) {
        $("embed-error").textContent = say(t.invalid, { why: e.message });
        $("embed-error").hidden = false;
        if (/^height/.test(e.message)) $("embed-height").setAttribute("aria-invalid", "true");
        $("embed-code").value = "";
        $("embed-copy").disabled = true;
      }
    }

    $("embed-open").addEventListener("click", function () {
      $("embed-lang").value = lang;
      embedCode();
      $("embed").showModal();
    });
    $("embed-form").addEventListener("input", embedCode);
    $("embed-form").addEventListener("change", embedCode);
    $("embed-form").addEventListener("submit", function (e) { e.preventDefault(); });
    $("embed-copy").addEventListener("click", function () {
      var code = $("embed-code");
      var said = function (text) { $("embed-copied").textContent = text; };
      var select = function () { code.focus(); code.select(); said(t.copySelect); };
      if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(code.value).then(function () { said(t.copied); }, select);
      else select();
    });
    $("embed-close").addEventListener("click", function () { $("embed").close(); });
    $("embed").addEventListener("close", function () { $("embed-open").focus(); });
    if (embedded) attribute();
    $("detail-close").addEventListener("click", function () { $("detail").close(); });
    // A click on the backdrop (the dialog box itself, outside its content) closes it, as Esc does.
    // Tab cycles inside: in a frame the browser's own modal trap lets focus out to the host page.
    ["detail", "embed"].forEach(function (id) {
      $(id).addEventListener("keydown", function (event) {
        if (event.key !== "Tab") return;
        var all = Array.prototype.filter.call($(id).querySelectorAll("a[href], button, input, select, textarea, summary, [tabindex]:not([tabindex='-1'])"), function (n) {
          // A closed fold keeps boxes for its content in Chromium, yet nothing in it takes focus but its summary.
          var fold = n.closest("details:not([open])");
          return !n.disabled && n.getClientRects().length > 0 && (!fold || n.parentElement === fold && n.tagName === "SUMMARY");
        });
        if (!all.length) return;
        var first = all[0], last = all[all.length - 1];
        if (event.shiftKey && (document.activeElement === first || !$(id).contains(document.activeElement))) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && (document.activeElement === last || !$(id).contains(document.activeElement))) { event.preventDefault(); first.focus(); }
      });
      $(id).addEventListener("click", function (event) {
        if (event.target !== $(id)) return;
        var r = $(id).getBoundingClientRect();
        if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) $(id).close();
      });
    });
    $("detail").addEventListener("close", function () { var f = $("detail").returnFocus; if (f && f.focus) f.focus(); });
    window.addEventListener("resize", height);

    // The type list only says which types have a location, for the map; the view reads its own.
    get(api + "ngsi-ld/v1/types?details=true").then(function (answer) {
      state.types = (Array.isArray(answer.body) ? answer.body : []).filter(function (x) { return x && NAME.test(x.typeName || ""); });
      drawMap();
    }, function () {});

    if (!root.jcExplorer || typeof root.jcExplorer.mount !== "function") return tell(t.noWidget, "error");
    try {
      root.jcExplorer.mount($("grid"), {
        origin: "https://api." + String(location.hostname).replace(/^viz\./, ""),
        slug: where.slug,
        lang: lang,
        link: { type: where.type || undefined, q: where.q || undefined, sort: where.sort || undefined, noDefault: where.all || undefined },
        onLink: onLink,
        onCard: function (card) {
          state.card = card;
          if (card && card.type === state.type) drawMap();
        },
        onOpenRow: openRow,
        csv: true
      });
    } catch (e) {
      tell(t.noWidget, "error");
    }
    height();
  }

  root.jcExplore = { parseHash: parseHash, hashOf: hashOf, base: base, entitiesUrl: entitiesUrl, bboxQuery: bboxQuery, links: links, templateOf: templateOf, tilesetUrl: tilesetUrl, geoProps: geoProps, mapOptions: mapOptions, mapChoice: mapChoice, colourClasses: colourClasses, splitPoints: splitPoints, countLine: countLine, show: show, cardValue: cardValue, namedLink: namedLink, cardOf: cardOf, hostOf: hostOf, humanise: humanise, TEXT: TEXT };
  if (typeof module === "object" && module.exports) module.exports = root.jcExplore;
  else if (typeof document !== "undefined") main();
})(typeof self !== "undefined" ? self : this);
