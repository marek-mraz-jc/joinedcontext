// The host page on viz.{domain} (ADR-N-053 §2.5, T-4641). It is our code and runs no spec: it
// reads the published CKAN view named in the fragment, checks the spec, fetches the declared
// source of that Endpoint without a token and within the limits of §2.6, and hands spec and rows
// to the renderer frame, which is sandboxed to an opaque origin and may fetch nothing at all.
(function (root) {
  "use strict";

  var LIMIT_ROWS = 50000;
  var LIMIT_BYTES = 20 * 1024 * 1024;
  var TIMEOUT_MS = 15000;
  var PAGE = 1000;
  // T-6170: reads one page runs at once, across its chart frames. The edge holds a client to 10
  // bulk and history reads at once; a dataset page with six charts asked for six and lost one.
  var READS = 4;
  var MAX_DAYS = 31;
  var SLUG = /^[a-z0-9]{16,64}$/;
  var NAME = /^[A-Za-z][A-Za-z0-9_-]{0,127}$/;
  var LINK = {
    sk: "Dáta a tabuľka v katalógu otvorených dát",
    cs: "Data a tabulka v katalogu otevřených dat",
    fi: "Aineisto ja taulukko avoimen datan luettelossa",
    en: "Data and table in the open-data catalogue"
  };
  // What a reader is told when the Endpoint refuses a read, in the page's language (T-5421).
  var UNREADABLE = {
    sk: "Údaje tohto grafu teraz nie sú dostupné.",
    cs: "Data tohoto grafu nyní nejsou dostupná.",
    fi: "Tämän kaavion tiedot eivät ole nyt saatavilla.",
    en: "The data of this chart is not available now."
  };
  // T-5414: a temporal chart cut at a limit says so instead of passing for the whole window.
  var PARTIAL = {
    sk: "Graf je neúplný: zobrazuje údaje do {at}, zvyšok presahuje limity stránky.",
    cs: "Graf je neúplný: ukazuje data do {at}, zbytek přesahuje limity stránky.",
    fi: "Kaavio on vajaa: se näyttää tiedot {at} asti, loput ylittävät sivun rajat.",
    en: "Partial chart: it shows data up to {at}; the rest is over the page's limits."
  };

  // `#{viewId}`, then `&inline` (the catalogue shows title and description itself) and
  // `&lang=sk` (the page's language) in any order.
  function parseHash(hash) {
    var parts = String(hash || "").replace(/^#/, "").split("&");
    if (!/^[0-9a-f-]{36}$/.test(parts[0])) return null;
    var lang = parts.map(function (p) { return /^lang=([a-z]{2})$/.exec(p); }).filter(Boolean)[0];
    return { id: parts[0], inline: parts.indexOf("inline") > 0, lang: lang ? lang[1] : null };
  }

  // viz.{domain} → {domain}: the hosts the page reads are named by its own, never by a parameter.
  function hosts(hostname) {
    var domain = String(hostname).replace(/^viz\./, "");
    return { domain: domain, api: "https://api." + domain, data: "https://opendata." + domain };
  }

  // Whether a parent may hand this page a spec (`#show`, ADR-N-053 §2.9): https on the domain or
  // one of its subdomains, no port and no user, so only a page on our own hosts makes us draw. An
  // App's origin `{name}.apps.{domain}` runs untrusted code (ADR-N-037) and is not ours.
  function ourOrigin(origin, domain) {
    var m = /^https:\/\/([a-z0-9.-]+)$/.exec(String(origin || ""));
    if (!m || m[1].slice(-(domain.length + 6)) === ".apps." + domain) return false;
    return m[1] === domain || m[1].slice(-(domain.length + 1)) === "." + domain;
  }

  // PT24H, P7D, P1DT12H: the window of a temporal source, in milliseconds; null when it is not one
  // or exceeds 31 days.
  function duration(text) {
    var m = /^P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?)?$/.exec(String(text || ""));
    if (!m || text === "P" || /T$/.test(text)) return null;
    var ms = ((+m[1] || 0) * 86400 + (+m[2] || 0) * 3600 + (+m[3] || 0) * 60) * 1000;
    return ms > 0 && ms <= MAX_DAYS * 86400000 ? ms : null;
  }

  function query(params) {
    return Object.keys(params)
      .filter(function (k) { return params[k] !== undefined && params[k] !== null && params[k] !== ""; })
      .map(function (k) { return encodeURIComponent(k) + "=" + encodeURIComponent(params[k]); })
      .join("&");
  }

  // The first request of a source: its URL under the Endpoint and how its answer is read. Throws
  // on anything the docs do not name (API/04 §3a), so a view never becomes a URL of its own.
  function firstRequest(source, endpoint, api, now) {
    if (!SLUG.test(endpoint || "")) throw new Error("the view names no Endpoint");
    var base = api + "/api/endpoint/" + endpoint + "/";
    var s = source || {};
    switch (s.source) {
      case "csv":
        if (!NAME.test(s.type || "")) throw new Error("csv needs a type");
        return { kind: "csv", url: base + "file.csv?" + query({ type: s.type }) };
      case "ogc":
        if (!NAME.test(s.collection || "")) throw new Error("ogc needs a collection");
        return {
          kind: "ogc",
          url: base + "ogc/features/collections/" + s.collection + "/items?" +
            query({ limit: PAGE, filter: s.filter, properties: s.properties })
        };
      case "odata":
        if (!NAME.test(s.entitySet || "")) throw new Error("odata needs an entitySet");
        return {
          kind: "odata",
          type: s.entitySet,
          url: base + "odata/v4/" + s.entitySet + "?" +
            query({ $filter: s.$filter, $select: s.$select, $apply: s.$apply, $orderby: s.$orderby })
        };
      case "temporal": {
        var attrs = Array.isArray(s.attrs) ? s.attrs : [];
        if (!NAME.test(s.type || "") || !attrs.length || !attrs.every(function (a) { return NAME.test(a); })) {
          throw new Error("temporal needs a type and attrs");
        }
        var window = duration(s.last);
        if (!window) throw new Error("temporal needs last, at most P31D");
        var aggregated = !!s.aggrMethod;
        if (aggregated && (!/^(avg|min|max|sum|totalCount|distinctCount|stddev|sumsq)$/.test(s.aggrMethod) || !duration(s.aggrPeriod))) {
          throw new Error("temporal aggrMethod needs an aggrPeriod");
        }
        return {
          kind: "temporal",
          type: s.type,
          attrs: attrs,
          method: aggregated ? s.aggrMethod : null,
          url: base + "ngsi-ld/v1/temporal/entities?" + query({
            type: s.type,
            attrs: attrs.join(","),
            timerel: "after",
            timeAt: new Date(now - window).toISOString().replace(/\.\d{3}Z$/, "Z"),
            format: aggregated ? "aggregatedValues" : "temporalValues",
            aggrMethods: aggregated ? s.aggrMethod : undefined,
            aggrPeriodDuration: aggregated ? s.aggrPeriod : undefined,
            // instances per attribute: the broker serves 100 unless asked, so a week of
            // five-minute readings took 21 windows one after another (T-5777); it caps N at
            // its own ceiling, and an aggregation counts buckets, not instances
            firstN: aggregated ? undefined : PAGE,
            limit: PAGE
          })
        };
      }
      default:
        throw new Error("unknown source");
    }
  }

  // A LanguageProperty read with `lang`, or its map: the page's language, else the first text.
  function text(value, lang) {
    if (value && typeof value === "object" && !Array.isArray(value)) {
      var map = value.languageMap || value;
      return map[lang] || map[Object.keys(map)[0]] || null;
    }
    return value === undefined ? null : value;
  }

  function rowsFromOgc(body) {
    return (body.features || []).map(function (f) {
      var row = { id: f.id, geometry: f.geometry };
      Object.keys(f.properties || {}).forEach(function (k) { row[k] = f.properties[k]; });
      return row;
    });
  }

  // The instances of an entity's attribute in a temporal answer: one object, or an array of
  // them when the attribute has several, each with its own datasetId (T-5432).
  function parts(entity, attr) {
    var p = entity && entity[attr];
    return (Array.isArray(p) ? p : [p]).filter(function (i) { return i && typeof i === "object"; });
  }

  // One row per instant, entity, attribute and instance: `observedAt` (the start of the period
  // when aggregated) and the attribute's value under its name. An instance with a datasetId is a
  // series of its own: its row carries the datasetId and the name starts with its last segment,
  // which a legend cut at 160 px still shows.
  function rowsFromTemporal(body, request, names) {
    var rows = [];
    (Array.isArray(body) ? body : []).forEach(function (entity) {
      request.attrs.forEach(function (attr) {
        parts(entity, attr).forEach(function (p) {
          var pairs = request.method ? p[request.method] : p.values;
          var dataset = typeof p.datasetId === "string" ? p.datasetId : null;
          var name = names[entity.id] || entity.id;
          (Array.isArray(pairs) ? pairs : []).forEach(function (pair) {
            var row = { id: entity.id, name: dataset ? dataset.split(":").pop() + " · " + name : name, observedAt: pair[1] };
            if (dataset) row.datasetId = dataset;
            row[attr] = pair[0];
            rows.push(row);
          });
        });
      });
    });
    return rows;
  }

  // Rows of any source with an `id` and no name of their own get the entity's name in the page's
  // language (OData leaves a LanguageProperty empty).
  function named(rows, names, lang) {
    return rows.map(function (row) {
      var name = text(row.name, lang);
      if ((name === null || name === "") && row.id && names[row.id]) name = names[row.id];
      if (name !== null || "name" in row) row.name = name;
      return row;
    });
  }

  // An error of the page's own limits: a temporal chart past its first page stops there and
  // draws what it has, with a note.
  function limit(message) {
    var error = new Error(message);
    error.limit = true;
    return error;
  }

  // One of the page's READS slots for `run` (a function returning a promise), held until it
  // settles. Every chart frame of a page shares an origin, so Web Locks count them together;
  // without them (an old browser, node) the slots are this frame's alone.
  var held = 0;
  var queued = [];
  function slot(run) {
    var locks = root.navigator && root.navigator.locks;
    if (locks && typeof locks.request === "function") {
      var free = function (i) {
        // Every slot taken: wait in line for one of them.
        if (i === READS) return locks.request("jc-viz-read-" + Math.floor(Math.random() * READS), run);
        return locks.request("jc-viz-read-" + i, { ifAvailable: true }, function (lock) {
          return lock ? run() : free(i + 1);
        });
      };
      return free(0);
    }
    return new Promise(function (resolve) {
      if (held < READS) {
        held += 1;
        resolve();
      } else queued.push(resolve);
    }).then(run).finally(function () {
      var next = queued.shift();
      if (next) next();
      else held -= 1;
    });
  }

  // T-6170: one more try after a 429 or a network error (a refusal without CORS reads as one),
  // after Retry-After capped at 2 s, or 1 s, plus up to 0.5 s so the charts do not come back
  // together. A second refusal is the reader's to see.
  function retried(fetcher, url, options) {
    var again = function (seconds) {
      var wait = Math.min(seconds > 0 ? seconds : 1, 2) * 1000 + Math.random() * 500;
      return new Promise(function (resolve) { setTimeout(resolve, wait); }).then(function () {
        return fetcher(url, options);
      });
    };
    return fetcher(url, options).then(function (r) {
      return r.status === 429 ? again(+r.headers.get("Retry-After")) : r;
    }, function (error) {
      if (error && error.name === "AbortError") throw error;
      return again(0);
    });
  }

  // A JSON or text answer, stopped once the budget's bytes are spent.
  function read(response, budget) {
    if (!response.ok) {
      var refused = new Error("the Endpoint answered " + response.status);
      refused.status = response.status;
      throw refused;
    }
    var length = +response.headers.get("Content-Length") || 0;
    if (length > budget.bytes) throw limit("over 20 MB: aggregate on the server");
    var reader = response.body.getReader();
    var chunks = [];
    function pump() {
      return reader.read().then(function (step) {
        if (step.done) return new TextDecoder().decode(concat(chunks));
        budget.bytes -= step.value.length;
        if (budget.bytes < 0) {
          reader.cancel();
          throw limit("over 20 MB: aggregate on the server");
        }
        chunks.push(step.value);
        return pump();
      });
    }
    return pump();
  }

  function concat(chunks) {
    var size = chunks.reduce(function (n, c) { return n + c.length; }, 0);
    var out = new Uint8Array(size);
    var at = 0;
    chunks.forEach(function (c) { out.set(c, at); at += c.length; });
    return out;
  }

  // The reader's line for a refused read; the status stays for support.
  function unreadable(status, lang) {
    return (UNREADABLE[lang] || UNREADABLE.en) + " (HTTP " + status + ")";
  }

  function count(rows, budget) {
    budget.rows -= rows.length;
    if (budget.rows < 0) throw limit("over 50,000 rows: aggregate on the server");
    return rows;
  }

  // A next link is followed only while it stays under the Endpoint's own URL; one that leaves it
  // stops the chart rather than drawing part of the rows as if they were all.
  function sameEndpoint(next, request) {
    if (next === undefined || next === null) return null;
    var prefix = request.url.slice(0, request.url.indexOf("?")).replace(/\/[^/]*$/, "/");
    if (typeof next !== "string" || next.indexOf(prefix) !== 0) throw new Error("the next page is not on the Endpoint");
    return next;
  }

  // The next window of a cut (206) temporal answer (T-5414). Antares names it in Link
  // rel="intervalafter", offsetN per attribute, so no instance is skipped or repeated; a broker
  // with Content-Range alone continues `after` the range's end, whose boundary instant comes again
  // (4.11 `after` includes it) and is merged once. Content-Range's end is the newest instance of
  // any entity, so it skips the rest of an entity cut earlier: the Link comes first.
  function nextWindow(response, url, request) {
    if (response.status !== 206) return null;
    var link = /<([^>]*)>[^<]*?rel="?intervalafter\b/.exec(response.headers.get("Link") || "");
    if (link) return sameEndpoint(new URL(link[1], url).href, request);
    var range = /^date-time \S+-(\d{4}-\d\d-\d\dT[^/\s]+)\/\S+$/.exec(response.headers.get("Content-Range") || "");
    if (!range || !/[?&]timerel=after(&|$)/.test(url)) return null;
    return url.replace(/([?&]timeAt=)[^&]*/, "$1" + encodeURIComponent(range[1]));
  }

  // A temporal answer's instances, every attribute of every entity.
  function instances(body, request) {
    var key = request.method || "values";
    return (Array.isArray(body) ? body : []).reduce(function (n, entity) {
      return request.attrs.reduce(function (m, attr) {
        return parts(entity, attr).reduce(function (k, p) {
          return k + (Array.isArray(p[key]) ? p[key].length : 0);
        }, m);
      }, n);
    }, 0);
  }

  // Windows of one temporal read, merged per entity, attribute and datasetId; an instant served
  // twice (the boundary of an `after` window) is kept once.
  function merger(request) {
    var key = request.method || "values";
    var entities = [];
    var byId = {};
    var byPart = {};
    var seen = {};
    return {
      entities: entities,
      add: function (body) {
        (Array.isArray(body) ? body : []).forEach(function (entity) {
          if (!entity || typeof entity.id !== "string") return;
          var into = byId[entity.id];
          if (!into) entities.push(into = byId[entity.id] = { id: entity.id });
          request.attrs.forEach(function (attr) {
            parts(entity, attr).forEach(function (p) {
              if (!Array.isArray(p[key])) return;
              var dataset = typeof p.datasetId === "string" ? p.datasetId : "";
              var at = entity.id + "\n" + attr + "\n" + dataset;
              var part = byPart[at];
              if (!part) {
                part = byPart[at] = {};
                if (dataset) part.datasetId = dataset;
                part[key] = [];
                (into[attr] = into[attr] || []).push(part);
              }
              var keys = seen[at] = seen[at] || {};
              p[key].forEach(function (pair) {
                var k = JSON.stringify(pair);
                if (!keys[k]) part[key].push(pair);
                keys[k] = true;
              });
            });
          });
        });
      }
    };
  }

  // `deadline`: past it no further window is asked for, so a slow read still draws before the
  // page's 15 s timeout; by default 3 s short of it.
  function load(view, api, lang, fetcher, now, deadline) {
    var source = JSON.parse(view.source || "{}");
    var request = firstRequest(source, view.endpoint, api, now);
    var budget = { rows: LIMIT_ROWS, bytes: LIMIT_BYTES };
    var stopAt = deadline || Date.now() + TIMEOUT_MS - 3000;
    var open = function (url) {
      return slot(function () {
        return retried(fetcher, url, { credentials: "omit", referrerPolicy: "no-referrer" }).then(function (r) {
          return read(r, budget).then(function (text) { return { response: r, text: text }; });
        });
      });
    };
    var get = function (url) {
      return open(url).then(function (page) { return page.text; });
    };
    var names = function (type) {
      var url = api + "/api/endpoint/" + view.endpoint + "/ngsi-ld/v1/entities?" +
        query({ type: type, pick: "id,name", format: "keyValues", lang: lang, limit: PAGE });
      return get(url).then(JSON.parse).then(function (list) {
        var map = {};
        (Array.isArray(list) ? list : []).forEach(function (e) { map[e.id] = text(e.name, lang); });
        return map;
      }, function () { return {}; });
    };
    if (request.kind === "csv") {
      return get(request.url).then(function (body) {
        count(new Array(Math.max(body.split("\n").length - 2, 0)), budget);
        return { format: "csv", text: body };
      });
    }
    if (request.kind === "temporal") {
      var merged = merger(request);
      var partial = false;
      // `block`: the entities the first window of this entity page listed; a full one (PAGE)
      // continues with the next entities once its windows are read.
      var windowOf = function (url, offset, block) {
        return open(url).then(function (page) {
          var body = JSON.parse(page.text);
          count(new Array(instances(body, request)), budget);
          return { page: page, body: body };
        }).catch(function (error) {
          if (!merged.entities.length || !(error.limit || error.name === "AbortError")) throw error;
          partial = true;
          return null;
        }).then(function (got) {
          if (!got) return null;
          merged.add(got.body);
          if (block === null) block = Array.isArray(got.body) ? got.body.length : 0;
          var next = nextWindow(got.page.response, url, request);
          if (next && (next === url || Date.now() > stopAt)) partial = true;
          else if (next) return windowOf(next, offset, block);
          if (!partial && block === PAGE) return windowOf(request.url + "&offset=" + (offset + PAGE), offset + PAGE, null);
          return null;
        });
      };
      return windowOf(request.url, 0, null).then(function () {
        return names(request.type);
      }).then(function (map) {
        var out = rowsFromTemporal(merged.entities, request, map);
        var data = { format: "json", rows: count(out, { rows: LIMIT_ROWS }) };
        if (partial) {
          var at = out.reduce(function (m, row) { return row.observedAt > m ? row.observedAt : m; }, "");
          data.partial = (PARTIAL[lang] || PARTIAL.en).replace("{at}", at ? at.slice(0, 16).replace("T", " ") + " UTC" : "?");
        }
        return data;
      });
    }
    var rows = [];
    function page(url) {
      return get(url).then(JSON.parse).then(function (body) {
        if (request.kind === "ogc") {
          rows = rows.concat(count(rowsFromOgc(body), budget));
          var next = (body.links || []).filter(function (l) { return l.rel === "next"; })[0];
          next = next && sameEndpoint(next.href, request);
          return next ? page(next) : rows;
        }
        rows = rows.concat(count(body.value || [], budget));
        var link = sameEndpoint(body["@odata.nextLink"], request);
        return link ? page(link) : rows;
      });
    }
    return page(request.url).then(function (list) {
      if (request.kind === "ogc") return { format: "json", rows: list };
      return names(request.type).then(function (map) {
        return { format: "json", rows: count(named(list, map, lang), { rows: LIMIT_ROWS }) };
      });
    });
  }

  function main() {
    var where = parseHash(location.hash);
    var h = hosts(location.hostname);
    var status = document.getElementById("status");
    var frame = document.getElementById("frame");
    var lang = (where && where.lang) || (navigator.language || "en").slice(0, 2);
    var fail = function (message) {
      status.textContent = message;
      status.className = "";
      status.hidden = false;
      frame.hidden = true;
      resize();
    };
    function resize() {
      // The content, not the viewport: a frame grows and shrinks with its chart.
      var height = Math.ceil(document.body.getBoundingClientRect().height);
      if (window.parent !== window) window.parent.postMessage({ type: "jc-viz-height", height: height }, "*");
    }
    if (location.hash === "#show") return show(h, frame, fail);
    if (!where) return fail("This page draws a chart of the open-data catalogue; the address names none.");
    if (where.inline) document.body.classList.add("inline");
    var controller = new AbortController();
    var timer = setTimeout(function () { controller.abort(); }, TIMEOUT_MS);
    var fetcher = function (url, options) {
      options.signal = controller.signal;
      return fetch(url, options);
    };
    var ready = new Promise(function (resolve) {
      window.addEventListener("message", function (event) {
        // The renderer runs in an opaque origin, so its messages come from "null"; the source
        // check is what ties them to our own frame.
        if (event.source !== frame.contentWindow || event.origin !== "null") return;
        var data = event.data || {};
        if (data.type === "jc-viz-ready") resolve();
        if (data.type === "jc-viz-height" && typeof data.height === "number") {
          frame.style.height = Math.min(Math.max(Math.round(data.height), 120), 2000) + "px";
          resize();
        }
        if (data.type === "jc-viz-error") fail(String(data.message || "The chart could not be drawn."));
      });
    });
    fetcher(h.data + "/api/3/action/resource_view_show?" + query({ id: where.id }), { credentials: "omit", referrerPolicy: "no-referrer" })
      .then(function (r) { return r.ok ? r.json() : Promise.reject(new Error("The chart is not published.")); })
      .then(function (body) {
        var view = body && body.result;
        if (!view || view.view_type !== "jc_visualization") throw new Error("The chart is not published.");
        var spec = JSON.parse(view.spec || "null");
        var errors = root.jcRefuse(spec, h.domain);
        if (errors.length) throw new Error("The chart was refused: " + errors[0].pointer + " — " + errors[0].rule + ".");
        var title = text(JSON.parse(view.titles || "null"), lang) || view.title || "";
        var description = text(JSON.parse(view.descriptions || "null"), lang) || view.description || "";
        document.title = title || document.title;
        document.getElementById("title").textContent = title;
        document.getElementById("description").textContent = description;
        frame.title = title;
        var table = document.getElementById("table");
        table.textContent = LINK[lang] || LINK.en;
        table.href = h.data + "/dataset/" + encodeURIComponent(view.package_id || "");
        return load(view, h.api, lang, fetcher, Date.now()).then(function (data) {
          if (data.partial) {
            status.textContent = data.partial;
            status.className = "note";
            status.hidden = false;
          }
          return ready.then(function () {
            frame.contentWindow.postMessage({ type: "jc-viz-render", spec: spec, data: data, lang: lang }, "*");
          });
        });
      })
      .catch(function (error) {
        if (error && error.status) return fail(unreadable(error.status, lang));
        fail(error && error.name === "AbortError" ? "The data took longer than 15 seconds." : String(error.message || error));
      })
      .then(function () { clearTimeout(timer); });
    window.addEventListener("resize", resize);
  }

  // `#show`: the assistant's widget hands one spec and its rows by message (ADR-N-053 §2.9). A
  // Vega spec goes to the renderer, which runs in an opaque origin as in §2.5; a map spec, which
  // holds no expression, goes to the map page in this origin, because the tiles answer CORS for
  // our hosts and never for `null`. Heights and errors go back to the parent's origin alone.
  function show(h, frame, fail) {
    var parent = null;
    var map = false;
    var mapFrame = null;
    document.body.classList.add("inline", "show");
    var tell = function (message) {
      if (parent) window.parent.postMessage(message, parent);
    };
    var resize = function () {
      tell({ type: "jc-viz-height", height: Math.ceil(document.body.getBoundingClientRect().height) });
    };
    var refuse = function (message) {
      fail(message);
      tell({ type: "jc-viz-error", message: message });
    };
    var pending = null;
    var ready = false; // the frame that draws has said so; the renderer may say it before the parent speaks
    var send = function () {
      if (!pending || !ready) return;
      (map ? mapFrame : frame).contentWindow.postMessage(pending, map ? location.origin : "*");
      pending = null;
    };
    window.addEventListener("message", function (event) {
      var data = event.data || {};
      if (event.source === window.parent && !parent) {
        if (!ourOrigin(event.origin, h.domain) || data.type !== "jc-viz-show") return;
        parent = event.origin;
        var lang = /^[a-z]{2}$/.test(data.lang) ? data.lang : "en";
        document.documentElement.lang = lang;
        map = !!data.spec && data.spec.$schema === "urn:joinedcontext:map:v1";
        var errors = map ? root.jcRefuseMap(data.spec, h.domain) : root.jcRefuse(data.spec, h.domain);
        if (errors.length) return refuse("The chart was refused: " + errors[0].pointer + " — " + errors[0].rule + ".");
        var title = map ? data.spec.title : null;
        document.getElementById("title").textContent = typeof title === "string" ? title : "";
        pending = map
          ? { type: "jc-viz-render", spec: data.spec, lang: lang }
          : { type: "jc-viz-render", spec: data.spec, data: { format: "json", rows: Array.isArray(data.rows) ? data.rows.slice(0, LIMIT_ROWS) : [] }, lang: lang };
        if (map) {
          // The renderer frame stays empty; the map page takes its place.
          mapFrame = document.createElement("iframe");
          mapFrame.id = "map";
          mapFrame.title = document.getElementById("title").textContent || "Map";
          mapFrame.setAttribute("sandbox", "allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox");
          mapFrame.src = "map.html";
          ready = false;
          frame.replaceWith(mapFrame);
        }
        return send();
      }
      var mine = map ? mapFrame && event.source === mapFrame.contentWindow && event.origin === location.origin
        : event.source === frame.contentWindow && event.origin === "null";
      if (!mine) return;
      if (data.type === "jc-viz-ready") {
        ready = true;
        send();
      }
      if (data.type === "jc-viz-height" && typeof data.height === "number") {
        (map ? mapFrame : frame).style.height = Math.min(Math.max(Math.round(data.height), 120), 2000) + "px";
        resize();
      }
      if (data.type === "jc-viz-error") refuse(String(data.message || "The chart could not be drawn."));
    });
    window.addEventListener("resize", resize);
  }

  root.jcHost = {
    ourOrigin: ourOrigin,
    unreadable: unreadable,
    parseHash: parseHash,
    hosts: hosts,
    duration: duration,
    firstRequest: firstRequest,
    rowsFromTemporal: rowsFromTemporal,
    nextWindow: nextWindow,
    rowsFromOgc: rowsFromOgc,
    named: named,
    load: load
  };
  if (typeof module === "object" && module.exports) module.exports = root.jcHost;
  else if (typeof document !== "undefined") main();
})(typeof self !== "undefined" ? self : this);
