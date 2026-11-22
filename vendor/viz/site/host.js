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
    sk: "Graf je neúplný: zobrazuje iba údaje od {from} do {to}, novšie údaje alebo ďalšie stanice presahujú limity stránky.",
    cs: "Graf je neúplný: ukazuje jen data od {from} do {to}, novější data nebo další stanice přesahují limity stránky.",
    fi: "Kaavio on vajaa: se näyttää vain tiedot {from}–{to}, uudemmat tiedot tai muut asemat ylittävät sivun rajat.",
    en: "Partial chart: it shows only data from {from} to {to}; newer data or more stations are over the page's limits."
  };

  // T-7490: the toolbar of a temporal chart. Its state lives in the fragment after the view id, so
  // a link or an embed keeps the reader's choice: `r` range, `from`/`to` days of a custom range,
  // `g` granularity, `m` method, `a` attribute, `s` stations (comma-separated entity ids).
  var RANGES = ["PT24H", "P7D", "P31D"];
  var GRANS = ["raw", "PT1H", "P1D"];
  var METHODS = ["avg", "min", "max"];
  var AGGR = /^(avg|min|max|sum|totalCount|distinctCount|stddev|sumsq)$/;
  var DAY = 86400000;
  // Raw readings past a week come back hourly: a week of ten-minute readings of eight stations is
  // already 8,000 instances (payload bound).
  var RAW_DAYS = 7;
  // A window longer than the gateway's anonymous 31 days is read as consecutive 31-day daily
  // windows, at most WALK of them (the newest), never by asking the gateway for more.
  var WALK = 12;
  var DATE = /^\d{4}-\d\d-\d\d$/;
  var ENTITY = /^urn:[A-Za-z0-9][A-Za-z0-9:._~%+-]{0,254}$/;
  var TEXT = {
    sk: {
      controls: "Nastavenia grafu", range: "Obdobie", rangeHint: "Napr. 7 dní: posledný týždeň do teraz; Vlastné: od a do podľa dátumu.",
      PT24H: "24 hodín", P7D: "7 dní", P31D: "31 dní", custom: "Vlastné", from: "Od", to: "Do", dateHint: "Deň v UTC, napr. 1. november 2026.",
      gran: "Podrobnosť", granHint: "Napr. Hodinovo: jedna hodnota za každú hodinu.", raw: "Merania", PT1H: "Hodinovo", P1D: "Denne",
      method: "Výpočet", methodHint: "Napr. Maximum: najvyššia hodnota v hodine alebo dni.", avg: "Priemer", min: "Minimum", max: "Maximum",
      stations: "Stanice", stationsHint: "Vyberte jednu alebo viac staníc (na počítači so Ctrl alebo Cmd); bez výberu sú všetky.",
      attr: "Veličina", attrHint: "Napr. Teplota: graf ukáže iba ju.", all: "Všetky",
      loading: "Načítavam údaje…", empty: "Za toto obdobie graf nemá žiadne údaje. Skúste dlhšie obdobie alebo inú podrobnosť.",
      hourly: "Merania za viac ako 7 dní sa zobrazujú po hodinách.", daily: "Obdobie dlhšie ako 31 dní sa zobrazuje po dňoch.",
      capped: "Zobrazuje sa posledných {n} dní vybraného obdobia (limit stránky).", badDates: "Dátum Od musí byť najneskôr Do a nie v budúcnosti.",
      zoom: "Časy na osi sú v miestnom čase. Ťahaním v páse pod grafom priblížite časť obdobia. Klik na stanicu v legende ju zvýrazní.",
      time: "Čas", station: "Stanica", retry: "Skúsiť znova", resetZoom: "Zobraziť celé obdobie", firstData: "Prvé údaje vo vybranom období sú z {d}; skôr zdroj nemá merania."
    },
    cs: {
      controls: "Nastavení grafu", range: "Období", rangeHint: "Např. 7 dní: poslední týden do teď; Vlastní: od a do podle data.",
      PT24H: "24 hodin", P7D: "7 dní", P31D: "31 dní", custom: "Vlastní", from: "Od", to: "Do", dateHint: "Den v UTC, např. 1. listopad 2026.",
      gran: "Podrobnost", granHint: "Např. Po hodinách: jedna hodnota za každou hodinu.", raw: "Měření", PT1H: "Po hodinách", P1D: "Po dnech",
      method: "Výpočet", methodHint: "Např. Maximum: nejvyšší hodnota v hodině nebo dni.", avg: "Průměr", min: "Minimum", max: "Maximum",
      stations: "Stanice", stationsHint: "Vyberte jednu nebo více stanic (na počítači s Ctrl nebo Cmd); bez výběru jsou všechny.",
      attr: "Veličina", attrHint: "Např. Teplota: graf ukáže jen ji.", all: "Všechny",
      loading: "Načítám data…", empty: "Za toto období graf nemá žádná data. Zkuste delší období nebo jinou podrobnost.",
      hourly: "Měření za více než 7 dní se zobrazují po hodinách.", daily: "Období delší než 31 dní se zobrazuje po dnech.",
      capped: "Zobrazuje se posledních {n} dní vybraného období (limit stránky).", badDates: "Datum Od musí být nejpozději Do a ne v budoucnosti.",
      zoom: "Časy na ose jsou v místním čase. Tažením v pásu pod grafem přiblížíte část období. Klik na stanici v legendě ji zvýrazní.",
      time: "Čas", station: "Stanice", retry: "Zkusit znovu", resetZoom: "Zobrazit celé období", firstData: "První data ve vybraném období jsou z {d}; dříve zdroj nemá měření."
    },
    fi: {
      controls: "Kaavion asetukset", range: "Aikaväli", rangeHint: "Esim. 7 päivää: viimeinen viikko tähän asti; Oma: alku- ja loppupäivä.",
      PT24H: "24 tuntia", P7D: "7 päivää", P31D: "31 päivää", custom: "Oma", from: "Alkaen", to: "Asti", dateHint: "Päivä UTC-ajassa, esim. 1. marraskuuta 2026.",
      gran: "Tarkkuus", granHint: "Esim. Tunneittain: yksi arvo kultakin tunnilta.", raw: "Mittaukset", PT1H: "Tunneittain", P1D: "Päivittäin",
      method: "Laskenta", methodHint: "Esim. Maksimi: tunnin tai päivän suurin arvo.", avg: "Keskiarvo", min: "Minimi", max: "Maksimi",
      stations: "Asemat", stationsHint: "Valitse yksi tai useampi asema (tietokoneella Ctrl tai Cmd); ilman valintaa kaikki.",
      attr: "Suure", attrHint: "Esim. Lämpötila: kaavio näyttää vain sen.", all: "Kaikki",
      loading: "Ladataan tietoja…", empty: "Tältä aikaväliltä ei ole tietoja. Kokeile pidempää aikaväliä tai toista tarkkuutta.",
      hourly: "Yli 7 päivän mittaukset näytetään tunneittain.", daily: "Yli 31 päivän aikaväli näytetään päivittäin.",
      capped: "Näytetään valitun aikavälin viimeiset {n} päivää (sivun raja).", badDates: "Alkupäivän on oltava viimeistään loppupäivä eikä tulevaisuudessa.",
      zoom: "Akselin ajat ovat paikallista aikaa. Vedä kaavion alla olevassa kaistassa lähentääksesi. Napsauta asemaa selitteessä korostaaksesi sen.",
      time: "Aika", station: "Asema", retry: "Yritä uudelleen", resetZoom: "Näytä koko aikaväli", firstData: "Valitun aikavälin ensimmäiset tiedot ovat {d}; sitä ennen lähteellä ei ole mittauksia."
    },
    en: {
      controls: "Chart settings", range: "Range", rangeHint: "E.g. 7 days: the last week up to now; Custom: from and to by date.",
      PT24H: "24 hours", P7D: "7 days", P31D: "31 days", custom: "Custom", from: "From", to: "To", dateHint: "A UTC day, e.g. 1 November 2026.",
      gran: "Granularity", granHint: "E.g. Hourly: one value for each hour.", raw: "Readings", PT1H: "Hourly", P1D: "Daily",
      method: "Method", methodHint: "E.g. Maximum: the highest value of each hour or day.", avg: "Average", min: "Minimum", max: "Maximum",
      stations: "Stations", stationsHint: "Pick one or more stations (on a computer with Ctrl or Cmd); none picked means all.",
      attr: "Measure", attrHint: "E.g. Temperature: the chart shows it alone.", all: "All",
      loading: "Loading data…", empty: "The chart has no data for this range. Try a longer range or another granularity.",
      hourly: "Readings over more than 7 days are shown hourly.", daily: "A range over 31 days is shown daily.",
      capped: "Showing the last {n} days of the range picked (the page's limit).", badDates: "From must be on or before To and not in the future.",
      zoom: "Times on the axis are local. Drag in the strip under the chart to zoom into part of the range. Click a station in the legend to highlight it.",
      time: "Time", station: "Station", retry: "Try again", resetZoom: "Show the whole range", firstData: "The first data in the range picked is from {d}; the source has no readings before."
    }
  };
  // UN/CEFACT unit codes an answer's unitCode may carry, as a reader writes them.
  var UNITS = { CEL: "°C", KEL: "K", P1: "%", HPA: "hPa", A97: "hPa", PAL: "Pa", MMT: "mm", MTS: "m/s", KMH: "km/h", DD: "°",
    GQ: "µg/m³", GP: "mg/m³", "59": "ppm", "61": "ppb", WTT: "W", KWT: "kW", KWH: "kWh", LUX: "lx", DB: "dB", MTR: "m", CMT: "cm" };
  // The Smart Data Models units of the attributes the catalogue's weather and air charts draw, for
  // an answer without unitCode; `fraction`: the model stores 0..1 and a reader reads percent.
  var KNOWN = {
    temperature: { unit: "CEL", sk: "Teplota", cs: "Teplota", fi: "Lämpötila", en: "Temperature" },
    relativeHumidity: { unit: "P1", fraction: true, sk: "Relatívna vlhkosť", cs: "Relativní vlhkost", fi: "Suhteellinen kosteus", en: "Relative humidity" },
    atmosphericPressure: { unit: "HPA", sk: "Tlak vzduchu", cs: "Tlak vzduchu", fi: "Ilmanpaine", en: "Air pressure" },
    precipitation: { unit: "MMT", sk: "Zrážky", cs: "Srážky", fi: "Sademäärä", en: "Precipitation" },
    windSpeed: { unit: "MTS", sk: "Rýchlosť vetra", cs: "Rychlost větru", fi: "Tuulen nopeus", en: "Wind speed" },
    windDirection: { unit: "DD", sk: "Smer vetra", cs: "Směr větru", fi: "Tuulen suunta", en: "Wind direction" },
    pm10: { unit: "GQ", sk: "PM10", cs: "PM10", fi: "PM10", en: "PM10" },
    pm25: { unit: "GQ", sk: "PM2,5", cs: "PM2,5", fi: "PM2,5", en: "PM2.5" },
    no2: { unit: "GQ", sk: "NO₂", cs: "NO₂", fi: "NO₂", en: "NO₂" },
    o3: { unit: "GQ", sk: "O₃", cs: "O₃", fi: "O₃", en: "O₃" },
    so2: { unit: "GQ", sk: "SO₂", cs: "SO₂", fi: "SO₂", en: "SO₂" }
  };

  // `#{viewId}`, then `&inline` (the catalogue shows title and description itself) and
  // `&lang=sk` (the page's language) in any order.
  function parseHash(hash) {
    var parts = String(hash || "").replace(/^#/, "").split("&");
    // jc's catalogue mints `view-<uuid>` where CKAN minted the bare UUID: both name a view.
    if (!/^(?:view-)?[0-9a-f-]{36}$/.test(parts[0])) return null;
    var lang = parts.map(function (p) { return /^lang=([a-z]{2})$/.exec(p); }).filter(Boolean)[0];
    return { id: parts[0], inline: parts.indexOf("inline") > 0, lang: lang ? lang[1] : null };
  }

  // viz.{domain} → {domain}: the hosts the page reads are named by its own, never by a parameter.
  // jc's viz mount answers beside other modules and names the gateway and the catalogue in
  // `<meta name="jc-viz">` (as explore.js reads it): a meta that is present names both, and one it
  // leaves out or names badly is "" (none), never a guess.
  function hosts(hostname, meta) {
    var domain = String(hostname).replace(/^viz\./, "");
    var own = { domain: domain, api: "https://api." + domain, data: "https://opendata." + domain };
    if (meta == null) return own;
    var given = {};
    try { given = JSON.parse(meta) || {}; } catch (e) { given = {}; }
    [["api", "api"], ["data", "catalogue"]].forEach(function (pair) {
      var v = given[pair[1]];
      own[pair[0]] = typeof v === "string" && /^https?:\/\/[A-Za-z0-9.-]+(:[0-9]{1,5})?(\/[A-Za-z0-9._~\/-]*)?$/.test(v) ? v.replace(/\/+$/, "") : "";
    });
    return own;
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

  // The toolbar's defaults: what the view itself asks for. A `last`, an aggrPeriod or an aggrMethod
  // the toolbar does not list stays as an extra option, so no authored chart changes on its own.
  function defaults(source) {
    var s = source || {};
    var aggregated = !!s.aggrMethod && !!duration(s.aggrPeriod);
    return {
      range: duration(s.last) ? s.last : "P7D",
      from: null,
      to: null,
      gran: aggregated ? s.aggrPeriod : "raw",
      method: aggregated && AGGR.test(s.aggrMethod) ? s.aggrMethod : "avg",
      attr: null,
      stations: []
    };
  }

  // The fragment's toolbar state over the view's defaults. A part that is not one of the allowed
  // values is dropped, never repaired, so a link cannot put a value of its own into a URL.
  function parseState(hash, source) {
    var state = defaults(source);
    var own = defaults(source);
    var attrs = Array.isArray(source && source.attrs) ? source.attrs : [];
    String(hash || "").replace(/^#/, "").split("&").slice(1).forEach(function (part) {
      var at = part.indexOf("=");
      if (at < 1) return;
      var key = part.slice(0, at);
      var value;
      try { value = decodeURIComponent(part.slice(at + 1)); } catch (e) { return; }
      if (key === "r" && (RANGES.indexOf(value) >= 0 || value === own.range || value === "custom")) state.range = value;
      if ((key === "from" || key === "to") && day(value) !== null) state[key] = value;
      if (key === "g" && (GRANS.indexOf(value) >= 0 || value === own.gran)) state.gran = value;
      if (key === "m" && (METHODS.indexOf(value) >= 0 || value === own.method)) state.method = value;
      if (key === "a" && attrs.length > 1 && attrs.indexOf(value) >= 0) state.attr = value;
      if (key === "s") state.stations = value.split(",").filter(function (id) { return ENTITY.test(id); }).slice(0, 50);
    });
    if (state.range === "custom" && (!state.from || !state.to)) state.range = own.range;
    if (state.range !== "custom") state.from = state.to = null;
    return state;
  }

  // The fragment parts of a state that differ from the view's defaults, in a fixed order.
  function stateHash(state, source) {
    var own = defaults(source);
    var out = [];
    if (state.range !== own.range) out.push("r=" + encodeURIComponent(state.range));
    if (state.range === "custom") out.push("from=" + state.from, "to=" + state.to);
    if (state.gran !== own.gran) out.push("g=" + encodeURIComponent(state.gran));
    if (state.method !== own.method) out.push("m=" + encodeURIComponent(state.method));
    if (state.attr) out.push("a=" + encodeURIComponent(state.attr));
    if (state.stations.length) out.push("s=" + state.stations.map(encodeURIComponent).join(","));
    return out.join("&");
  }

  // A `YYYY-MM-DD` day as the UTC midnight it starts at, or null.
  function day(text) {
    if (!DATE.test(String(text || ""))) return null;
    var ms = Date.parse(text + "T00:00:00Z");
    return isNaN(ms) || new Date(ms).toISOString().slice(0, 10) !== text ? null : ms;
  }

  // A date keeps its parts on one line ("14. 11. 2026").
  function nb(text) {
    return String(text).replace(/ /g, "\u00a0");
  }

  function iso(ms) {
    return new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z");
  }

  // The reads of a temporal chart in the toolbar's state: one window for a range up to 31 days,
  // else consecutive daily windows of at most 31 days, the newest WALK of them. A preset range is
  // read `after` now minus the range (a cut answer continues by Content-Range), a custom or walked
  // one `between`. Throws on a source or a state no read may come from.
  function temporalPlan(source, state, endpoint, api, now) {
    firstRequest(source, endpoint, api, now);
    var s = source;
    var attrs = state.attr ? [state.attr] : s.attrs;
    var start;
    var end;
    var custom = state.range === "custom";
    if (custom) {
      start = day(state.from);
      end = day(state.to);
      if (start === null || end === null || start > end || start > now) throw new Error("dates");
      end = Math.min(end + DAY, now);
    } else {
      var span = duration(state.range);
      if (!span) throw new Error("range");
      start = now - span;
      end = now;
    }
    var gran = state.gran;
    var forced = null;
    if (gran !== "raw" && !duration(gran)) throw new Error("granularity");
    if (end - start > MAX_DAYS * DAY && gran !== "P1D") {
      gran = "P1D";
      forced = "daily";
    } else if (gran === "raw" && end - start > RAW_DAYS * DAY) {
      gran = "PT1H";
      forced = "hourly";
    }
    var method = gran === "raw" ? null : state.method;
    if (method !== null && !AGGR.test(method)) throw new Error("method");
    // An aggregated read starts on a whole hour or UTC day, so each bucket is one hour or one day
    // (the broker counts its periods from timeAt); a preset range whose whole day would pass the
    // gateway's 31 days starts on the next one.
    if (method) {
      var step = duration(gran);
      var floor = Math.floor(start / step) * step;
      start = !custom && end - floor > MAX_DAYS * DAY ? Math.ceil(start / step) * step : floor;
    }
    // Windows counted forward from the start keep that alignment; the newest WALK of them are read.
    var windows = [];
    for (var from = start; from < end; from += MAX_DAYS * DAY) windows.push({ from: from, to: Math.min(from + MAX_DAYS * DAY, end) });
    var capped = windows.length > WALK;
    windows = windows.slice(-WALK);
    var base = api + "/api/endpoint/" + endpoint + "/ngsi-ld/v1/temporal/entities?";
    var between = custom || windows.length > 1;
    return {
      kind: "temporal",
      type: s.type,
      attrs: attrs,
      method: method,
      gran: gran,
      forced: forced,
      start: windows[0].from,
      end: end,
      capped: capped ? Math.round((end - windows[0].from) / DAY) : 0,
      urls: windows.map(function (w) {
        return base + query({
          type: s.type,
          attrs: attrs.join(","),
          timerel: between ? "between" : "after",
          timeAt: iso(w.from),
          endTimeAt: between ? iso(w.to) : undefined,
          format: method ? "aggregatedValues" : "temporalValues",
          aggrMethods: method || undefined,
          aggrPeriodDuration: method ? gran : undefined,
          firstN: method ? undefined : PAGE,
          limit: PAGE
        });
      })
    };
  }

  // An attribute's axis title in the page's language with its unit, and whether its 0..1 values
  // are drawn as percent: the answer's unitCode first, else the model's.
  function measure(attr, unitCode, lang, values) {
    var known = KNOWN[attr] || {};
    var label = known[lang] || known.en || attr.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/^./, function (c) { return c.toUpperCase(); });
    var code = typeof unitCode === "string" ? unitCode : known.unit;
    var percent = !!known.fraction && code === "P1" && values.every(function (v) { return typeof v !== "number" || Math.abs(v) <= 1.5; });
    var unit = UNITS[code] || "";
    return { label: label, unit: unit, percent: percent, title: unit ? label + " (" + unit + ")" : label };
  }

  // The interactive form of a single-view temporal spec (a `mark`, an x of type temporal, nothing
  // composed and no params of its own): the authored chart at `width` with the y title and the
  // tooltip naming the measure and its unit, a legend that toggles a station (when colour is a
  // field), and a strip under it whose x-interval brush zooms the chart. Any other spec only gets
  // the titles and the full width.
  function interactive(spec, opts) {
    var copy = JSON.parse(JSON.stringify(spec));
    var enc = copy.encoding || {};
    var t = TEXT[opts.lang] || TEXT.en;
    if (opts.from && opts.to && opts.from !== opts.to) rename(copy, opts.from, opts.to);
    if (enc.y && enc.y.field === opts.attr && enc.y.title === undefined) enc.y.title = opts.measure.title;
    (Array.isArray(enc.tooltip) ? enc.tooltip : []).forEach(function (tip) {
      if (!tip || typeof tip !== "object" || tip.title !== undefined) return;
      if (tip.field === opts.attr) tip.title = opts.measure.title;
      else if (tip.type === "temporal") {
        tip.title = t.time;
        if (!tip.format) tip.format = opts.daily ? "%d.%m.%Y" : "%d.%m.%Y %H:%M";
      } else if (tip.field === "name") tip.title = t.station;
    });
    var single = copy.mark && enc.x && enc.x.type === "temporal" && enc.x.field &&
      !copy.layer && !copy.vconcat && !copy.hconcat && !copy.concat && !copy.facet && !copy.repeat && !copy.params && !enc.row && !enc.column;
    if (!single) {
      if (copy.width === undefined && !copy.vconcat && !copy.hconcat && !copy.concat && !copy.facet && !copy.repeat) copy.width = "container";
      return copy;
    }
    var outer = { $schema: copy.$schema, data: copy.data };
    ["datasets", "config", "title", "description"].forEach(function (key) {
      if (copy[key] !== undefined) outer[key] = copy[key];
      delete copy[key];
    });
    delete copy.$schema;
    delete copy.data;
    var width = Math.max(Math.round(opts.width), 200);
    copy.width = width;
    // A gap in the readings breaks the line (gapped() puts a null there) instead of passing for
    // readings; past 60 points a series draws without its markers.
    copy.mark = Object.assign({}, typeof copy.mark === "string" ? { type: copy.mark } : copy.mark, { invalid: "break-paths-show-domains" });
    if (opts.dense && copy.mark.point) copy.mark.point = false;
    // Daily points sit on their day and the ticks are days; finer data labels each tick with its
    // time and day, so a zoomed axis still says which day it shows.
    var ticks = opts.daily
      ? { format: "%-d. %-m.", tickMinStep: DAY }
      : { labelExpr: "[timeFormat(datum.value, '%H:%M'), timeFormat(datum.value, '%-d. %-m.')]" };
    enc.x.axis = enc.x.axis === null ? null : Object.assign({ labelOverlap: "greedy", labelSeparation: 8 }, ticks, enc.x.axis);
    if (copy.height === undefined) copy.height = 260;
    // A few pixels round the plot keep a point on its edge whole.
    enc.x.scale = Object.assign({ padding: 8 }, enc.x.scale, { domain: { param: "brush" } });
    if (enc.y) enc.y.scale = Object.assign({ padding: 8 }, enc.y.scale);
    var color = enc.color && enc.color.field;
    // Every station keeps its colour whichever of them are picked.
    if (color && Array.isArray(opts.names) && opts.names.length) enc.color.scale = Object.assign({ domain: opts.names }, enc.color.scale);
    if (color && !enc.opacity) {
      copy.params = [{ name: "pick", select: { type: "point", fields: [color] }, bind: "legend" }];
      enc.opacity = { condition: { param: "pick", value: 1 }, value: 0.15 };
      enc.color.legend = Object.assign({ orient: "bottom", columns: opts.width < 480 ? 2 : 4, labelLimit: 0 }, Array.isArray(opts.shown) && opts.shown.length ? { values: opts.shown } : {}, enc.color.legend);
    }
    var strip = {
      width: width,
      height: 44,
      mark: { type: "line", strokeWidth: 1, invalid: "break-paths-show-domains", point: opts.dense ? false : { size: 8, filled: true } },
      params: [{ name: "brush", select: { type: "interval", encodings: ["x"] } }],
      encoding: {
        x: { field: enc.x.field, type: "temporal", title: null, axis: { format: "%-d. %-m.", labelOverlap: "greedy", labelSeparation: 8, tickMinStep: DAY, tickCount: Math.max(Math.floor(width / 80), 2) } },
        y: Object.assign({}, enc.y, { title: null, axis: null, scale: { zero: false } })
      }
    };
    delete strip.encoding.y.sort;
    if (color) strip.encoding.color = { field: color, type: enc.color.type || "nominal", scale: enc.color.scale, legend: null };
    if (color && enc.detail) strip.encoding.detail = enc.detail;
    outer.vconcat = [copy, strip];
    return outer;
  }

  // Rows of each series (name and datasetId) in time order, with a row whose value is null
  // inside every gap over `factor` (default 3) times the series' median step, which breaks the
  // drawn line there. Aggregated buckets are regular, so there one missing bucket is a gap.
  function gapped(rows, attr, factor) {
    var over = factor || 3;
    var series = {};
    rows.forEach(function (row) {
      var key = row.name + "\n" + (row.datasetId || "");
      (series[key] = series[key] || []).push(row);
    });
    var out = [];
    Object.keys(series).forEach(function (key) {
      var list = series[key].slice().sort(function (a, b) { return a.observedAt < b.observedAt ? -1 : a.observedAt > b.observedAt ? 1 : 0; });
      var times = list.map(function (row) { return Date.parse(row.observedAt); });
      var steps = times.slice(1).map(function (t, i) { return t - times[i]; }).filter(function (d) { return d > 0; }).sort(function (a, b) { return a - b; });
      var step = steps.length ? steps[Math.floor(steps.length / 2)] : 0;
      list.forEach(function (row, i) {
        if (i && step && times[i] - times[i - 1] > over * step) {
          var hole = Object.assign({}, row, { observedAt: iso(times[i - 1] + step) });
          hole[attr] = null;
          out.push(hole);
        }
        out.push(row);
      });
    });
    return out;
  }

  // Every `field` naming one attribute names another (the toolbar's attribute picker).
  function rename(node, from, to) {
    if (Array.isArray(node)) return node.forEach(function (n) { rename(n, from, to); });
    if (!node || typeof node !== "object") return;
    Object.keys(node).forEach(function (key) {
      if (key === "field" && node[key] === from) node[key] = to;
      else rename(node[key], from, to);
    });
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
    // An opaque origin (the host beside the Portal, jc slug mode) has no lock manager: each
    // request fails with a SecurityError, so it counts its reads itself.
    if (locks && typeof locks.request === "function" && root.origin !== "null") {
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
    var units = {};
    return {
      entities: entities,
      units: units,
      add: function (body) {
        (Array.isArray(body) ? body : []).forEach(function (entity) {
          if (!entity || typeof entity.id !== "string") return;
          var into = byId[entity.id];
          if (!into) entities.push(into = byId[entity.id] = { id: entity.id });
          request.attrs.forEach(function (attr) {
            parts(entity, attr).forEach(function (p) {
              if (!Array.isArray(p[key])) return;
              if (typeof p.unitCode === "string" && !units[attr]) units[attr] = p.unitCode;
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
  // page's 15 s timeout; by default 3 s short of it. `state`: the toolbar's (T-7490), else the
  // view's own defaults.
  function load(view, api, lang, fetcher, now, deadline, state) {
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
      var plan = temporalPlan(source, state || defaults(source), view.endpoint, api, now);
      request = { kind: "temporal", type: plan.type, attrs: plan.attrs, method: plan.method, url: plan.urls[0] };
      var merged = merger(request);
      var partial = false;
      // `block`: the entities the first window of this entity page listed; a full one (PAGE)
      // continues with the next entities once its windows are read.
      var windowOf = function (base, url, offset, block) {
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
          else if (next) return windowOf(base, next, offset, block);
          if (!partial && block === PAGE) return windowOf(base, base + "&offset=" + (offset + PAGE), offset + PAGE, null);
          return null;
        });
      };
      // The windows of a walk one after another, oldest first; a cut one ends the walk.
      var walk = plan.urls.reduce(function (done, url) {
        return done.then(function () {
          if (partial) return null;
          if (Date.now() > stopAt && merged.entities.length) {
            partial = true;
            return null;
          }
          return windowOf(url, url, 0, null);
        });
      }, Promise.resolve());
      return walk.then(function () {
        return names(request.type);
      }).then(function (map) {
        var out = rowsFromTemporal(merged.entities, request, map);
        var data = { format: "json", rows: count(out, { rows: LIMIT_ROWS }), plan: plan, units: merged.units };
        if (partial) {
          // The span the rows cover: a cut answer can miss either end of the range picked.
          var times = out.map(function (row) { return row.observedAt; }).sort();
          var stamp = function (at) {
            return at ? nb(new Date(at).toLocaleString(lang, { day: "numeric", month: "numeric", year: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23" })) : "?";
          };
          data.partial = (PARTIAL[lang] || PARTIAL.en).replace("{from}", stamp(times[0])).replace("{to}", stamp(times[times.length - 1]));
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
    var meta = document.querySelector('meta[name="jc-viz"]');
    var h = hosts(location.hostname, meta ? meta.getAttribute("content") : null);
    var status = document.getElementById("status");
    var frame = document.getElementById("frame");
    var lang = (where && where.lang) || (navigator.language || "en").slice(0, 2);
    var say = function (message, kind) {
      status.textContent = message;
      status.className = kind || "";
      status.hidden = !message;
      resize();
    };
    var fail = function (message) {
      say(message, "");
      frame.hidden = true;
    };
    function resize() {
      // The content, not the viewport: a frame grows and shrinks with its chart.
      var height = Math.ceil(document.body.getBoundingClientRect().height);
      if (window.parent !== window) window.parent.postMessage({ type: "jc-viz-height", height: height }, "*");
    }
    if (location.hash === "#show") return show(h, frame, fail);
    if (!where) return fail("This page draws a chart of the open-data catalogue; the address names none.");
    if (!h.data || !h.api) return fail("This page names no open-data catalogue to draw a chart from.");
    if (where.inline) document.body.classList.add("inline");
    // One controller and one 15 s timer per read: a toolbar change aborts the read before it.
    var controller = null;
    var timer = null;
    var fetcher = function (url, options) {
      options.signal = controller.signal;
      return fetch(url, options);
    };
    var fresh = function () {
      if (controller) controller.abort();
      clearTimeout(timer);
      var mine = controller = new AbortController();
      timer = setTimeout(function () { mine.abort(); }, TIMEOUT_MS);
      return mine;
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
        if (data.type === "jc-viz-zoom") document.dispatchEvent(new CustomEvent("jc-viz-zoom", { detail: data.zoomed === true }));
      });
    });
    var failed = function (error) {
      if (error && error.status) return fail(unreadable(error.status, lang));
      fail(error && error.name === "AbortError" ? "The data took longer than 15 seconds." : String(error.message || error));
    };
    fresh();
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
        var source = JSON.parse(view.source || "{}");
        if (source.source === "temporal") return temporal(view, spec, source);
        return load(view, h.api, lang, fetcher, Date.now()).then(function (data) {
          if (data.partial) say(data.partial, "note");
          return ready.then(function () {
            frame.contentWindow.postMessage({ type: "jc-viz-render", spec: spec, data: data, lang: lang }, "*");
          });
        });
      })
      .catch(failed)
      .then(function () { clearTimeout(timer); });
    window.addEventListener("resize", resize);

    // A temporal chart (T-7490): the toolbar over the frame, each change a new read whose rows go
    // into the running view; a change of attribute or width draws the spec again.
    function temporal(view, spec, source) {
      var t = TEXT[lang] || TEXT.en;
      var state = parseState(location.hash, source);
      var bar = source.controls === false ? null : toolbar(document, source, state, t, lang, function (next) {
        state = next;
        remember();
        read();
      });
      var all = [];
      var units = {};
      var plan = null;
      var drawn = null; // the key of the spec in the frame
      var note = document.createElement("p");
      note.id = "controls-note";
      note.className = "hint";
      if (bar) bar.form.appendChild(note);
      var retry = document.createElement("button");
      retry.type = "button";
      retry.className = "retry";
      retry.textContent = t.retry;
      retry.hidden = true;
      retry.addEventListener("click", function () { read(); });
      status.parentNode.insertBefore(retry, status.nextSibling);
      var skeleton = document.createElement("div");
      skeleton.className = "skeleton";
      skeleton.setAttribute("aria-hidden", "true");
      skeleton.hidden = true;
      frame.parentNode.insertBefore(skeleton, frame);
      var zoom = document.createElement("p");
      zoom.className = "hint";
      zoom.textContent = t.zoom;
      // Drawing the chart again clears the brush (a double click is hard to find on a phone).
      var reset = document.createElement("button");
      reset.type = "button";
      reset.className = "retry";
      reset.textContent = t.resetZoom;
      reset.hidden = true;
      document.addEventListener("jc-viz-zoom", function (event) { reset.hidden = !event.detail; });
      reset.addEventListener("click", function () {
        drawn = null;
        draw();
      });
      function remember() {
        var extra = stateHash(state, source);
        var parts = location.hash.replace(/^#/, "").split("&").filter(function (p) { return p.indexOf("=") < 0 || /^lang=/.test(p); });
        try { history.replaceState(null, "", "#" + parts.concat(extra ? [extra] : []).join("&")); } catch (e) { /* the hash stays */ }
      }
      function picked() {
        // A picked station the answer lacks is dropped: the rest still draw.
        state.stations = state.stations.filter(function (id) { return all.some(function (row) { return row.id === id; }); });
        if (!state.stations.length) return all;
        return all.filter(function (row) { return state.stations.indexOf(row.id) >= 0; });
      }
      function draw() {
        var attr = plan.attrs.length === 1 ? plan.attrs[0] : null;
        var rows = picked();
        var values = attr ? all.map(function (row) { return row[attr]; }) : [];
        var m = attr ? measure(attr, units[attr], lang, values) : null;
        if (m && m.percent) {
          rows = rows.map(function (row) {
            var copy = Object.assign({}, row);
            if (typeof copy[attr] === "number") copy[attr] = Math.round(copy[attr] * 1000) / 10;
            return copy;
          });
        }
        var daily = plan.gran === "P1D";
        // A daily bucket starts at its UTC midnight: drawn at that day's local midnight, it sits on
        // the day's tick in any time zone.
        if (daily) rows = rows.map(function (row) { return Object.assign({}, row, { observedAt: String(row.observedAt).slice(0, 10) + "T00:00:00" }); });
        var authored = spec.encoding && spec.encoding.y && source.attrs.indexOf(spec.encoding.y.field) >= 0 ? spec.encoding.y.field : null;
        if (attr) rows = gapped(rows, attr, plan.method ? 1.5 : 3);
        var series = {};
        rows.forEach(function (row) { series[row.name] = (series[row.name] || 0) + 1; });
        var dense = Object.keys(series).some(function (k) { return series[k] > 60; });
        var width = Math.max(frame.clientWidth || document.body.clientWidth || 600, 200) - 70;
        var names = Object.keys(all.reduce(function (seen, row) { seen[row.name] = true; return seen; }, {})).sort();
        var shownNames = Object.keys(rows.reduce(function (seen, row) { seen[row.name] = true; return seen; }, {})).sort();
        var key = [attr, m && m.title, dense, daily, names.join(","), shownNames.join(","), Math.round(width / 40)].join("|");
        // An empty answer keeps the last station list, so the reader can still change it.
        if (bar && all.length) bar.stations(all, state.stations);
        frame.hidden = zoom.hidden = !rows.length;
        if (!rows.length) reset.hidden = true;
        if (!rows.length) return say(t.empty, "note");
        if (drawn === key) {
          frame.contentWindow.postMessage({ type: "jc-viz-rows", rows: rows }, "*");
          return;
        }
        drawn = key;
        reset.hidden = true; // a new drawing starts unzoomed
        var shown = attr ? interactive(spec, { attr: attr, measure: m, lang: lang, width: width, dense: dense, daily: daily, names: names, shown: shownNames, from: authored, to: state.attr }) : spec;
        frame.contentWindow.postMessage({ type: "jc-viz-render", spec: shown, data: { format: "json", rows: rows }, lang: lang }, "*");
        if (shown.vconcat && !zoom.parentNode) {
          frame.parentNode.insertBefore(zoom, frame.nextSibling);
          zoom.parentNode.insertBefore(reset, zoom.nextSibling);
        }
      }
      function read() {
        var mine = fresh();
        if (bar) bar.busy(true);
        frame.setAttribute("aria-busy", "true");
        say(t.loading, "loading");
        retry.hidden = true;
        // The chart's place stays taken while the first rows load, so the page does not jump.
        if (!plan) frame.hidden = true;
        skeleton.hidden = !frame.hidden;
        var p;
        try {
          p = load(view, h.api, lang, fetcher, Date.now(), null, state);
        } catch (error) {
          p = Promise.reject(error);
        }
        return p.then(function (data) {
          if (mine !== controller) return null;
          all = data.rows;
          units = data.units || {};
          plan = data.plan;
          var notes = [];
          if (plan.forced) {
            // The toolbar shows what is drawn: the forced granularity becomes the choice.
            notes.push(t[plan.forced]);
            state.gran = plan.gran;
            if (bar) bar.gran(plan.gran);
            remember();
          }
          var late = "";
          var first = all.reduce(function (m, row) { var at = Date.parse(row.observedAt); return at < m ? at : m; }, Infinity);
          // An answer is read oldest first, so a cut one misses its end: a late first reading is
          // still where the source starts, and both notes hold.
          if (first !== Infinity && first - plan.start > Math.max(DAY, (plan.end - plan.start) / 10)) {
            late = t.firstData.replace("{d}", nb(new Date(first).toLocaleDateString(lang, { day: "numeric", month: "numeric", year: "numeric" })));
          }
          if (plan.capped) notes.push(t.capped.replace(/\{n\}/g, plan.capped));
          note.textContent = notes.join(" ");
          say([data.partial, late].filter(Boolean).join(" "), "note");
          frame.hidden = zoom.hidden = false;
          return ready.then(draw);
        }).catch(function (error) {
          if (mine !== controller) return null;
          note.textContent = "";
          zoom.hidden = reset.hidden = true;
          if (error && error.message === "dates") return say(t.badDates, "");
          failed(error);
          retry.hidden = false;
        }).then(function () {
          if (mine !== controller) return;
          skeleton.hidden = true;
          clearTimeout(timer);
          frame.removeAttribute("aria-busy");
          if (bar) bar.busy(false);
        });
      }
      if (bar) bar.onStations(function (ids) {
        state.stations = ids;
        remember();
        if (plan) draw();
      });
      var resizing = null;
      window.addEventListener("resize", function () {
        clearTimeout(resizing);
        resizing = setTimeout(function () { if (plan && !frame.hidden) draw(); }, 200);
      });
      return read();
    }
  }

  // The toolbar: labelled native selects, each with a hint line, in the page's language. `change`
  // gets a new state for a change that needs a new read; stations filter the rows already read.
  function toolbar(doc, source, state, t, lang, change) {
    var own = defaults(source);
    var form = doc.createElement("form");
    form.className = "controls";
    form.setAttribute("aria-label", t.controls);
    form.addEventListener("submit", function (event) { event.preventDefault(); });
    var figure = doc.getElementById("status").parentNode;
    figure.insertBefore(form, doc.getElementById("status"));
    function field(id, label, hint, control) {
      var box = doc.createElement("div");
      box.className = "control";
      var l = doc.createElement("label");
      l.htmlFor = id;
      l.textContent = label;
      control.id = id;
      var small = doc.createElement("small");
      small.id = id + "-hint";
      small.className = "hint";
      small.textContent = hint;
      control.setAttribute("aria-describedby", small.id);
      box.appendChild(l);
      box.appendChild(control);
      box.appendChild(small);
      form.appendChild(box);
      return box;
    }
    function select(values, labels, value) {
      var el = doc.createElement("select");
      values.forEach(function (v, i) {
        var o = doc.createElement("option");
        o.value = v;
        o.textContent = labels[i];
        el.appendChild(o);
      });
      el.value = value;
      return el;
    }
    function label(v) {
      return t[v] || v;
    }
    var ranges = RANGES.indexOf(own.range) >= 0 ? RANGES.slice() : RANGES.concat([own.range]);
    ranges.push("custom");
    var range = select(ranges, ranges.map(label), state.range);
    field("c-range", t.range, t.rangeHint, range);
    var today = new Date().toISOString().slice(0, 10);
    var from = doc.createElement("input");
    var to = doc.createElement("input");
    [from, to].forEach(function (input) {
      input.type = "date";
      input.max = today;
      input.required = true;
    });
    from.value = state.from || new Date(Date.now() - 30 * DAY).toISOString().slice(0, 10);
    to.value = state.to || today;
    var fromBox = field("c-from", t.from, t.dateHint, from);
    var toBox = field("c-to", t.to, t.dateHint, to);
    var grans = GRANS.indexOf(own.gran) >= 0 ? GRANS : GRANS.concat([own.gran]);
    var gran = select(grans, grans.map(label), state.gran);
    field("c-gran", t.gran, t.granHint, gran);
    var methods = METHODS.indexOf(own.method) >= 0 ? METHODS : METHODS.concat([own.method]);
    var method = select(methods, methods.map(label), state.method);
    var methodBox = field("c-method", t.method, t.methodHint, method);
    var attr = null;
    if (source.attrs.length > 1) {
      attr = select([""].concat(source.attrs), [t.all].concat(source.attrs.map(function (a) { return measure(a, null, lang, []).label; })), state.attr || "");
      field("c-attr", t.attr, t.attrHint, attr);
    }
    var stations = doc.createElement("select");
    stations.multiple = true;
    stations.size = 4;
    var stationsBox = field("c-stations", t.stations, t.stationsHint, stations);
    stationsBox.hidden = true;
    function shape() {
      var custom = range.value === "custom";
      fromBox.hidden = toBox.hidden = !custom;
      methodBox.hidden = gran.value === "raw";
    }
    shape();
    function changed() {
      shape();
      if (range.value === "custom" && (!from.checkValidity() || !to.checkValidity() || day(from.value) === null || day(to.value) === null)) return;
      change({
        range: range.value,
        from: range.value === "custom" ? from.value : null,
        to: range.value === "custom" ? to.value : null,
        gran: gran.value,
        method: method.value,
        attr: attr && attr.value ? attr.value : null,
        stations: state.stations
      });
    }
    [range, from, to, gran, method].concat(attr ? [attr] : []).forEach(function (el) { el.addEventListener("change", changed); });
    var onStations = function () {};
    stations.addEventListener("change", function () {
      var ids = Array.prototype.filter.call(stations.options, function (o) { return o.selected; }).map(function (o) { return o.value; });
      state.stations = ids;
      onStations(ids);
    });
    return {
      form: form,
      busy: function (on) { form.setAttribute("aria-busy", on ? "true" : "false"); },
      onStations: function (f) { onStations = f; },
      gran: function (value) {
        gran.value = value;
        shape();
      },
      // The stations of the rows read, by name; a picked one the answer lacks is dropped.
      stations: function (rows, picked) {
        var seen = {};
        var list = [];
        rows.forEach(function (row) {
          if (!seen[row.id]) list.push(seen[row.id] = { id: row.id, name: String(row.name || row.id) });
        });
        list.sort(function (a, b) { return a.name.localeCompare(b.name); });
        stations.textContent = "";
        list.forEach(function (s) {
          var o = doc.createElement("option");
          o.value = s.id;
          o.textContent = s.name;
          o.selected = picked.indexOf(s.id) >= 0;
          stations.appendChild(o);
        });
        stations.size = Math.min(Math.max(list.length, 2), 6);
        stationsBox.hidden = list.length < 2;
      }
    };
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
        // The host page's scheme for the chart theme (T-6886): light unless it says dark.
        var theme = data.theme === "dark" ? "dark" : "light";
        document.documentElement.dataset.theme = theme;
        document.documentElement.lang = lang;
        map = !!data.spec && data.spec.$schema === "urn:joinedcontext:map:v1";
        var errors = map ? root.jcRefuseMap(data.spec, h.domain) : root.jcRefuse(data.spec, h.domain);
        if (errors.length) return refuse("The chart was refused: " + errors[0].pointer + " — " + errors[0].rule + ".");
        var title = map ? data.spec.title : null;
        document.getElementById("title").textContent = typeof title === "string" ? title : "";
        pending = map
          ? { type: "jc-viz-render", spec: data.spec, lang: lang, theme: theme }
          : { type: "jc-viz-render", spec: data.spec, data: { format: "json", rows: Array.isArray(data.rows) ? data.rows.slice(0, LIMIT_ROWS) : [] }, lang: lang, theme: theme };
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
    defaults: defaults,
    parseState: parseState,
    stateHash: stateHash,
    temporalPlan: temporalPlan,
    measure: measure,
    interactive: interactive,
    gapped: gapped,
    rowsFromOgc: rowsFromOgc,
    named: named,
    load: load
  };
  if (typeof module === "object" && module.exports) module.exports = root.jcHost;
  else if (typeof document !== "undefined") main();
})(typeof self !== "undefined" ? self : this);
