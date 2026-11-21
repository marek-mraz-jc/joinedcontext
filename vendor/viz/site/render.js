// The renderer frame (ADR-N-053 §2.4, §2.5). It runs sandboxed to an opaque origin under a CSP
// that allows no connection, so a spec that got past every check still reads no cookie and calls
// no host. It takes one message from the host page, checks the spec again and draws it through
// Vega's expression interpreter, never through generated code.
(function () {
  "use strict";

  var chart = document.getElementById("chart");
  var domain = location.hostname.replace(/^viz\./, "");

  // To any parent: beside the Portal (jc slug mode) the host page is an opaque origin too, which no
  // named target matches, and a ready, a height or a refusal tells a framer nothing it lacks.
  function tell(message) {
    window.parent.postMessage(message, "*");
  }

  function height() {
    tell({ type: "jc-viz-height", height: Math.ceil(document.body.getBoundingClientRect().height) });
  }

  // Vega asks its loader for nothing a checked spec names; any ask is refused all the same.
  var refused = function () {
    return Promise.reject(new Error("the renderer loads nothing"));
  };
  var loader = { load: refused, sanitize: refused, http: refused, file: refused };

  // The view drawn last: a toolbar change of the host (T-7490) swaps its rows without a redraw.
  var shown = null;

  // T-7490: dates and times on an axis or in a tooltip in the page's language, 24-hour clock.
  var TIME = {
    sk: { days: ["nedeľa", "pondelok", "utorok", "streda", "štvrtok", "piatok", "sobota"], shortDays: ["ne", "po", "ut", "st", "št", "pi", "so"],
      months: ["január", "február", "marec", "apríl", "máj", "jún", "júl", "august", "september", "október", "november", "december"],
      shortMonths: ["jan", "feb", "mar", "apr", "máj", "jún", "júl", "aug", "sep", "okt", "nov", "dec"] },
    cs: { days: ["neděle", "pondělí", "úterý", "středa", "čtvrtek", "pátek", "sobota"], shortDays: ["ne", "po", "út", "st", "čt", "pá", "so"],
      months: ["leden", "únor", "březen", "duben", "květen", "červen", "červenec", "srpen", "září", "říjen", "listopad", "prosinec"],
      shortMonths: ["led", "úno", "bře", "dub", "kvě", "čvn", "čvc", "srp", "zář", "říj", "lis", "pro"] },
    fi: { days: ["sunnuntai", "maanantai", "tiistai", "keskiviikko", "torstai", "perjantai", "lauantai"], shortDays: ["su", "ma", "ti", "ke", "to", "pe", "la"],
      months: ["tammikuu", "helmikuu", "maaliskuu", "huhtikuu", "toukokuu", "kesäkuu", "heinäkuu", "elokuu", "syyskuu", "lokakuu", "marraskuu", "joulukuu"],
      shortMonths: ["tammi", "helmi", "maalis", "huhti", "touko", "kesä", "heinä", "elo", "syys", "loka", "marras", "joulu"] }
  };
  function timeLocale(lang) {
    var names = TIME[lang];
    if (!names) return undefined;
    return Object.assign({ dateTime: "%-d. %-m. %Y %H:%M:%S", date: "%-d. %-m. %Y", time: "%H:%M:%S", periods: ["", ""] }, names);
  }

  function draw(message) {
    // The chart theme (theme.js, T-6886) of the host page's scheme: light unless it says dark.
    var mode = message.theme === "dark" ? "dark" : "light";
    document.documentElement.dataset.theme = mode;
    document.body.style.backgroundColor = self.jcTheme[mode].background;
    var errors = self.jcRefuse(message.spec, domain);
    if (errors.length) throw new Error("The chart was refused: " + errors[0].pointer + " — " + errors[0].rule + ".");
    var data = message.data || {};
    // A CSV is read by csv.js and typed from its records: vega's own CSV reader runs `new
    // Function`, which this page's CSP refuses (T-6865).
    var rows = data.format === "csv" ? vega.read(self.jcCsv(String(data.text || "")), { type: "json", parse: "auto" }) : data.rows || [];
    shown = null;
    return vegaEmbed(chart, withRows(message.spec, rows, self.jcTheme[mode]), {
      ast: true,
      expr: vega.expressionInterpreter,
      loader: loader,
      renderer: "svg",
      timeFormatLocale: timeLocale(document.documentElement.lang),
      // Export only: no source, no compiled spec, no "open in the editor" (which posts the spec
      // to another site).
      actions: { export: true, source: false, compiled: false, editor: false },
      downloadFileName: "chart"
    }).then(function (result) {
      shown = result && result.view ? result.view : null;
      return result;
    });
  }

  // New rows for the data set `source` of the view drawn last; the spec is the one already checked.
  function swap(message) {
    if (!shown) throw new Error("The chart could not be drawn.");
    return shown.data("source", Array.isArray(message.rows) ? message.rows : []).runAsync();
  }

  // The rows go in as the named data set `source` before the first draw, so the chart never
  // draws empty first: Vega-Lite's top-level `datasets`, Vega's data entry of that name.
  // A Vega-Lite spec draws in the chart theme, under what its own config says, member by member.
  function themed(theme, own) {
    var config = Object.assign({}, theme);
    Object.keys(own || {}).forEach(function (key) {
      var mine = own[key];
      config[key] = mine && typeof mine === "object" && !Array.isArray(mine) && theme[key] && typeof theme[key] === "object"
        ? Object.assign({}, theme[key], mine) : mine;
    });
    return config;
  }

  function withRows(spec, rows, theme) {
    var copy = Object.assign({}, spec);
    if (/vega-lite/.test(spec.$schema)) {
      copy.datasets = Object.assign({}, spec.datasets, { source: rows });
      copy.config = themed(theme, spec.config);
      // T-5415: on a phone the legend goes under the chart in two columns with whole labels; at
      // the right it took half the width and cut "Praha 2-Legerova" to "Praha 2-Legero".
      if (window.innerWidth < 480) {
        var config = copy.config;
        // T-6865: a title wider than the plot, centred on it, ran out of the frame ("PM10 podľa
        // stanice (µg/m"), and long y labels took 60 % of the width: the title starts at the
        // chart's left edge and ends in an ellipsis, a y label gets at most a third of the width.
        var width = window.innerWidth;
        copy.config = Object.assign({}, config, {
          legend: Object.assign({}, config.legend, { orient: "bottom", columns: 2, labelLimit: 0 }),
          title: Object.assign({}, config.title, { anchor: "start", frame: "bounds", limit: width - 48 }),
          axisY: Object.assign({}, config.axisY, { labelLimit: Math.round(width / 3) })
        });
      }
    } else if (Array.isArray(spec.data)) {
      copy.data = spec.data.map(function (entry) {
        return entry.name === "source" ? Object.assign({}, entry, { values: rows }) : entry;
      });
    }
    return copy;
  }

  window.addEventListener("message", function (event) {
    // Our parent alone: on its own origin the host page's (the URL's; ours is opaque), beside the
    // Portal `null`, as the host runs sandboxed there too. The spec is checked again below.
    if (event.source !== window.parent || (event.origin !== location.origin && event.origin !== "null")) return;
    var message = event.data || {};
    if (message.type !== "jc-viz-render" && message.type !== "jc-viz-rows") return;
    if (message.type === "jc-viz-render") document.documentElement.lang = /^[a-z]{2}$/.test(message.lang) ? message.lang : "en";
    Promise.resolve()
      .then(function () { return message.type === "jc-viz-rows" ? swap(message) : draw(message); })
      .then(height, function (error) {
        tell({ type: "jc-viz-error", message: String((error && error.message) || error) });
      });
  });
  new ResizeObserver(height).observe(document.body);
  tell({ type: "jc-viz-ready" });
})();
