var JC_FIELDS = {
  "$comment": "One importance order and one label table for NGSI-LD attributes (T-4661): the assistant widget labels its table columns from it, and the Portal orders entity fields from its byte copy (ui/src/data/field-order.json). Groups are in order; an attribute no group lists goes to 'value' when it is a number, else to 'other'.",
  "groups": [
    { "group": "name", "attributes": ["name", "title", "alternateName"] },
    { "group": "time", "attributes": ["startDate", "startTime", "endDate", "endTime", "dateObserved", "observedAt", "validFrom", "validTo", "openingHours"] },
    { "group": "value", "attributes": ["no2", "pm10", "pm25", "pm1", "o3", "so2", "co", "nox", "no", "airQualityIndex", "temperature", "relativeHumidity", "atmosphericPressure", "precipitation", "windSpeed", "windDirection", "intensity", "occupancy", "availableSpotNumber", "totalSpotNumber", "capacity", "kpiValue", "pupilCount"] },
    { "group": "category", "attributes": ["category", "eventCategory", "status", "airQualityLevel", "reliability", "eventType", "typeOfLocation"] },
    { "group": "place", "attributes": ["address", "addressLocality", "streetAddress", "postalCode", "areaServed", "locationName", "contactPoint", "telephone", "email"] },
    { "group": "link", "attributes": ["url", "image"] },
    { "group": "other", "attributes": ["description"] },
    { "group": "meta", "attributes": ["source", "dataProvider", "owner", "seeAlso", "dateCreated", "dateModified", "dateIssued", "dateRetrieved", "type"] },
    { "group": "location", "attributes": ["location"] },
    { "group": "id", "attributes": ["id", "_id"] }
  ],
  "labels": {
    "name": { "sk": "Názov", "cs": "Název", "en": "Name", "de": "Name" },
    "title": { "sk": "Názov", "cs": "Název", "en": "Title", "de": "Titel" },
    "alternateName": { "sk": "Iný názov", "cs": "Jiný název", "en": "Other name", "de": "Anderer Name" },
    "id": { "sk": "Identifikátor", "cs": "Identifikátor", "en": "Identifier", "de": "Kennung" },
    "_id": { "sk": "Identifikátor", "cs": "Identifikátor", "en": "Identifier", "de": "Kennung" },
    "type": { "sk": "Typ", "cs": "Typ", "en": "Type", "de": "Typ" },
    "startDate": { "sk": "Začiatok", "cs": "Začátek", "en": "Start", "de": "Beginn" },
    "startTime": { "sk": "Čas", "cs": "Čas", "en": "Time", "de": "Uhrzeit" },
    "endDate": { "sk": "Koniec", "cs": "Konec", "en": "End", "de": "Ende" },
    "endTime": { "sk": "Čas konca", "cs": "Čas konce", "en": "End time", "de": "Endzeit" },
    "validFrom": { "sk": "Platí od", "cs": "Platí od", "en": "Valid from", "de": "Gültig ab" },
    "validTo": { "sk": "Platí do", "cs": "Platí do", "en": "Valid to", "de": "Gültig bis" },
    "openingHours": { "sk": "Otváracie hodiny", "cs": "Otevírací doba", "en": "Opening hours", "de": "Öffnungszeiten" },
    "observedAt": { "sk": "Čas merania", "cs": "Čas měření", "en": "Measured", "de": "Gemessen" },
    "dateObserved": { "sk": "Dátum pozorovania", "cs": "Datum pozorování", "en": "Date observed", "de": "Beobachtet" },
    "temperature": { "sk": "Teplota", "cs": "Teplota", "en": "Temperature", "de": "Temperatur" },
    "relativeHumidity": { "sk": "Vlhkosť", "cs": "Vlhkost", "en": "Humidity", "de": "Luftfeuchte" },
    "atmosphericPressure": { "sk": "Tlak", "cs": "Tlak", "en": "Pressure", "de": "Luftdruck" },
    "precipitation": { "sk": "Zrážky", "cs": "Srážky", "en": "Precipitation", "de": "Niederschlag" },
    "windSpeed": { "sk": "Rýchlosť vetra", "cs": "Rychlost větru", "en": "Wind speed", "de": "Windgeschwindigkeit" },
    "windDirection": { "sk": "Smer vetra", "cs": "Směr větru", "en": "Wind direction", "de": "Windrichtung" },
    "pm10": { "sk": "PM10", "cs": "PM10", "en": "PM10", "de": "PM10" },
    "pm25": { "sk": "PM2,5", "cs": "PM2,5", "en": "PM2.5", "de": "PM2,5" },
    "pm1": { "sk": "PM1", "cs": "PM1", "en": "PM1", "de": "PM1" },
    "no2": { "sk": "NO₂", "cs": "NO₂", "en": "NO₂", "de": "NO₂" },
    "no": { "sk": "NO", "cs": "NO", "en": "NO", "de": "NO" },
    "nox": { "sk": "NOₓ", "cs": "NOₓ", "en": "NOₓ", "de": "NOₓ" },
    "o3": { "sk": "O₃", "cs": "O₃", "en": "O₃", "de": "O₃" },
    "so2": { "sk": "SO₂", "cs": "SO₂", "en": "SO₂", "de": "SO₂" },
    "co": { "sk": "CO", "cs": "CO", "en": "CO", "de": "CO" },
    "airQualityIndex": { "sk": "Index kvality ovzdušia", "cs": "Index kvality ovzduší", "en": "Air quality index", "de": "Luftqualitätsindex" },
    "airQualityLevel": { "sk": "Kvalita ovzdušia", "cs": "Kvalita ovzduší", "en": "Air quality", "de": "Luftqualität" },
    "intensity": { "sk": "Intenzita", "cs": "Intenzita", "en": "Intensity", "de": "Stärke" },
    "occupancy": { "sk": "Obsadenosť", "cs": "Obsazenost", "en": "Occupancy", "de": "Belegung" },
    "availableSpotNumber": { "sk": "Voľné miesta", "cs": "Volná místa", "en": "Free spaces", "de": "Freie Plätze" },
    "totalSpotNumber": { "sk": "Miesta spolu", "cs": "Míst celkem", "en": "Total spaces", "de": "Plätze gesamt" },
    "capacity": { "sk": "Kapacita", "cs": "Kapacita", "en": "Capacity", "de": "Kapazität" },
    "kpiValue": { "sk": "Hodnota", "cs": "Hodnota", "en": "Value", "de": "Wert" },
    "pupilCount": { "sk": "Počet žiakov", "cs": "Počet žáků", "en": "Pupils", "de": "Schüler" },
    "category": { "sk": "Kategória", "cs": "Kategorie", "en": "Category", "de": "Kategorie" },
    "eventCategory": { "sk": "Kategória", "cs": "Kategorie", "en": "Category", "de": "Kategorie" },
    "eventType": { "sk": "Druh", "cs": "Druh", "en": "Kind", "de": "Art" },
    "typeOfLocation": { "sk": "Druh miesta", "cs": "Druh místa", "en": "Kind of place", "de": "Ortsart" },
    "status": { "sk": "Stav", "cs": "Stav", "en": "Status", "de": "Status" },
    "reliability": { "sk": "Spoľahlivosť", "cs": "Spolehlivost", "en": "Reliability", "de": "Zuverlässigkeit" },
    "description": { "sk": "Popis", "cs": "Popis", "en": "Description", "de": "Beschreibung" },
    "address": { "sk": "Adresa", "cs": "Adresa", "en": "Address", "de": "Adresse" },
    "addressLocality": { "sk": "Obec", "cs": "Obec", "en": "Locality", "de": "Ort" },
    "streetAddress": { "sk": "Ulica", "cs": "Ulice", "en": "Street", "de": "Straße" },
    "postalCode": { "sk": "PSČ", "cs": "PSČ", "en": "Postcode", "de": "PLZ" },
    "addressRegion": { "sk": "Kraj", "cs": "Kraj", "en": "Region", "de": "Region" },
    "addressCountry": { "sk": "Krajina", "cs": "Země", "en": "Country", "de": "Land" },
    "areaServed": { "sk": "Oblasť", "cs": "Oblast", "en": "Area", "de": "Gebiet" },
    "locationName": { "sk": "Miesto", "cs": "Místo", "en": "Place", "de": "Ort" },
    "contactPoint": { "sk": "Kontakt", "cs": "Kontakt", "en": "Contact", "de": "Kontakt" },
    "telephone": { "sk": "Telefón", "cs": "Telefon", "en": "Phone", "de": "Telefon" },
    "email": { "sk": "E-mail", "cs": "E-mail", "en": "Email", "de": "E-Mail" },
    "url": { "sk": "Odkaz", "cs": "Odkaz", "en": "Link", "de": "Link" },
    "image": { "sk": "Obrázok", "cs": "Obrázek", "en": "Image", "de": "Bild" },
    "source": { "sk": "Zdroj", "cs": "Zdroj", "en": "Source", "de": "Quelle" },
    "dataProvider": { "sk": "Poskytovateľ", "cs": "Poskytovatel", "en": "Provider", "de": "Anbieter" },
    "owner": { "sk": "Vlastník", "cs": "Vlastník", "en": "Owner", "de": "Eigentümer" },
    "seeAlso": { "sk": "Pozri aj", "cs": "Viz také", "en": "See also", "de": "Siehe auch" },
    "dateCreated": { "sk": "Vytvorené", "cs": "Vytvořeno", "en": "Created", "de": "Erstellt" },
    "dateModified": { "sk": "Zmenené", "cs": "Změněno", "en": "Modified", "de": "Geändert" },
    "dateIssued": { "sk": "Vydané", "cs": "Vydáno", "en": "Issued", "de": "Ausgegeben" },
    "dateRetrieved": { "sk": "Načítané", "cs": "Načteno", "en": "Retrieved", "de": "Abgerufen" },
    "location": { "sk": "Poloha", "cs": "Poloha", "en": "Location", "de": "Lage" },
    "longitude": { "sk": "Zemepisná dĺžka", "cs": "Zeměpisná délka", "en": "Longitude", "de": "Längengrad" },
    "latitude": { "sk": "Zemepisná šírka", "cs": "Zeměpisná šířka", "en": "Latitude", "de": "Breitengrad" },
    "geometryType": { "sk": "Typ geometrie", "cs": "Typ geometrie", "en": "Geometry type", "de": "Geometrietyp" }
  },
  "enums": {
    "museumsGalleriesLibraries": { "sk": "Múzeá, galérie, knižnice", "cs": "Muzea, galerie, knihovny", "en": "Museums, galleries, libraries", "de": "Museen, Galerien, Bibliotheken" },
    "theatre": { "sk": "Divadlo", "cs": "Divadlo", "en": "Theatre", "de": "Theater" },
    "concert": { "sk": "Koncert", "cs": "Koncert", "en": "Concert", "de": "Konzert" },
    "music": { "sk": "Hudba", "cs": "Hudba", "en": "Music", "de": "Musik" },
    "cinema": { "sk": "Kino", "cs": "Kino", "en": "Cinema", "de": "Kino" },
    "film": { "sk": "Film", "cs": "Film", "en": "Film", "de": "Film" },
    "sport": { "sk": "Šport", "cs": "Sport", "en": "Sport", "de": "Sport" },
    "sports": { "sk": "Šport", "cs": "Sport", "en": "Sport", "de": "Sport" },
    "exhibition": { "sk": "Výstava", "cs": "Výstava", "en": "Exhibition", "de": "Ausstellung" },
    "festival": { "sk": "Festival", "cs": "Festival", "en": "Festival", "de": "Festival" },
    "education": { "sk": "Vzdelávanie", "cs": "Vzdělávání", "en": "Education", "de": "Bildung" },
    "children": { "sk": "Pre deti", "cs": "Pro děti", "en": "For children", "de": "Für Kinder" },
    "market": { "sk": "Trh", "cs": "Trh", "en": "Market", "de": "Markt" },
    "good": { "sk": "Dobrá", "cs": "Dobrá", "en": "Good", "de": "Gut" },
    "moderate": { "sk": "Mierna", "cs": "Mírná", "en": "Moderate", "de": "Mäßig" },
    "poor": { "sk": "Zlá", "cs": "Špatná", "en": "Poor", "de": "Schlecht" },
    "veryPoor": { "sk": "Veľmi zlá", "cs": "Velmi špatná", "en": "Very poor", "de": "Sehr schlecht" },
    "open": { "sk": "Otvorené", "cs": "Otevřeno", "en": "Open", "de": "Geöffnet" },
    "closed": { "sk": "Zatvorené", "cs": "Zavřeno", "en": "Closed", "de": "Geschlossen" },
    "working": { "sk": "V prevádzke", "cs": "V provozu", "en": "Working", "de": "In Betrieb" },
    "outOfService": { "sk": "Mimo prevádzky", "cs": "Mimo provoz", "en": "Out of service", "de": "Außer Betrieb" }
  }
}
;
// What the widget shows of an answer (T-3325): a small, safe part of Markdown read into a plain
// tree, and the answer's citations grouped into its sources. Nothing here touches the page: the
// widget builds DOM nodes from the tree with createElement and textContent alone, so no HTML the
// model writes is ever parsed. Loaded before widget.js; `node --test` reads it as a module.
"use strict";
var JCRender = (function () {
  // Bold, italic, inline code, an http(s) link, and a citation marker `[1]` or `[1, 2]`.
  var INLINE = /\*\*(.+?)\*\*|__(.+?)__|`([^`]+)`|\[([^\]\n]+)\]\((https?:\/\/[^\s()]+)\)|\[(\d+(?:\s*,\s*\d+)*)\]|\*(?=\S)(.+?)\*|(^|[^\w])_(?=\S)(.+?)_(?!\w)/g;

  function inline(text, known, linkable) {
    var out = [];
    var last = 0;
    var match;
    var re = new RegExp(INLINE.source, "g");
    while ((match = re.exec(text)) !== null) {
      // `_em_` takes the character before it, so a snake_case name stays one word.
      out.push(text.slice(last, match.index) + (match[8] || ""));
      if (match[1] !== undefined || match[2] !== undefined) {
        out.push({ tag: "strong", children: inline(match[1] !== undefined ? match[1] : match[2], known, linkable) });
      } else if (match[3] !== undefined) {
        out.push({ tag: "code", children: [match[3]] });
      } else if (match[4] !== undefined) {
        var words = inline(match[4], known, linkable);
        // A link off the allowlist is its words alone (T-4149): an address a passage planted is
        // never one click away. `![alt](url)` is a "!" and the same link or words: never an image.
        if (linkable(match[5])) out.push({ tag: "a", href: match[5], children: words });
        else out = out.concat(words);
      } else if (match[6] !== undefined) {
        var numbers = match[6].split(",").map(function (n) { return Number(n.trim()); })
          .filter(function (n) { return known(n); });
        // A marker no citation stands behind is dropped, never shown as typed.
        if (numbers.length > 0) out.push({ tag: "cite", numbers: numbers });
      } else {
        out.push({ tag: "em", children: inline(match[7] !== undefined ? match[7] : match[9], known, linkable) });
      }
      last = re.lastIndex;
    }
    out.push(text.slice(last));
    return out.filter(function (node) { return node !== ""; });
  }

  var BULLET = /^\s*[-*+]\s+(.*)$/;
  var NUMBERED = /^\s*\d{1,3}[.)]\s+(.*)$/;
  var HEADING = /^\s{0,3}#{1,6}\s+(.*?)\s*#*\s*$/;

  // The citation numbers an item carries at its own level, as one key; "" when it cites nothing.
  function citeKey(item) {
    var numbers = [];
    item.children.forEach(function (node) {
      if (node && node.tag === "cite") numbers = numbers.concat(node.numbers);
    });
    return numbers.filter(function (n, i) { return numbers.indexOf(n) === i; }).sort(function (a, b) { return a - b; }).join(",");
  }

  // An item without its markers, and without the commas and spaces that only joined them.
  function uncited(item) {
    var kept = item.children.filter(function (node) { return !(node && node.tag === "cite"); });
    var at = kept.length - 1;
    while (at >= 0 && typeof kept[at] === "string" && /^[\s,;]*$/.test(kept[at])) at -= 1;
    kept = kept.slice(0, at + 1);
    if (typeof kept[at] === "string") kept[at] = kept[at].replace(/[\s,;]+$/, "");
    item.children = kept.filter(function (node) { return node !== ""; });
  }

  /**
   * Consecutive items of a list that cite the same sources keep the markers on the last of them
   * only (T-4278): "[1],[2]" after every weekday of the opening hours was noise on a phone.
   */
  function collapseCites(list) {
    var items = list.children;
    for (var i = 0; i < items.length - 1; i += 1) {
      var key = citeKey(items[i]);
      if (key !== "" && key === citeKey(items[i + 1])) uncited(items[i]);
    }
  }

  var FENCE = /^\s{0,3}(```|~~~)/;
  var ROW = /^\s*\|.*\|\s*$/;
  var RULE = /^\s*\|?(\s*:?-+:?\s*\|)+\s*(:?-+:?\s*)?$/;

  // A table row's cells: the text between its pipes, a `\|` kept as a pipe.
  function cells(line, tag, known, linkable) {
    return line.trim().replace(/\\\|/g, "\u0000").replace(/^\|/, "").replace(/\|$/, "").split("|").map(function (cell) {
      return { tag: tag, children: inline(cell.trim().replace(/\u0000/g, "|"), known, linkable) };
    });
  }

  /**
   * The answer as blocks: paragraphs (lines joined by line breaks), bullet and numbered lists,
   * headings read as bold paragraphs, pipe tables (T-4649) and fenced code. `known(n)` says
   * whether citation n exists, `linkable(url)` whether a link to `url` stays a link (none does
   * without it).
   */
  function markdown(text, known, linkable) {
    known = known || function () { return false; };
    linkable = linkable || function () { return false; };
    var blocks = [];
    var paragraph = null;
    var list = null;
    var table = null;
    var fence = null;
    String(text || "").replace(/\r\n?/g, "\n").split("\n").forEach(function (line) {
      if (fence) {
        if (FENCE.test(line)) fence = null;
        else fence.lines.push(line);
        return;
      }
      if (FENCE.test(line)) {
        fence = { tag: "pre", children: [{ tag: "code", children: [] }], lines: [] };
        blocks.push(fence);
        paragraph = list = table = null;
        return;
      }
      if (ROW.test(line) || (table && RULE.test(line))) {
        paragraph = list = null;
        if (table && table.children.length === 1 && RULE.test(line)) {
          // The rule under the first row makes it the header.
          table.children[0].children.forEach(function (cell) { cell.tag = "th"; });
          return;
        }
        if (!table) {
          table = { tag: "table", children: [] };
          blocks.push(table);
        }
        table.children.push({ tag: "tr", children: cells(line, "td", known, linkable) });
        return;
      }
      table = null;
      var item = BULLET.exec(line);
      var number = item ? null : NUMBERED.exec(line);
      var heading = HEADING.exec(line);
      if (item || number) {
        var tag = item ? "ul" : "ol";
        if (!list || list.tag !== tag) {
          list = { tag: tag, children: [] };
          blocks.push(list);
        }
        list.children.push({ tag: "li", children: inline((item || number)[1], known, linkable) });
        paragraph = null;
      } else if (line.trim() === "") {
        paragraph = null;
        list = null;
      } else if (heading) {
        blocks.push({ tag: "p", children: [{ tag: "strong", children: inline(heading[1], known, linkable) }] });
        paragraph = null;
        list = null;
      } else if (list && /^\s{2,}\S/.test(line)) {
        // An indented line goes on with the list item above it.
        var last = list.children[list.children.length - 1];
        last.children = last.children.concat([{ tag: "br" }], inline(line.trim(), known, linkable));
      } else {
        list = null;
        if (!paragraph) {
          paragraph = { tag: "p", children: [] };
          blocks.push(paragraph);
        } else {
          paragraph.children.push({ tag: "br" });
        }
        paragraph.children = paragraph.children.concat(inline(line.trim(), known, linkable));
      }
    });
    blocks.forEach(function (block) {
      if (block.lines) {
        block.children[0].children = [block.lines.join("\n")];
        delete block.lines;
      }
      if (block.tag === "ul" || block.tag === "ol") collapseCites(block);
    });
    return blocks;
  }

  /**
   * The text so far of an answer still streaming, cut where `markdown` would show a marker the
   * model has not closed yet (T-4649): an open fence is closed, and of the unfinished last line a
   * table row or a bare `#`/`-` waits for its line, an unpaired `**`, `` ` `` or `*` is dropped,
   * a half link shows its words and a half citation nothing. What is complete is untouched, so
   * the streamed view and the final view are the same render of the same lines.
   */
  function streamable(text) {
    text = String(text || "").replace(/\r\n?/g, "\n");
    var lines = text.split("\n");
    var open = lines.filter(function (line) { return FENCE.test(line); }).length % 2 === 1;
    var last = lines.pop();
    if (open) return lines.concat([last, "```"]).join("\n");
    if (ROW.test(last) || /^\s*\|/.test(last) || /^\s{0,3}(#{1,6}|[-*+]|\d{1,3}[.)]?)?\s*$/.test(last) || /^\s{0,3}(`{1,3}|~{1,3})/.test(last)) {
      last = "";
    } else {
      last = last.replace(/\[([^\]\n]*)\]\([^)\s]*$/, "$1").replace(/\[[\d,\s]*$/, "").replace(/\[([^\]\n]*)$/, "$1");
      if ((last.match(/\*\*/g) || []).length % 2 === 1) last = last.replace(/\*\*(?!.*\*\*)/, "");
      if ((last.match(/`/g) || []).length % 2 === 1) last = last.replace(/`(?!.*`)/, "");
      var bullet = /^(\s*[-*+]\s+)?(.*)$/.exec(last);
      if ((bullet[2].replace(/\*\*/g, "").match(/\*/g) || []).length % 2 === 1) {
        last = (bullet[1] || "") + bullet[2].replace(/\*(?!.*\*)/, "");
      }
    }
    return lines.concat([last]).join("\n");
  }

  function domain(url) {
    var match = /^https?:\/\/([^/?#:]+)/i.exec(url);
    return match ? match[1].replace(/^www\./, "") : "";
  }

  /**
   * Whether `url` goes to one of `hosts` itself, `www.` aside (T-4149): never a host under one, as
   * a city's subdomains are not all the city's own pages (T-4654). A host with anything but
   * letters, digits, dots and dashes (a `user@host`, a backslash) never matches.
   */
  function linkHost(url, hosts) {
    if (machine(url)) return false;
    var match = /^https?:\/\/([^/?#]+)(?:[/?#]|$)/i.exec(String(url || ""));
    if (!match) return false;
    var host = match[1].toLowerCase();
    if (!/^[a-z0-9.-]+$/.test(host)) return false;
    host = host.replace(/^www\./, "");
    return (hosts || []).some(function (h) {
      h = String(h || "").toLowerCase().replace(/^www\./, "");
      return h !== "" && host === h;
    });
  }

  /**
   * The citations as the sources a visitor reads: one per address, the numbers that share it
   * merged, each with its title and domain. A citation of live data reads as live data and never
   * by its tool's name. `position[n]` is the 1-based place of citation n in `sources`.
   */
  function sources(citations) {
    var list = [];
    var byKey = {};
    var position = {};
    (citations || []).forEach(function (c) {
      if (!c || typeof c.n !== "number") return;
      var url = typeof c.url === "string" && /^https?:\/\//i.test(c.url) ? c.url : null;
      var live = Boolean(c.tool);
      var key = url || (live ? "live:" + (c.endpoint || "") : null);
      if (!key) return;
      if (!(key in byKey)) {
        byKey[key] = list.length;
        list.push({
          url: url,
          title: typeof c.title === "string" && c.title.trim() ? c.title.trim() : null,
          live: live,
          endpoint: typeof c.endpoint === "string" ? c.endpoint : null,
          domain: url ? domain(url) : "",
          numbers: [],
          cards: []
        });
      }
      var entry = list[byKey[key]];
      cards(c.cards).forEach(function (card) {
        var same = JSON.stringify(card);
        if (entry.cards.length < 3 && !entry.cards.some(function (k) { return JSON.stringify(k) === same; })) {
          entry.cards.push(card);
        }
      });
      if (!entry.title && typeof c.title === "string" && c.title.trim()) entry.title = c.title.trim();
      entry.numbers.push(c.n);
      position[c.n] = byKey[key] + 1;
    });
    return { list: list, position: position };
  }

  /** What a source's link says: its title, else the address without its scheme. */
  function label(source) {
    if (source.title) return source.title;
    // An uploaded file is named by its file name (T-4184).
    if (source.url && /^upload:/.test(source.url)) return source.url.replace(/^upload:[^/]*\//, "").replace(/#page=\d+$/, "");
    if (source.url) return source.url.replace(/^https?:\/\//i, "").replace(/\/$/, "");
    return source.endpoint || "";
  }

  /**
   * Whether a source is rows of a type no catalogue dataset publishes (T-4802): live data named
   * by its type with no page a person can open. Its card says so instead of linking anything.
   */
  function unlisted(source) {
    return Boolean(source && source.live && source.title && !safeUrl(source.url));
  }

  /**
   * Whether `url` is an address for programs (T-4761): a schema, an MCP or NGSI-LD path, a JSON,
   * JSON-LD or GeoJSON file, or a JSON format parameter. A person who clicks it gets raw JSON.
   */
  function machine(url) {
    var parts = /^[^?#]*/.exec(String(url || ""))[0].toLowerCase();
    var query = /\?([^#]*)/.exec(String(url || ""));
    return /\/(schema|mcp|ngsi-ld)(\/|$)|\.(json|jsonld|geojson)$/.test(parts) ||
      Boolean(query && /(^|&)(f|format)=(geo)?json(-?ld)?(&|$)/i.test(query[1]));
  }

  /**
   * An `http(s)` address a person can open, as given; anything else (a `javascript:` link, a
   * relative path, an address for programs) null.
   */
  function safeUrl(url) {
    return typeof url === "string" && /^https?:\/\/[^\s]+$/i.test(url) && !machine(url) ? url : null;
  }

  /**
   * The complete Server-Sent Events in `buffer` (T-4138): each `{name, data}` with its data parsed
   * as JSON, a malformed one skipped, and the unfinished tail returned as `rest` for the next read.
   */
  function events(buffer) {
    var out = [];
    var text = String(buffer).replace(/\r\n?/g, "\n");
    var at;
    while ((at = text.indexOf("\n\n")) >= 0) {
      var block = text.slice(0, at);
      text = text.slice(at + 2);
      var name = "";
      var data = [];
      block.split("\n").forEach(function (line) {
        if (line.indexOf("event:") === 0) name = line.slice(6).trim();
        else if (line.indexOf("data:") === 0) data.push(line.slice(5).replace(/^ /, ""));
      });
      if (!name) continue;
      try {
        out.push({ name: name, data: JSON.parse(data.join("\n")) });
      } catch (e) {
        // A malformed event is skipped and the stream goes on.
      }
    }
    return { events: out, rest: text };
  }

  /**
   * The language the chat opens in (T-4343), by the deployment's `widget.language` (`mode`): the
   * page's `?lang=` (`requested`) when the deployment offers it; with `fixed` then the first of the
   * deployment's languages; otherwise the language the visitor picked with the toggle (`chosen`),
   * then the first of the browser's `navigator.languages` (`browser`) whose primary subtag the
   * deployment offers (`cs` falls back to `sk` only when it offers no `cs`), then its first
   * language. Only languages the widget has words for (`known`) count; Slovak, else English, when
   * the deployment lists none of them.
   */
  function language(requested, offered, known, chosen, browser, mode) {
    var has = function (l) { return Object.prototype.hasOwnProperty.call(known, l); };
    var pick = function (l) { return String(l || "").split("-")[0].toLowerCase(); };
    var list = (offered || []).map(pick).filter(has);
    var offers = function (l) { return has(l) && (list.length === 0 || list.indexOf(l) >= 0); };
    var first = list.length > 0 ? list[0] : has("sk") ? "sk" : "en";
    if (offers(pick(requested))) return pick(requested);
    if (mode === "fixed") return first;
    if (offers(pick(chosen))) return pick(chosen);
    var tags = Array.isArray(browser) ? browser : [];
    for (var i = 0; i < tags.length; i += 1) {
      var tag = pick(tags[i]);
      if (list.indexOf(tag) >= 0) return tag;
      if (tag === "cs" && list.indexOf("sk") >= 0) return "sk";
    }
    return first;
  }

  /**
   * A `data` event as a table to show (T-4138, T-4140): its columns and at most 50 rows of
   * text, and its dataset, download and GeoJSON links when they are http(s); null when nothing to show.
   */
  // Headers a person reads (T-4650): the common Smart Data Models attributes and types in the
  // widget's languages; any other key is spelt out from its camelCase.
  // One label table with the CKAN publisher and theme (T-4661): jc-core's field-order.json,
  // which the server puts before this script as JC_FIELDS and `node --test` reads as a file.
  var FIELDS = typeof JC_FIELDS !== "undefined" ? JC_FIELDS : require("../../jc-core/data/field-order.json");
  var LABELS = FIELDS.labels;
  var TYPES = {
    AirQualityObserved: { sk: "Kvalita ovzdušia", cs: "Kvalita ovzduší", en: "Air quality", de: "Luftqualität" },
    WeatherObserved: { sk: "Počasie", cs: "Počasí", en: "Weather", de: "Wetter" },
    School: { sk: "Školy", cs: "Školy", en: "Schools", de: "Schulen" },
    PointOfInterest: { sk: "Miesta", cs: "Místa", en: "Places", de: "Orte" },
    OffStreetParking: { sk: "Parkoviská", cs: "Parkoviště", en: "Car parks", de: "Parkplätze" },
    WaterQualityObserved: { sk: "Kvalita vody", cs: "Kvalita vody", en: "Water quality", de: "Wasserqualität" },
    TrafficFlowObserved: { sk: "Doprava", cs: "Doprava", en: "Traffic", de: "Verkehr" },
    KeyPerformanceIndicator: { sk: "Ukazovatele", cs: "Ukazatele", en: "Indicators", de: "Kennzahlen" },
    Bridge: { sk: "Mosty", cs: "Mosty", en: "Bridges", de: "Brücken" },
    GtfsStop: { sk: "Zastávky", cs: "Zastávky", en: "Stops", de: "Haltestellen" },
    BikeHireDockingStation: { sk: "Stanice bicyklov", cs: "Stanice kol", en: "Bike stations", de: "Fahrradstationen" },
    Event: { sk: "Podujatia", cs: "Akce", en: "Events", de: "Veranstaltungen" },
    Vehicle: { sk: "Vozidlá", cs: "Vozidla", en: "Vehicles", de: "Fahrzeuge" }
  };
  var YES_NO = { sk: ["Áno", "Nie"], cs: ["Ano", "Ne"], en: ["Yes", "No"], de: ["Ja", "Nein"] };
  var OF = { sk: "z", cs: "z", en: "of", de: "von" };
  var FIRST = { sk: "prvých", cs: "prvních", en: "first", de: "die ersten" };
  // A long address in a cell reads as this word, a link (T-4682).
  var LINK = { sk: "Zdroj", cs: "Zdroj", en: "Source", de: "Quelle" };
  // The rows a table shows in the panel; the dataset holds the rest (T-4682).
  var ROWS = 20;
  // A row measured a day or more before the table's newest says so beside its time (T-4682).
  var STALE = { sk: "neaktuálne", cs: "neaktuální", en: "out of date", de: "veraltet" };
  var STALE_MS = 24 * 3600 * 1000;

  // `dateObserved` → "Date observed", `AirQualityObserved` → "Air quality observed".
  function spelt(key) {
    var words = String(key).replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").trim().toLowerCase();
    return words.charAt(0).toUpperCase() + words.slice(1);
  }

  function word(map, key, lang) {
    var entry = Object.prototype.hasOwnProperty.call(map, key) ? map[key] : null;
    return entry ? entry[lang] || entry.en : spelt(key);
  }

  /** The chart renderer the widget frames (T-4657): `https://viz.{domain}` exactly, else null. */
  function vizOrigin(value) {
    return typeof value === "string" && /^https:\/\/viz(\.[a-z0-9-]+)+$/.test(value) ? value : null;
  }

  /**
   * A spec as the visitor reads it (T-4657): a map's popup labels and a chart's axis titles in
   * `lang`, a unit in brackets kept. A copy; the renderer checks it again and sets it as text.
   */
  function vizSpec(spec, lang, labels) {
    var copy = JSON.parse(JSON.stringify(spec || {}));
    // The model's words for a column (T-4682) before the shared table's.
    var named = function (key) {
      var own = labels && typeof labels === "object" && labels[key] && typeof labels[key] === "object" ? labels[key] : null;
      return (own && (entry(own, lang) || entry(own, "en"))) || word(LABELS, key, lang);
    };
    if (Array.isArray(copy.popup)) {
      copy.popup.forEach(function (f) {
        if (f && typeof f.field === "string") f.label = named(f.field);
      });
    }
    var encoding = copy.encoding && typeof copy.encoding === "object" ? copy.encoding : {};
    Object.keys(encoding).forEach(function (channel) {
      var title = encoding[channel] && encoding[channel].title;
      if (typeof title !== "string") return;
      var parts = /^(.*?)( \([^()]*\))?$/.exec(title);
      encoding[channel].title = named(parts[1]) + (parts[2] || "");
    });
    return copy;
  }

  /** Whether a data card opens on its map (T-4657): every row of the table has a place. */
  function mapFirst(data) {
    var features = data && data.map && data.map.data && data.map.data.features;
    var rows = data && Array.isArray(data.rows) ? data.rows.length : 0;
    return Array.isArray(features) && rows > 0 && features.length >= rows;
  }

  /** A time as "pred 5 min.", "dnes 18:40" or "17. 10. 2026 18:40" in `lang`. */
  function when(stamp, lang, now) {
    var t = Date.parse(stamp);
    if (!isFinite(t)) return String(stamp);
    var minutes = Math.round((t - now) / 60000);
    var day = function (x) { var d = new Date(x); d.setHours(0, 0, 0, 0); return d.getTime(); };
    var clock = new Intl.DateTimeFormat(lang, { hour: "2-digit", minute: "2-digit" }).format(t);
    if (minutes <= 0 && minutes > -60) return new Intl.RelativeTimeFormat(lang, { numeric: "auto", style: "short" }).format(minutes, "minute");
    var days = Math.round((day(t) - day(now)) / 86400000);
    if (days === 0 || days === -1) return new Intl.RelativeTimeFormat(lang, { numeric: "auto" }).format(days, "day") + " " + clock;
    return new Intl.DateTimeFormat(lang, { day: "numeric", month: "numeric", year: "numeric" }).format(t) + " " + clock;
  }

  /**
   * A `data` event as the table the widget shows (T-4650, T-4682): the caption is the type's name
   * and the row count, each header its label and unit, each cell text in `lang`. Numbers take the
   * locale's separator and at most three decimals, but a year or a code (a column named for one)
   * is never grouped: 1953, not "1 953". Times read as `when` says, booleans as words, and an enum
   * as the words the table's `enums` give it. An http(s) address becomes `{text, href}`, a short
   * link. An empty cell stays empty. The entity's URN is never a column: an unnamed row is called
   * by the URN's last segment, and a column of references is left out. At most 20 rows; the
   * caption says how many more the dataset holds. Everything is text; nothing here becomes HTML.
   */
  function table(data, lang, now) {
    if (!data || !Array.isArray(data.columns) || data.columns.length === 0) return null;
    lang = YES_NO[lang] ? lang : "en";
    now = typeof now === "number" ? now : Date.now();
    var numbers = new Intl.NumberFormat(lang, { maximumFractionDigits: 3 });
    var units = Array.isArray(data.units) ? data.units : [];
    // A measured value to one decimal (11,5 µg/m³, never 11,506), a plain number to two (T-4711).
    var measured = new Intl.NumberFormat(lang, { maximumFractionDigits: 1 });
    var plainNumber = new Intl.NumberFormat(lang, { maximumFractionDigits: 2 });
    var small = new Intl.NumberFormat(lang, { maximumSignificantDigits: 2 });
    var modelled = Array.isArray(data.labels) ? data.labels : [];
    var enums = Array.isArray(data.enums) ? data.enums : [];
    var raw = (Array.isArray(data.rows) ? data.rows : []).filter(Array.isArray).slice(0, ROWS);
    var urn = function (v) { return typeof v === "string" && /^urn:/i.test(v); };
    // The columns shown: an `id` column becomes the row's name, a column of references goes.
    var keep = [];
    data.columns.forEach(function (c, i) {
      var filled = raw.filter(function (row) { return row[i] !== null && row[i] !== undefined && row[i] !== ""; });
      if (c !== "id" && filled.length > 0 && filled.every(function (row) { return urn(row[i]); })) return;
      keep.push(i);
    });
    var plain = function (column) { return /year|code$/i.test(String(column)) || /Id$|^id$/.test(String(column)); };
    var cell = function (v, column, i) {
      if (v === null || v === undefined || v === "") return "";
      // A row's own name per language (T-5330) and an enum's words come before the URN tail.
      var words = enums[i] && typeof enums[i] === "object" && (typeof v === "string" || typeof v === "number") && Object.prototype.hasOwnProperty.call(enums[i], v) ? enums[i][v] : null;
      if (words && typeof words === "object") return entry(words, lang) || entry(words, "en") || entry(words, Object.keys(words)[0]) || String(v);
      if (column === "id" && urn(v)) return String(v).split(":").pop();
      if (typeof v === "number") {
        if (!isFinite(v)) return "";
        if (plain(column) && Number.isInteger(v)) return String(v);
        // Below 1, two significant digits: 0,04 mg/m³ never reads as 0 (integrator, T-4711).
        if (v !== 0 && Math.abs(v) < 1) return small.format(v);
        return (typeof units[i] === "string" && units[i] ? measured : plainNumber).format(v);
      }
      if (typeof v === "boolean") return YES_NO[lang][v ? 0 : 1];
      if (typeof v === "object") return JSON.stringify(v);
      if (safeUrl(v)) return { text: LINK[lang], href: v };
      if (column === "observedAt" || (TIME.test(v) && v.length > 10)) return when(v, lang, now);
      return String(v);
    };
    var at = data.columns.indexOf("observedAt");
    var times = raw.map(function (row) { return at < 0 || typeof row[at] !== "string" ? NaN : Date.parse(row[at]); });
    var newest = Math.max.apply(null, times.filter(isFinite).concat([-Infinity]));
    var rows = raw.map(function (row, r) {
      return keep.map(function (i) {
        var shown = cell(row[i], data.columns[i], i);
        return i === at && newest - times[r] >= STALE_MS ? shown + " · " + STALE[lang] : shown;
      });
    });
    var numeric = keep.map(function (i) {
      return raw.length > 0 && raw.every(function (row) {
        return row[i] === null || row[i] === undefined || typeof row[i] === "number";
      });
    });
    // The data model's labels where the table carries them (T-4672), else the shared table's.
    var columns = keep.map(function (i) {
      var c = data.columns[i];
      var own = modelled[i] && typeof modelled[i] === "object" ? entry(modelled[i], lang) || entry(modelled[i], "en") : "";
      var label = own || word(LABELS, c === "id" ? "name" : String(c), lang);
      return typeof units[i] === "string" && units[i] ? label + " (" + units[i] + ")" : label;
    });
    var named = typeof data.type === "string" && data.type ? word(TYPES, data.type, lang) : "";
    var caption = "";
    if (named && typeof data.total === "number" && data.total > rows.length) {
      caption = named + " — " + FIRST[lang] + " " + numbers.format(rows.length) + " " + OF[lang] + " " + numbers.format(data.total);
    } else if (named) {
      caption = named + " (" + numbers.format(rows.length) + ")";
    }
    return {
      caption: caption,
      columns: columns,
      numeric: numeric,
      rows: rows,
      dataset: safeUrl(data.dataset),
      download: safeUrl(data.download),
      // An address for programs is neither a link nor text: the dataset page names it (T-4799).
      geojson: safeUrl(data.geojson)
    };
  }

  /**
   * The greeting the empty chat shows (T-4199): the deployment's own, which its author writes in
   * its first language, when the chat speaks that language or the deployment names none; else the
   * widget's greeting in the visitor's language.
   */
  function greeting(own, languages, lang, fallback) {
    // One per language (T-4365): this language's, else the widget's own.
    if (own && typeof own === "object") return Array.isArray(own) ? fallback : entry(own, lang) || fallback;
    if (!own) return "";
    return !Array.isArray(languages) || languages.length === 0 || languages[0] === lang ? own : fallback;
  }

  function entry(map, key) {
    var text = Object.prototype.hasOwnProperty.call(map, key) ? map[key] : null;
    return typeof text === "string" && text.trim() ? text.trim() : "";
  }

  /**
   * The header's title (T-4365): the one string, or of a map per language this language's, else
   * the first of `languages`'s, else any; "" when there is none.
   */
  function title(own, languages, lang) {
    if (typeof own === "string") return own.trim();
    if (!own || typeof own !== "object" || Array.isArray(own)) return "";
    var first = Array.isArray(languages) ? languages[0] : undefined;
    var keys = [lang, first].concat(Object.keys(own).sort());
    for (var i = 0; i < keys.length; i++) {
      if (entry(own, keys[i])) return entry(own, keys[i]);
    }
    return "";
  }

  var TIME = /^\d{4}-\d{2}-\d{2}(?:[T ][\d:.]+(?:Z|[+-]\d{2}:?\d{2})?)?$/;

  /**
   * A `data` event as a small chart (T-4198): when exactly one column holds numbers and another
   * names them, its points: a line over time when every name is a date, else bars, at most 50,
   * a value that is no finite number skipped. Null in any other case, and under two points.
   */
  function chart(data) {
    if (!data || !Array.isArray(data.columns) || !Array.isArray(data.rows)) return null;
    var rows = data.rows.filter(Array.isArray);
    var finite = function (v) { return typeof v === "number" && isFinite(v); };
    var numeric = data.columns.map(function (_, i) {
      var numbers = 0, words = 0;
      rows.forEach(function (row) {
        if (finite(row[i])) numbers += 1;
        else if (typeof row[i] === "string" && row[i].trim()) words += 1;
      });
      return numbers > 0 && numbers > words;
    });
    var values = numeric.reduce(function (found, yes, i) { return yes ? found.concat([i]) : found; }, []);
    if (values.length !== 1) return null;
    var named = data.columns.map(function (_, i) { return i; }).filter(function (i) { return !numeric[i]; });
    var isTime = function (i) {
      return rows.every(function (row) { return typeof row[i] === "string" && TIME.test(row[i]) && isFinite(Date.parse(row[i])); });
    };
    // T-4711, owner 04:46Z: no chart unless every check passes; a failed check is no chart, never
    // a repaired one. A line needs time with 3 distinct moments; one moment is categories (bars).
    var time = named.filter(isTime)[0];
    if (time !== undefined) {
      var moments = {};
      rows.forEach(function (row) { moments[Date.parse(row[time])] = true; });
      var distinct = Object.keys(moments).length;
      if (distinct === 2) return null;
      if (distinct < 3) {
        named = named.filter(function (i) { return i !== time; });
        time = undefined;
      }
    }
    var by = time !== undefined ? time : named[0];
    if (by === undefined) return null;
    // Nothing truncated: a chart of the first rows of more would mislead.
    if (typeof data.total === "number" && data.total > rows.length) return null;
    var v = values[0];
    var points = rows.filter(function (row) { return finite(row[v]); }).slice(0, 50).map(function (row) {
      var name = row[by] === null || row[by] === undefined ? "" : String(row[by]);
      return { label: name.slice(0, 40), value: row[v] };
    });
    if (time !== undefined) points.sort(function (a, b) { return Date.parse(a.label) - Date.parse(b.label); });
    if (points.length < (time !== undefined ? 3 : 2)) return null;
    if (time === undefined) {
      // Bars name each category once, by a person's name, never a slug or an id.
      var labels = points.map(function (p) { return p.label; });
      var SLUG = /^[a-z0-9]+(-[a-z0-9]+){2,}$|^urn:/i;
      if (labels.some(function (l) { return !l.trim() || SLUG.test(l); })) return null;
      if (labels.some(function (l, i) { return labels.indexOf(l) !== i; })) return null;
      // No total drawn beside its own parts: one bar that is the sum of the others.
      var sum = points.reduce(function (a, p) { return a + p.value; }, 0);
      if (points.length > 2 && points.some(function (p) { return Math.abs(p.value * 2 - sum) <= Math.abs(p.value) * 0.005; })) return null;
    }
    return { kind: time !== undefined ? "line" : "bar", value: String(data.columns[v]), by: String(data.columns[by]), points: points };
  }

  var EMAIL = /^[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}$/;

  /** A phone as a `tel:` link: 9 to 15 digits, a leading `+` and spaces, `/`, `-` aside; else null. */
  function tel(phone) {
    var compact = String(phone || "").replace(/[\s\/-]/g, "");
    return /^\+?\d{9,15}$/.test(compact) ? "tel:" + compact : null;
  }

  /** An e-mail address as a `mailto:` link, null unless it is one address and nothing else. */
  function mailto(address) {
    return EMAIL.test(String(address || "")) ? "mailto:" + address : null;
  }

  function text(value, max) {
    return typeof value === "string" ? value.replace(/\s+/g, " ").trim().slice(0, max) : "";
  }

  /**
   * A citation's cards (T-4155) as the chat shows them: each field text, each link checked
   * (`tel:`, `mailto:`, an OpenStreetMap search), an empty field left out and an empty card dropped.
   */
  function cards(raw) {
    return (Array.isArray(raw) ? raw : []).map(function (card) {
      if (!card || typeof card !== "object") return null;
      var list = function (values, link) {
        return (Array.isArray(values) ? values : []).map(function (v) { return text(v, 300); })
          .filter(Boolean).slice(0, 5).map(function (v) { return { text: v, href: link(v) }; });
      };
      if (card.kind === "contact") {
        var phones = list(card.phones, tel);
        var emails = list(card.emails, mailto);
        if (phones.length + emails.length === 0) return null;
        return { kind: "contact", name: text(card.name, 300), phones: phones, emails: emails };
      }
      if (card.kind === "place") {
        var address = text(card.address, 300);
        return address ? { kind: "place", address: address,
          href: "https://www.openstreetmap.org/search?query=" + encodeURIComponent(address) } : null;
      }
      if (card.kind === "procedure") {
        var steps = (Array.isArray(card.steps) ? card.steps : []).map(function (v) { return text(v, 300); })
          .filter(Boolean).slice(0, 10);
        return steps.length > 0 ? { kind: "procedure", steps: steps } : null;
      }
      return null;
    }).filter(Boolean);
  }

  var STAGES = {
    sk: { understanding: "Rozmýšľam nad otázkou…", searching: "Hľadám v zdrojoch", reading: "Čítam živé údaje",
      history: "Čítam históriu údajov", schema: "Čítam schému údajov", answering: "Pripravujem odpoveď…",
      steps: "Postup" },
    en: { understanding: "Reading your question…", searching: "Searching the sources", reading: "Reading live data",
      history: "Reading the data's history", schema: "Reading the data's schema", answering: "Preparing the answer…",
      steps: "Steps" },
    cs: { understanding: "Přemýšlím nad otázkou…", searching: "Hledám ve zdrojích", reading: "Čtu živá data",
      history: "Čtu historii dat", schema: "Čtu schéma dat", answering: "Připravuji odpověď…",
      steps: "Postup" }
  };
  // The stages that are one step of the answer's work, listed under it (T-4294).
  var STEPS = ["searching", "reading", "history", "schema"];
  // The `done` event's `stagesMs`, in the order they run, by name (T-4729).
  var TIMED = ["route", "search", "judge", "answer"];
  var TIMED_WORDS = {
    sk: { route: "Výber zdroja", search: "Vyhľadávanie v zdrojoch", judge: "Kontrola nájdeného", answer: "Odpoveď" },
    en: { route: "Choosing the sources", search: "Searching the sources", judge: "Checking what was found", answer: "Answer" },
    cs: { route: "Výběr zdroje", search: "Vyhledávání ve zdrojích", judge: "Kontrola nalezeného", answer: "Odpověď" }
  };

  /**
   * The progress line of a `stage` event (T-4155): fixed words per step, a search or a read
   * naming its topic (`Hľadám v zdrojoch: „…“…`); null for a step the chat does not know.
   */
  function stage(data, lang) {
    var words = STAGES[lang] || STAGES.en;
    var name = data && data.stage;
    if (name === "steps" || !Object.prototype.hasOwnProperty.call(words, name)) return null;
    var topic = text(data.topic, 60);
    // A search names the visitor's own words; a read names its data in words or nothing, never an
    // Endpoint slug or a type name (T-4682).
    if (name !== "searching") topic = Object.prototype.hasOwnProperty.call(TYPES, topic) ? word(TYPES, topic, lang) : "";
    if (STEPS.indexOf(name) >= 0) {
      return words[name] + (topic ? ": „" + topic + "“…" : "…");
    }
    return words[name];
  }


  // The words of a read's step (T-4801); a date bound of its `q` as words (T-5343).
  var READ_WORDS = {
    sk: { read: "Hľadal som v", all: "všetky záznamy", from: "od", until: "do" },
    en: { read: "Searched", all: "all records", from: "from", until: "until" },
    cs: { read: "Hledal jsem v", all: "všechny záznamy", from: "od", until: "do" },
    de: { read: "Gesucht in", all: "alle Einträge", from: "ab", until: "bis" }
  };
  var OPS = { "==": "=", "!=": "≠", ">=": "≥", "<=": "≤", ">": ">", "<": "<", "~=": "~", "!~=": "≁" };
  var CLAUSE = /^([A-Za-z_][\w.\[\]]*)\s*(==|!=|>=|<=|!~=|~=|>|<)\s*(.+)$/;
  var DAY = /^(\d{4}-\d{2}-\d{2})(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:?\d{2})?)?$/;

  /**
   * A read's `q` in words (T-5343): `endDate>="2026-10-20T00:00:00Z"` → `od 20. 10. 2026`,
   * `yearBuilt<1900` → `Year built < 1900`; a `q` with `|` or parentheses, or a clause this does
   * not know, stays as written.
   */
  function query(q, lang) {
    var words = READ_WORDS[lang] || READ_WORDS.en;
    if (/[|()]/.test(q)) return q;
    var said = q.split(";").map(function (clause) {
      var m = CLAUSE.exec(clause.trim());
      if (!m) return null;
      var value = m[3].trim().replace(/^"(.*)"$/, "$1");
      var day = DAY.exec(value);
      var at = day ? Date.parse(day[2] ? value : day[1] + "T00:00:00Z") : NaN;
      if (isFinite(at)) {
        // Midnight UTC is a day; any other instant also says its local time.
        var midnight = !day[2] || new Date(at).getUTCHours() + new Date(at).getUTCMinutes() + new Date(at).getUTCSeconds() === 0;
        var shown = new Intl.DateTimeFormat(lang, midnight
          ? { day: "numeric", month: "numeric", year: "numeric", timeZone: "UTC" }
          : { day: "numeric", month: "numeric", year: "numeric", hour: "2-digit", minute: "2-digit" }).format(at);
        if (m[2] === ">=" || m[2] === ">") return words.from + " " + shown;
        if (m[2] === "<=" || m[2] === "<") return words.until + " " + shown;
        value = shown;
      }
      return word(LABELS, m[1], lang) + " " + OPS[m[2]] + " " + value;
    });
    return said.every(Boolean) ? said.join(", ") : q;
  }

  /**
   * A `step` event (T-4801) as the steps list reads it: the source's title, its title per
   * language (T-5343), the type, the `q`, the count the source returned and the dataset page,
   * each checked; null when it names neither a source nor a type.
   */
  function read(data) {
    if (!data || typeof data !== "object") return null;
    var count = typeof data.count === "number" && isFinite(data.count) && data.count >= 0 ? Math.floor(data.count) : null;
    var titles = {};
    if (data.titles && typeof data.titles === "object" && !Array.isArray(data.titles)) {
      Object.keys(data.titles).forEach(function (l) {
        var t = text(data.titles[l], 80);
        if (/^[a-z]{2,3}$/.test(l) && t) titles[l] = t;
      });
    }
    var out = { source: text(data.source, 80), titles: titles, type: text(data.type, 80), q: text(data.q, 200), count: count, url: safeUrl(data.url) };
    return out.source || out.type ? out : null;
  }

  /**
   * A read's line: `Hľadal som v: Podujatia — Banská Bystrica · od 20. 10. 2026 → 55`; the source
   * in the visitor's language, the type in words unless the title already says it (T-5343).
   */
  function readLine(r, lang) {
    var words = READ_WORDS[lang] || READ_WORDS.en;
    var source = entry(r.titles, lang) || r.source;
    var kind = r.type ? word(TYPES, r.type, lang) : "";
    var says = function (w) { return source.toLowerCase().indexOf(w.toLowerCase()) >= 0; };
    if (source && kind && [kind, word(TYPES, r.type, "en"), r.type].some(says)) kind = "";
    var line = words.read + ": " + [source, kind].filter(Boolean).join(" / ");
    if (r.count === null) return line;
    var n = new Intl.NumberFormat(YES_NO[lang] ? lang : "en").format(r.count);
    return line + " · " + (r.q ? query(r.q, lang) : words.all) + " → " + n;
  }

  /**
   * The reads worth a line (T-5343): one that says what another already said is dropped, and so is
   * one without a count of a source and type another read counted.
   */
  function distinct(reads, lang) {
    var lines = [];
    return reads.filter(function (r) {
      var line = readLine(r, lang);
      var counted = r.count === null && reads.some(function (o) {
        return o.count !== null && o.source === r.source && o.type === r.type;
      });
      if (counted || lines.indexOf(line) >= 0) return false;
      lines.push(line);
      return true;
    });
  }

  /**
   * Whether an answer shows a `data` event's table (T-5343): a second read of the same Endpoint
   * and type would repeat the first one's heading, so only the first is shown. `seen` keeps the
   * shown ones.
   */
  function firstTable(seen, data) {
    if (!data || typeof data !== "object" || typeof data.type !== "string" || !data.type) return true;
    var key = String(data.endpoint || "") + "\n" + data.type;
    if (seen.indexOf(key) >= 0) return false;
    seen.push(key);
    return true;
  }

  /**
   * The steps an answer took, for the collapsed list under it (T-4294): with the `step` events
   * of its reads, each read by its source, query and count, with the dataset page in `links`
   * (T-4801); else with the `done` event's `stagesMs`, each stage that ran by its name and how
   * long it took (T-4729); without them, the line of each stage that did work, in order; null for
   * fewer than two stages, which need no list.
   */
  function steps(stages, lang, timed, reads) {
    var checked = distinct((Array.isArray(reads) ? reads : []).map(read).filter(Boolean), lang);
    if (checked.length > 0) {
      return { summary: (STAGES[lang] || STAGES.en).steps + " (" + checked.length + ")",
        lines: checked.map(function (r) { return readLine(r, lang); }),
        links: checked.map(function (r) { return r.url; }) };
    }
    var seconds = new Intl.NumberFormat(YES_NO[lang] ? lang : "en", { maximumFractionDigits: 1 });
    var words = TIMED_WORDS[lang] || TIMED_WORDS.en;
    var named = TIMED.filter(function (name) {
      var ms = timed && typeof timed === "object" ? timed[name] : undefined;
      return typeof ms === "number" && isFinite(ms) && ms > 0;
    }).map(function (name) { return words[name] + " " + seconds.format(timed[name] / 1000) + " s"; });
    if (named.length >= 2) {
      return { summary: (STAGES[lang] || STAGES.en).steps + " (" + named.length + ")", lines: named };
    }
    var lines = (Array.isArray(stages) ? stages : [])
      .filter(function (data) { return data && STEPS.indexOf(data.stage) >= 0; })
      .map(function (data) {
        var line = stage(data, lang);
        // Each step with when it began, in seconds after the question (T-4682).
        var ms = data.ms;
        if (!line || typeof ms !== "number" || !isFinite(ms) || ms < 0) return line;
        return line + " (" + new Intl.NumberFormat(YES_NO[lang] ? lang : "en", { maximumFractionDigits: 1 }).format(ms / 1000) + " s)";
      })
      .filter(Boolean);
    if (lines.length < 2) return null;
    return { summary: (STAGES[lang] || STAGES.en).steps + " (" + lines.length + ")", lines: lines };
  }


  /** A `followups` event's questions (T-4174): strings, trimmed, 1 to 120 characters, at most 3. */
  function followups(data) {
    var out = [];
    ((data && Array.isArray(data.questions)) ? data.questions : []).forEach(function (q) {
      if (typeof q !== "string") return;
      q = q.replace(/\s+/g, " ").trim();
      if (q && q.length <= 120 && out.indexOf(q) < 0 && out.length < 3) out.push(q);
    });
    return out;
  }

  // What the panel keeps in its sessionStorage (T-4174): the conversation's id, how many questions
  // it asked, and the visible turns, at most MAX_KEPT, each question and answer with its citations.
  var MAX_KEPT = 20;
  var UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

  /** The kept conversation read back from `raw`, or null when it is missing or not of its shape. */
  function restore(raw) {
    var kept;
    try { kept = JSON.parse(raw); } catch (e) { return null; }
    if (!kept || typeof kept !== "object" || typeof kept.conversation !== "string" ||
        !UUID.test(kept.conversation) || !Array.isArray(kept.turns)) return null;
    var turns = kept.turns.filter(function (t) {
      return t && typeof t.question === "string" && typeof t.text === "string" &&
        Number.isInteger(t.turn) && t.turn >= 1 && Array.isArray(t.citations);
    }).slice(-MAX_KEPT).map(function (t) {
      var turn = { question: t.question.slice(0, 4000), text: t.text.slice(0, 4000), turn: t.turn, citations: t.citations };
      var askedAt = typeof t.askedAt === "string" && stamp(t.askedAt);
      var at = typeof t.at === "string" && stamp(t.at);
      if (askedAt) turn.askedAt = askedAt.datetime;
      if (at) turn.at = at.datetime;
      var opened = t.opened && dataset([{ url: t.opened.url, title: t.opened.title }]);
      if (opened) turn.opened = opened;
      var more = followups({ questions: t.followups });
      if (more.length > 0) turn.followups = more;
      return turn;
    });
    var asked = Number.isInteger(kept.asked) && kept.asked >= 0 ? kept.asked : 0;
    turns.forEach(function (t) { if (t.turn > asked) asked = t.turn; });
    return { conversation: kept.conversation, asked: asked, turns: turns };
  }

  /**
   * A message's time (T-4339): `text` HH:MM, a 24-hour clock in every language as the answer
   * writes it, in the visitor's own time zone, `datetime` and `title`
   * the UTC minute; null when `at` is no moment.
   */
  function stamp(at, locale) {
    var d = new Date(at);
    if (at === null || at === undefined || at === "" || isNaN(d.getTime())) return null;
    var utc = d.toISOString().slice(0, 16);
    var text;
    try {
      text = d.toLocaleTimeString(locale || undefined, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
    } catch (e) {
      text = ("0" + d.getHours()).slice(-2) + ":" + ("0" + d.getMinutes()).slice(-2);
    }
    return { datetime: utc + "Z", text: text, title: utc.replace("T", " ") + " UTC" };
  }

  /** `kept` with one more answered turn, the oldest dropped past MAX_KEPT. */
  function remember(kept, conversation, asked, entry) {
    var turns = (kept && kept.conversation === conversation ? kept.turns : []).concat([entry]);
    return { conversation: conversation, asked: asked, turns: turns.slice(-MAX_KEPT) };
  }

  // The copy the visitor chose to keep in this browser (T-4670): at most 90 days after its last
  // change, then gone.
  var REMEMBER_MS = 90 * 24 * 3600 * 1000;

  /** What `localStorage` holds for a remembered conversation, stamped `now`; `kept` may be null. */
  function lasting(kept, now) {
    return JSON.stringify({ savedAt: now.toISOString(), kept: kept || null });
  }

  /**
   * The remembered copy read back from `raw`: null when the visitor never switched it on, the
   * value is not of its shape, or it is older than 90 days (the caller deletes it then); else
   * `{ kept }`, the conversation or null when there was none yet.
   */
  function recalled(raw, now) {
    var held;
    try { held = JSON.parse(raw); } catch (e) { return null; }
    if (!held || typeof held !== "object" || typeof held.savedAt !== "string") return null;
    var at = new Date(held.savedAt).getTime();
    if (isNaN(at) || at > now.getTime() + 60000 || now.getTime() - at > REMEMBER_MS) return null;
    return { kept: held.kept ? restore(JSON.stringify(held.kept)) : null };
  }

  /**
   * The paragraphs of the privacy dialog (API/05 §1.6, T-4670) in `words`: the deployment's
   * `disclaimer`, else the widget's own; what is kept for how many `days` (none when 0); and,
   * only when the deployment names a contact, whom to write (`mail`) with the page of its privacy
   * documents (`href`, https only).
   */
  function privacyLines(words, days, privacy, disclaimer) {
    var own = typeof disclaimer === "string" ? disclaimer.trim() : "";
    var lines = [{ text: own || words.aboutAi }];
    var n = Number.isInteger(days) && days > 0 ? days : 0;
    lines.push({ heading: words.keptHeading, text: n > 0 ? words.keptDays.replace("{days}", String(n)) : words.keptNone });
    var contact = privacy && typeof privacy.contact === "string" ? privacy.contact.trim() : "";
    if (!contact || !mailto(contact)) return lines;
    lines.push({ heading: words.askHeading, text: words.askData, mail: contact });
    var docs = safeUrl(privacy.documentsUrl);
    if (docs && /^https:/i.test(docs)) lines.push({ text: words.documents, href: docs });
    return lines;
  }

  /**
   * The questions an empty chat offers (T-4253): the deployment's own in the language the chat
   * speaks, else the widget's own, never another language's; at most four distinct ones.
   */
  function suggestions(own, lang, fallback) {
    var list = own && Array.isArray(own[lang]) && own[lang].length > 0 ? own[lang] : fallback || [];
    var seen = [];
    list.forEach(function (q) {
      var text = typeof q === "string" ? q.trim() : "";
      if (text && seen.indexOf(text) < 0 && seen.length < 4) seen.push(text);
    });
    return seen;
  }

  // The service's error titles (API/05 §1.6), English and stable, to the widget's own words.
  var TROUBLE = {
    "Conversation Budget Spent": "full", "Model Unavailable": "unreachable",
    "No Answer": "unanswered", "Answer Took Too Long": "slow", "Declined": "declined",
    "Daily Limit Reached": "limit", "Monthly Limit Reached": "limit", "Visitor Limit Reached": "limit"
  };

  /**
   * The widget's own word for a failed answer (T-4253), so the visitor reads it in the chat's
   * language and never an English detail: by the service's title, else by the status, a lost
   * connection (no status) as offline.
   */
  function trouble(status, title) {
    if (Object.prototype.hasOwnProperty.call(TROUBLE, title)) return TROUBLE[title];
    if (status === 429) return "busy";
    if (status === 503) return "paused";
    if (!status) return "offline";
    return "failed";
  }

  /**
   * Whether asking the same again can help: not once the conversation's share is spent, and not
   * past a limit before it resets (T-4671).
   */
  function retryable(word) {
    return word !== "full" && word !== "declined" && word !== "limit";
  }

  /**
   * The line a failed answer shows: the service's own sentence for a limit, which names when it
   * resets, while the chat reads the service's language (Slovak); the widget's word otherwise.
   */
  function said(word, detail, lang, words) {
    return word === "limit" && lang === "sk" && typeof detail === "string" && detail.trim()
      ? detail.trim()
      : words[word];
  }

  /**
   * What Retry sends again (T-4253): the last question and the history without it, which the
   * failed request already carried; null when the history does not end on a question.
   */
  function retry(history) {
    var last = Array.isArray(history) ? history[history.length - 1] : null;
    if (!last || last.role !== "user" || typeof last.text !== "string" || !last.text.trim()) return null;
    return { question: last.text, history: history.slice(0, -1) };
  }

  // The count's noun per plural category; which category a number takes is Intl.PluralRules' (T-4364).
  var SOURCE_FORMS = {
    sk: { one: "zdroj", few: "zdroje", other: "zdrojov" },
    cs: { one: "zdroj", few: "zdroje", other: "zdrojů" },
    en: { one: "source", other: "sources" }
  };

  /** The footer's count (T-4364): `1 zdroj`, `2 zdroje`, `5 zdrojov`; English for an unknown language. */
  function sourceCount(n, lang) {
    var forms = SOURCE_FORMS[lang] ? SOURCE_FORMS[lang] : SOURCE_FORMS.en;
    var category;
    try {
      category = new Intl.PluralRules(SOURCE_FORMS[lang] ? lang : "en").select(n);
    } catch (e) {
      category = n === 1 ? "one" : "other";
    }
    return n + " " + (forms[category] || forms.other);
  }

  /**
   * The footer's source kinds (T-4740): "page" when the answer cites a web page or a document,
   * "table" when it cites a dataset or live data, each once, the page first.
   */
  function badges(list) {
    var kinds = {};
    (list || []).forEach(function (source) {
      kinds[source.live || (source.url && DATASET.test(source.url.split(/[?#]/)[0])) ? "table" : "page"] = true;
    });
    return ["page", "table"].filter(function (kind) { return kinds[kind]; });
  }

  /** The language this browser chose before, when the deployment still offers it; else null. */
  function chosen(stored, offered) {
    return typeof stored === "string" && (offered || []).indexOf(stored) >= 0 ? stored : null;
  }

  /**
   * Whether the panel asks for the language first (T-4283 item 13), only with `widget.language:
   * ask` (T-4343): on a first open where there
   * is a choice to make, never on the full page that names `?lang=`, never over a conversation
   * already open or a paused deployment.
   */
  function asksLanguage(at) {
    if (!at || at.mode !== "ask" || at.stored || at.kept || at.enabled === false) return false;
    if ((at.offered || []).length < 2) return false;
    return !(at.page && at.requested);
  }

  /** Slovak first, then the deployment's order: the chooser's and the toggle's buttons. */
  function ordered(offered) {
    var list = (offered || []).slice();
    var sk = list.indexOf("sk");
    if (sk > 0) list.unshift(list.splice(sk, 1)[0]);
    return list;
  }


  /**
   * The origins the chat's close message goes to, never `*` (T-4654): the framing page's own,
   * where the browser records it (`ancestorOrigins`), else each origin the deployment may be
   * framed by (its `frame-ancestors`); a target that is not the parent drops the message.
   * Anything but an http(s) origin is left out.
   */
  // A catalogue dataset page (T-4685): https (http on this machine alone), a path
  // /dataset/{name}, nothing after it.
  var DATASET = /^(?:https:\/\/[a-z0-9.-]+|http:\/\/(?:localhost|127\.0\.0\.1))(?::\d{1,5})?\/dataset\/[a-z0-9_-]{2,100}\/?$/i;

  /**
   * The one dataset an answer is about (T-4685, API/05 §1.6.1): `{ url, title }` when the
   * citations link exactly one catalogue dataset page and the first citation is it; null for a
   * broad answer that links several, or none.
   */
  function dataset(citations) {
    var pages = (Array.isArray(citations) ? citations : []).filter(function (c) {
      return c && typeof c.url === "string" && DATASET.test(c.url.split(/[?#]/)[0]);
    }).map(function (c) {
      return { url: c.url.split(/[?#]/)[0].replace(/\/$/, ""), title: typeof c.title === "string" ? c.title.slice(0, 200) : "" };
    });
    if (pages.length === 0 || !citations[0] || typeof citations[0].url !== "string") return null;
    var first = pages[0];
    var one = pages.every(function (p) { return p.url === first.url; });
    return one && citations[0].url.split(/[?#]/)[0].replace(/\/$/, "") === first.url ? first : null;
  }

  /**
   * The panel's view kept across a page change (T-4685) read back from `raw`: the draft, its
   * caret, the scroll, whether the input had focus and an answer still streaming; null when it
   * is not of that shape.
   */
  function view(raw) {
    var v;
    try { v = JSON.parse(raw); } catch (e) { return null; }
    if (!v || typeof v !== "object") return null;
    var draft = typeof v.draft === "string" ? v.draft.slice(0, 4000) : "";
    var out = {
      draft: draft,
      caret: Number.isInteger(v.caret) ? Math.max(0, Math.min(v.caret, draft.length)) : draft.length,
      scroll: typeof v.scroll === "number" && isFinite(v.scroll) && v.scroll >= 0 ? v.scroll : null,
      focused: v.focused === true,
      streaming: null
    };
    var s = v.streaming;
    if (s && typeof s === "object" && typeof s.turn === "string" && UUID.test(s.turn) &&
        typeof s.conversation === "string" && UUID.test(s.conversation) &&
        Number.isInteger(s.number) && s.number >= 1 && typeof s.question === "string") {
      out.streaming = { turn: s.turn, conversation: s.conversation, number: s.number,
        question: s.question.slice(0, 4000), text: typeof s.text === "string" ? s.text.slice(0, 64000) : "" };
      var askedAt = typeof s.askedAt === "string" && stamp(s.askedAt);
      if (askedAt) out.streaming.askedAt = askedAt.datetime;
    }
    return out;
  }

  function closeTargets(ancestors, frameOrigins) {
    var origin = function (value) {
      var match = /^(https?:\/\/[a-z0-9.-]+(?::\d{1,5})?)\/?$/i.exec(String(value || ""));
      return match ? match[1].toLowerCase() : null;
    };
    var listed = ancestors && ancestors.length ? [ancestors[0]] : frameOrigins || [];
    return Array.prototype.map.call(listed, origin).filter(Boolean);
  }

  // "Add to your AI assistant" (T-4694): the steps each client documents, as they read on
  // 2026-10-19 (Claude's help centre, OpenAI's, VS Code's and Cursor's docs). No client publishes
  // a link that adds a connector in one click, so none is shown.
  var CONNECT = {
    sk: {
      open: "Pridať do svojho AI asistenta",
      title: "Pridať do svojho AI asistenta",
      lead: "Tie isté údaje a dokumenty, z ktorých odpovedá tento asistent, môžete čítať aj vo vlastnom AI asistentovi. Konektor len číta, netreba účet ani kľúč.",
      address: "Adresa konektora", copy: "Kopírovať adresu", copied: "Adresa je skopírovaná.", selected: "Adresa je označená: stlačte Ctrl+C.", tryIt: "Skúste sa tam opýtať:",
      claude: ["V Claude otvorte Customize → Connectors.", "Zvoľte + Add a potom Add custom connector.", "Zadajte názov, vložte adresu a zvoľte Continue.", "Pri prihlásení zvoľte No sign in a potom Add.", "V rozhovore ho zapnete cez + → Connectors. Plán Free dovoľuje jeden vlastný konektor."],
      chatgpt: ["V ChatGPT otvorte Plugins, zvoľte + a potom Add custom MCP server.", "Zadajte názov, adresu vložte pod Connection a zvoľte prihlásenie bez autentifikácie.", "Potvrďte upozornenie (I understand and want to continue).", "Staršia verzia: Settings → Connectors → Advanced → Developer mode, potom Create. Vlastné servery ChatGPT ponúka len v niektorých plánoch a len na webe."],
      code: ["VS Code: súbor .vscode/mcp.json v projekte:", "Cursor: súbor .cursor/mcp.json v projekte alebo ~/.cursor/mcp.json:", "Claude Code: v termináli spustite:"]
    },
    cs: {
      open: "Přidat do svého AI asistenta",
      title: "Přidat do svého AI asistenta",
      lead: "Stejná data a dokumenty, ze kterých odpovídá tento asistent, můžete číst i ve vlastním AI asistentovi. Konektor jen čte, není potřeba účet ani klíč.",
      address: "Adresa konektoru", copy: "Kopírovat adresu", copied: "Adresa je zkopírovaná.", selected: "Adresa je označená: stiskněte Ctrl+C.", tryIt: "Zkuste se tam zeptat:",
      claude: ["V Claude otevřete Customize → Connectors.", "Zvolte + Add a potom Add custom connector.", "Zadejte název, vložte adresu a zvolte Continue.", "U přihlášení zvolte No sign in a potom Add.", "V konverzaci ho zapnete přes + → Connectors. Plán Free dovoluje jeden vlastní konektor."],
      chatgpt: ["V ChatGPT otevřete Plugins, zvolte + a potom Add custom MCP server.", "Zadejte název, adresu vložte pod Connection a zvolte přihlášení bez autentizace.", "Potvrďte upozornění (I understand and want to continue).", "Starší verze: Settings → Connectors → Advanced → Developer mode, potom Create. Vlastní servery ChatGPT nabízí jen v některých plánech a jen na webu."],
      code: ["VS Code: soubor .vscode/mcp.json v projektu:", "Cursor: soubor .cursor/mcp.json v projektu nebo ~/.cursor/mcp.json:", "Claude Code: v terminálu spusťte:"]
    },
    en: {
      open: "Add to your AI assistant",
      title: "Add to your AI assistant",
      lead: "The same data and documents this assistant answers from, in your own AI assistant. The connector only reads; it needs no account and no key.",
      address: "Connector address", copy: "Copy the address", copied: "The address is copied.", selected: "The address is selected: press Ctrl+C.", tryIt: "Try asking there:",
      claude: ["In Claude, open Customize → Connectors.", "Choose + Add, then Add custom connector.", "Enter a name, paste the address and choose Continue.", "Under authentication choose No sign in, then Add.", "Turn it on in a chat under + → Connectors. The Free plan allows one custom connector."],
      chatgpt: ["In ChatGPT, open Plugins, choose + and then Add custom MCP server.", "Enter a name, paste the address under Connection and choose no authentication.", "Confirm the warning (I understand and want to continue).", "An older version: Settings → Connectors → Advanced → Developer mode, then Create. ChatGPT offers custom servers on some plans only, and on the web only."],
      code: ["VS Code: the file .vscode/mcp.json in your project:", "Cursor: the file .cursor/mcp.json in your project or ~/.cursor/mcp.json:", "Claude Code: run in a terminal:"]
    }
  };

  /**
   * The panel's text for connector `url` (an https address, else null) of deployment `publicId`
   * in `lang` (Slovak when it has none): the words, and per client its steps, with the snippet a
   * developer tool takes. The server's name in a snippet is the publicId, which is [a-z0-9-].
   */
  function connector(url, publicId, lang) {
    if (!/^https:\/\/[a-z0-9.-]+(?::\d{1,5})?\/[\x21-\x7e]*$/i.test(String(url || ""))) return null;
    var name = /^[a-z0-9-]{1,63}$/.test(String(publicId || "")) ? publicId : "assistant";
    var t = CONNECT[lang] || CONNECT.sk;
    var server = {};
    server[name] = { type: "http", url: url };
    var cursor = {};
    cursor[name] = { url: url };
    return {
      open: t.open, title: t.title, lead: t.lead, address: t.address, copy: t.copy, copied: t.copied, selected: t.selected, tryIt: t.tryIt,
      clients: [
        { name: "Claude", steps: t.claude.map(function (text) { return { text: text }; }) },
        { name: "ChatGPT", steps: t.chatgpt.map(function (text) { return { text: text }; }) },
        { name: "VS Code, Cursor, Claude Code", steps: [
          { text: t.code[0], code: JSON.stringify({ servers: server }, null, 2) },
          { text: t.code[1], code: JSON.stringify({ mcpServers: cursor }, null, 2) },
          { text: t.code[2], code: "claude mcp add --transport http " + name + " " + url }
        ] }
      ]
    };
  }

  return {
    closeTargets: closeTargets, connector: connector, dataset: dataset, view: view,
    sourceCount: sourceCount, badges: badges, chosen: chosen, asksLanguage: asksLanguage, ordered: ordered,
    followups: followups, restore: restore, remember: remember, stamp: stamp,
    lasting: lasting, recalled: recalled, privacyLines: privacyLines,
    cards: cards, stage: stage, steps: steps, firstTable: firstTable, tel: tel, mailto: mailto,
    markdown: markdown, streamable: streamable, sources: sources, label: label, unlisted: unlisted, linkHost: linkHost,
    safeUrl: safeUrl, events: events, language: language, table: table, chart: chart, greeting: greeting, title: title,
    suggestions: suggestions, trouble: trouble, retryable: retryable, said: said, retry: retry,
    vizOrigin: vizOrigin, vizSpec: vizSpec, mapFirst: mapFirst
  };
})();
if (typeof module !== "undefined") module.exports = JCRender;
// The knowledge assistant's public chat (API/05 §1.6, T-3058, AG-114, T-4138): the full page at
// /d/{publicId} and the panel the launcher (embed.js) opens. One question at a time to the chat
// route of this host, the answer streamed as Server-Sent Events, the conversation and its last
// turns kept in this panel's own sessionStorage, so they survive a page change on the host site
// (T-4174). No cookie, no third party, no HTML from the model.
"use strict";
(function () {
  var root = document.getElementById("jc-chat");
  if (!root) return;
  var config = JSON.parse(root.getAttribute("data-config") || "{}");
  // The chat's own host by default; a copy of the widget on another host names it (T-4694).
  var api = /^https:\/\/[a-z0-9.-]+(:\d{1,5})?$/i.test(String(config.api || "")) ? config.api : "";
  var WORDS = {
    sk: { ask: "Napíšte otázku", send: "Odoslať", stop: "Zastaviť", stopped: "Odpoveď ste zastavili.",
      tools: "Živé dáta, ktoré smie asistent čítať", thinking: "Hľadám…",
      script: "Skript, ktorý asistent spustil", failed: "Odpoveď sa nepodarilo načítať. Skúste to znova.",
      offline: "Nepodarilo sa spojiť so službou. Skontrolujte pripojenie a skúste to znova.",
      busy: "Položili ste veľa otázok naraz. Počkajte chvíľu a skúste to znova.",
      paused: "Asistent teraz neodpovedá. Skúste to neskôr.", limit: "Asistent dosiahol svoj limit. Skúste to znova neskôr.", you: "Vy", assistant: "Asistent", bot: "AI asistent",
      hello: "Dobrý deň! Na čo sa chcete opýtať?",
      reading: "Čítam", live: "Živé dáta", unlisted: "nie je v katalógu otvorených dát", source: "Zdroj", allRows: "Zobraziť všetky v datasete", fresh: "Nová konverzácia", expand: "Celá obrazovka", collapse: "Zmenšiť", copy: "Kopírovať",
      copied: "Odpoveď je skopírovaná.", selected: "Odpoveď je označená: stlačte Ctrl+C.", close: "Zavrieť", suggested: "Môžete sa opýtať",
      dataset: "Otvoriť dataset", download: "Stiahnuť údaje", geojson: "Stiahnuť GeoJSON", language: "Jazyk",
      table: "Údaje, z ktorých asistent odpovedá", up: "Užitočná odpoveď", down: "Neužitočná odpoveď",
      missing: "Čo chýbalo? (nepovinné)", sendNote: "Odoslať", skip: "Preskočiť",
      thanks: "Ďakujeme za hodnotenie.", more: "Ďalšie otázky", contact: "Kontakt", map: "Zobraziť na mape", steps: "Postup", retry: "Skúsiť znova",
      full: "Tento rozhovor je už dlhý. Pokračujte v novom rozhovore, otázku v ňom položím znova.",
      unreachable: "Asistent sa teraz nevie spojiť so svojím modelom. Skúste to o minútu znova.",
      unanswered: "Asistent nevedel zostaviť odpoveď. Skúste sa opýtať inými slovami.",
      slow: "Odpoveď trvala príliš dlho. Skúste otázku položiť kratšie alebo inak.",
      declined: "Na túto otázku asistent neodpovedá.",
      choose: "Vyberte jazyk",
      verify: "Dôležité informácie si prosím overte.", privacy: "Súkromie", aboutTitle: "O tomto asistentovi",
      aboutAi: "Odpovede vytvára umelá inteligencia a môžu obsahovať chyby. Dôležité informácie si overte v oficiálnych zdrojoch. Nevkladajte citlivé osobné údaje.",
      remember: "Zapamätať konverzáciu v tomto prehliadači",
      rememberHint: "Konverzácia ostane v tomto prehliadači najviac 90 dní od poslednej zmeny; vypnutím ju hneď vymažete. Na zdieľanom zariadení túto možnosť nezapínajte.",
      keptHeading: "Konverzácie",
      keptDays: "Obsah konverzácie uchovávame {days} dní na poskytovanie služby, spätnú kontrolu a riešenie chýb. História zahŕňa otázky a zobrazené odpovede.",
      keptNone: "Obsah konverzácií neuchovávame.",
      askHeading: "Otázky k osobným údajom",
      askData: "Chcete sa opýtať na spracúvanie údajov alebo požiadať o prístup, opravu či vymazanie? Napíšte nám:",
      documents: "Informácie o ochrane osobných údajov", moreInfo: "Viac", lessInfo: "Menej", tabTable: "Tabuľka", tabMap: "Mapa", chart: "Graf",
      dock: "Pripnúť k okraju", undock: "Plávajúce okno", follow: "Otvárať dataset automaticky",
      opened: "Otvoril som dataset {title}", back: "Späť", changed: "Stránka zmenená na dataset {title}" },
    en: { ask: "Type your question", send: "Send", stop: "Stop", stopped: "You stopped the answer.",
      tools: "Live data the assistant may read", thinking: "Looking it up…",
      script: "Script the assistant ran", failed: "The answer could not be loaded. Try again.",
      offline: "The service could not be reached. Check your connection and try again.",
      busy: "You asked many questions at once. Wait a moment and try again.",
      paused: "The assistant is not answering right now. Try again later.", limit: "The assistant has reached its limit. Try again later.", you: "You", assistant: "Assistant", bot: "AI assistant",
      hello: "Hello! What would you like to ask?",
      reading: "Reading", live: "Live data", unlisted: "not in the open data catalogue", source: "Source", allRows: "Show all in the dataset", fresh: "New conversation", expand: "Full screen", collapse: "Collapse", copy: "Copy",
      copied: "The answer is copied.", selected: "The answer is selected: press Ctrl+C.", close: "Close", suggested: "You can ask",
      dataset: "Open the dataset", download: "Download the data", geojson: "Download GeoJSON", language: "Language",
      table: "The data the assistant answers from", up: "Helpful answer", down: "Not helpful",
      missing: "What was missing? (optional)", sendNote: "Send", skip: "Skip",
      thanks: "Thank you for the feedback.", more: "More questions", contact: "Contact", map: "Show on the map", steps: "Steps", retry: "Try again",
      full: "This conversation is already long. Continue in a new conversation; I will ask your question there again.",
      unreachable: "The assistant cannot reach its model right now. Try again in a minute.",
      unanswered: "The assistant could not put an answer together. Try asking in other words.",
      slow: "The answer took too long. Try a shorter or different question.",
      declined: "The assistant does not answer this question.",
      choose: "Choose your language",
      verify: "Please verify important information.", privacy: "Privacy", aboutTitle: "About this assistant",
      aboutAi: "Answers are made by artificial intelligence and may contain mistakes. Check important information in official sources. Do not enter sensitive personal data.",
      remember: "Remember the conversation in this browser",
      rememberHint: "The conversation stays in this browser for at most 90 days after its last change; switching it off deletes it at once. Do not switch this on on a shared device.",
      keptHeading: "Conversations",
      keptDays: "We keep the content of conversations for {days} days to provide the service, review it and fix errors. The history holds the questions and the answers shown.",
      keptNone: "We keep no conversation content.",
      askHeading: "Questions about personal data",
      askData: "Want to ask about how data is processed, or request access, correction or erasure? Write to us:",
      documents: "Privacy information", moreInfo: "More", lessInfo: "Less", tabTable: "Table", tabMap: "Map", chart: "Chart",
      dock: "Dock to the side", undock: "Floating window", follow: "Open the dataset automatically",
      opened: "I opened the dataset {title}", back: "Back", changed: "The page changed to the dataset {title}" },
    cs: { ask: "Napište otázku", send: "Odeslat", stop: "Zastavit", stopped: "Odpověď jste zastavili.",
      tools: "Živá data, která smí asistent číst", thinking: "Hledám…",
      script: "Skript, který asistent spustil", failed: "Odpověď se nepodařilo načíst. Zkuste to znovu.",
      offline: "Službu se nepodařilo zastihnout. Zkontrolujte připojení a zkuste to znovu.",
      busy: "Položili jste mnoho otázek najednou. Chvíli počkejte a zkuste to znovu.",
      paused: "Asistent teď neodpovídá. Zkuste to později.", limit: "Asistent dosáhl svého limitu. Zkuste to znovu později.", you: "Vy", assistant: "Asistent", bot: "AI asistent",
      hello: "Dobrý den! Na co se chcete zeptat?",
      reading: "Čtu", live: "Živá data", unlisted: "není v katalogu otevřených dat", source: "Zdroj", allRows: "Zobrazit všechny v datové sadě", fresh: "Nová konverzace", expand: "Celá obrazovka", collapse: "Zmenšit", copy: "Kopírovat",
      copied: "Odpověď je zkopírovaná.", selected: "Odpověď je označená: stiskněte Ctrl+C.", close: "Zavřít", suggested: "Můžete se zeptat",
      dataset: "Otevřít dataset", download: "Stáhnout data", geojson: "Stáhnout GeoJSON", language: "Jazyk",
      table: "Data, ze kterých asistent odpovídá", up: "Užitečná odpověď", down: "Neužitečná odpověď",
      missing: "Co chybělo? (nepovinné)", sendNote: "Odeslat", skip: "Přeskočit",
      thanks: "Děkujeme za hodnocení.", more: "Další otázky", contact: "Kontakt", map: "Zobrazit na mapě", steps: "Postup", retry: "Zkusit znovu",
      full: "Tento rozhovor je už dlouhý. Pokračujte v novém rozhovoru, otázku v něm položím znovu.",
      unreachable: "Asistent se teď nemůže spojit se svým modelem. Zkuste to za minutu znovu.",
      unanswered: "Asistent nedokázal sestavit odpověď. Zkuste se zeptat jinými slovy.",
      slow: "Odpověď trvala příliš dlouho. Zkuste otázku položit kratší nebo jinak.",
      declined: "Na tuto otázku asistent neodpovídá.",
      choose: "Vyberte jazyk",
      verify: "Důležité informace si prosím ověřte.", privacy: "Soukromí", aboutTitle: "O tomto asistentovi",
      aboutAi: "Odpovědi vytváří umělá inteligence a mohou obsahovat chyby. Důležité informace si ověřte v oficiálních zdrojích. Nevkládejte citlivé osobní údaje.",
      remember: "Zapamatovat konverzaci v tomto prohlížeči",
      rememberHint: "Konverzace zůstane v tomto prohlížeči nejvýše 90 dní od poslední změny; vypnutím ji hned smažete. Na sdíleném zařízení tuto možnost nezapínejte.",
      keptHeading: "Konverzace",
      keptDays: "Obsah konverzace uchováváme {days} dní pro poskytování služby, zpětnou kontrolu a řešení chyb. Historie zahrnuje otázky a zobrazené odpovědi.",
      keptNone: "Obsah konverzací neuchováváme.",
      askHeading: "Dotazy k osobním údajům",
      askData: "Chcete se zeptat na zpracování údajů nebo požádat o přístup, opravu či smazání? Napište nám:",
      documents: "Informace o ochraně osobních údajů", moreInfo: "Více", lessInfo: "Méně", tabTable: "Tabulka", tabMap: "Mapa", chart: "Graf",
      dock: "Připnout k okraji", undock: "Plovoucí okno", follow: "Otevírat dataset automaticky",
      opened: "Otevřel jsem dataset {title}", back: "Zpět", changed: "Stránka změněna na dataset {title}" }
  };
  // Each language's name in itself, for the toggle's labels and the `ask` mode's chooser (T-4283).
  var NATIVE = { sk: "Slovensky", en: "English", cs: "Česky" };
  var params = new URLSearchParams(location.search);
  var embedded = params.get("embed") === "1" && window.parent !== window;
  // A light host page asks for the light scheme (T-4761): a cross-origin frame never inherits
  // the host's colours, so a dark system would paint a dark chat inside a light page.
  if (params.get("scheme") === "light") document.documentElement.setAttribute("data-scheme", "light");
  var page = document.body.classList.contains("jc-page");
  var offered = JCRender.ordered((config.languages || []).filter(function (l) { return WORDS[l]; }));
  // How the chat picks its language and whether it shows the toggle, the deployment's
  // `widget` (T-4343): `browser` by default, `ask` keeps the first-open chooser, `fixed` its first.
  var mode = (config.widget && config.widget.language) || "browser";
  var toggled = !(config.widget && config.widget.languageToggle === false);
  // The visitor's own choice, kept in this browser's storage; storage may be off, then the
  // chat opens by the browser's languages again (or the chooser asks again, in `ask` mode).
  var LANG_KEY = "jc-chat-lang";
  function storedLanguage() {
    try { return JCRender.chosen(localStorage.getItem(LANG_KEY), offered); } catch (e) { return null; }
  }
  function storeLanguage(code) {
    try { localStorage.setItem(LANG_KEY, code); } catch (e) { /* asked again next time */ }
  }
  // Without the toggle there is nothing to pick, so an old pick is not read.
  var stored = toggled ? storedLanguage() : null;
  // The browser's languages are read here and never sent: only the chosen code is used.
  var browser = navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language];
  var lang = JCRender.language(params.get("lang"), config.languages, WORDS, stored, browser, mode);
  var w = WORDS[lang];
  if (/^#[0-9a-fA-F]{6}$/.test(config.color || "")) {
    document.documentElement.style.setProperty("--accent", config.color);
  }
  // The host page's type (T-4740): family names only, as the server checked them.
  if (/^[\p{L}\p{N} ,'"-]{1,200}$/u.test(config.font || "")) {
    document.documentElement.style.setProperty("--font", config.font);
  }

  function el(tag, cls, text) {
    var node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  // Copies `value`, then `done`. Where the browser refuses the clipboard (an http page, a frame
  // without clipboard-write) its own copy command; failing that `node` stays selected for Ctrl+C
  // and `refused` says so, never a silent button (T-4762).
  function copyText(value, node, done, refused) {
    var fallback = function () {
      var area = el("textarea");
      area.value = value;
      area.setAttribute("readonly", "");
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.appendChild(area);
      area.select();
      var copied = false;
      try { copied = document.execCommand("copy"); } catch (error) { copied = false; }
      area.remove();
      if (copied) return done();
      if (node.select) node.select();
      else {
        var range = document.createRange();
        range.selectNodeContents(node);
        var selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
      }
      refused();
    };
    if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(value).then(done, fallback);
    else fallback();
  }

  // Every word on the page is bound to its key, so the language switch relabels them in place.
  var bound = [];
  function words(node, key, prop) {
    bound.push([node, key, prop || "textContent"]);
    node[prop || "textContent"] = typeof key === "function" ? key() : w[key];
    return node;
  }
  function button(cls, key) {
    var b = el("button", cls);
    b.type = "button";
    return words(b, key);
  }

  var SVG = "http://www.w3.org/2000/svg";

  function svg(tag, attrs, text) {
    var node = document.createElementNS(SVG, tag);
    Object.keys(attrs).forEach(function (k) { node.setAttribute(k, String(attrs[k])); });
    if (text) node.textContent = text;
    return node;
  }

  // The widget's few icons, drawn inline (no font, no image request): 24-unit paths, stroked.
  var ICONS = {
    fresh: "M12 5v14M5 12h14",
    close: "M6 6l12 12M18 6L6 18",
    expand: "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5",
    collapse: "M9 4v5H4M20 9h-5V4M15 20v-5h5M4 15h5v5",
    // Dock: a window with a panel at its right edge; undock: a window with a title bar (T-4767).
    dock: "M6 5h12a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2zM15 5v14",
    undock: "M6 5h12a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2zM4 9h16",
    chat: "M4 5h16v11H9l-5 4z",
    page: "M6 3h8l4 4v14H6zM14 3v4h4M9 12h6M9 16h6",
    table: "M4 5h16v14H4zM4 10h16M4 15h16M10 5v14",
    plug: "M9 3v5M15 3v5M6 8h12v3a6 6 0 0 1-12 0zM12 17v4",
    send: "M5 12h13M12 5l7 7-7 7",
    stop: "M7 7h10v10H7z",
    copy: "M9 9h10v10H9zM5 15V5h10",
    up: "M7 11v9H4v-9zM7 11l4-7c1.5 0 2.5 1 2 3l-1 3h6c1 0 2 1 1.7 2.2l-1.5 6C18 19.3 17.2 20 16 20H7",
    down: "M7 13V4H4v9zM7 13l4 7c1.5 0 2.5-1 2-3l-1-3h6c1 0 2-1 1.7-2.2l-1.5-6C18 4.7 17.2 4 16 4H7"
  };
  function icon(name) {
    var drawing = svg("svg", { viewBox: "0 0 24 24", width: 20, height: 20, "aria-hidden": "true", focusable: "false", class: "jc-icon" });
    drawing.appendChild(svg("path", { d: ICONS[name] }));
    return drawing;
  }
  // An icon button: its name is the word, read out and shown as a tooltip.
  function iconButton(cls, key, name) {
    var b = el("button", "jc-icon-button " + cls);
    b.type = "button";
    b.appendChild(icon(name));
    words(b, key, "ariaLabel");
    words(b, key, "title");
    return b;
  }

  var head = el("header", "jc-head");
  var brand = el("div", "jc-brand");
  var named = JCRender.title(config.title, config.languages, lang) !== "";
  // The chat icon always, never a monogram of the name (T-4740).
  var avatar = el("span", "jc-avatar");
  avatar.setAttribute("aria-hidden", "true");
  avatar.appendChild(icon("chat"));
  var title = el("h1", "jc-title");
  var name = el("span", "jc-name");
  // The title in the chat's language, again on every language switch (T-4365).
  function retitle() {
    var text = JCRender.title(config.title, config.languages, lang);
    name.textContent = text;
    name.title = text;
  }
  if (named) {
    retitle();
    title.appendChild(name);
    title.appendChild(words(el("span", "jc-role"), function () {
      return JCRender.title(config.subtitle, config.languages, lang) || WORDS[lang].assistant;
    }));
  } else {
    title.appendChild(words(el("span", "jc-name"), "assistant"));
  }
  brand.appendChild(avatar);
  brand.appendChild(title);
  var actions = el("div", "jc-actions");
  // The languages as a segmented toggle, SK | EN: nothing drops down over the conversation
  // (T-4283 item 8), and the visitor's choice is kept for the next open.
  var picker = null;
  if (offered.length > 1 && toggled) {
    picker = el("div", "jc-langs");
    picker.setAttribute("role", "group");
    words(picker, "language", "ariaLabel");
    offered.forEach(function (code) {
      var option = el("button", "", code.toUpperCase());
      option.type = "button";
      option.lang = code;
      option.value = code;
      option.setAttribute("aria-label", NATIVE[code] || code);
      option.addEventListener("click", function () {
        storeLanguage(code);
        relabel(code);
      });
      picker.appendChild(option);
    });
    actions.appendChild(picker);
  }
  // "Add to your AI assistant" (T-4694): a public deployment's own MCP connector.
  var connectOpen = null;
  if (JCRender.connector(config.mcp, config.publicId, lang)) {
    connectOpen = el("button", "jc-icon-button jc-connect-open");
    connectOpen.type = "button";
    connectOpen.appendChild(icon("plug"));
    connectOpen.setAttribute("aria-haspopup", "dialog");
    var connectWord = function () { return JCRender.connector(config.mcp, config.publicId, lang).open; };
    words(connectOpen, connectWord, "ariaLabel");
    words(connectOpen, connectWord, "title");
    actions.appendChild(connectOpen);
  }
  var fresh = iconButton("jc-fresh", "fresh", "fresh");
  actions.appendChild(fresh);
  // Embedded, the panel grows to the full window and back (T-4658); on a phone it already fills
  // the screen, so the button is left out there. A host that frames it with `dock=1` (the
  // catalogue, T-4685) docks it to its right edge on a wide screen, docked by default.
  var size = null;
  var dock = null;
  var placed = null;
  try { placed = sessionStorage.getItem("jc-chat-size:" + config.publicId); } catch (e) { /* panel */ }
  var docking = embedded && params.get("dock") === "1" && screen.width >= 1100;
  var expanded = false;
  var docked = docking && placed !== "panel";
  if (docking) {
    dock = el("button", "jc-icon-button jc-dock");
    dock.type = "button";
    dock.appendChild(icon(docked ? "undock" : "dock"));
    var docks = function () { return w[docked ? "undock" : "dock"]; };
    words(dock, docks, "ariaLabel");
    words(dock, docks, "title");
    dock.setAttribute("aria-pressed", String(docked));
    dock.addEventListener("click", function () { place(docked ? "panel" : "dock"); });
    actions.appendChild(dock);
  }
  if (embedded && screen.width >= 600) {
    expanded = placed === "full";
    if (dock) dock.hidden = expanded;
    size = el("button", "jc-icon-button jc-size");
    size.type = "button";
    size.appendChild(icon(expanded ? "collapse" : "expand"));
    var sized = function () { return w[expanded ? "collapse" : "expand"]; };
    words(size, sized, "ariaLabel");
    words(size, sized, "title");
    size.setAttribute("aria-pressed", String(expanded));
    size.addEventListener("click", function () { resize(!expanded); });
    actions.appendChild(size);
  }
  if (embedded) {
    var close = iconButton("jc-close", "close", "close");
    close.addEventListener("click", closePanel);
    actions.appendChild(close);
  }
  head.appendChild(brand);
  head.appendChild(actions);

  var toggles = [];
  var tools = null;
  if ((config.connectors || []).length > 0) {
    tools = el("details", "jc-tools");
    tools.appendChild(words(el("summary"), "tools"));
    config.connectors.forEach(function (name) {
      var label = el("label");
      var box = el("input");
      box.type = "checkbox";
      box.checked = true;
      box.value = name;
      label.appendChild(box);
      label.appendChild(document.createTextNode(name));
      tools.appendChild(label);
      toggles.push(box);
    });
  }

  var log = el("ol", "jc-log");
  words(log, "assistant", "ariaLabel");
  // The finished answer is read out once from here; the log itself is not live, so a streamed
  // answer is not announced character by character.
  var announce = el("p", "jc-sr");
  announce.setAttribute("aria-live", "polite");

  var form = el("form", "jc-form");
  var input = el("textarea");
  input.id = "jc-question";
  input.rows = 1;
  input.maxLength = config.questionChars || 1000;
  input.required = true;
  var label = words(el("label", "jc-sr"), "ask");
  label.htmlFor = "jc-question";
  words(input, "ask", "placeholder");
  // One bar: the field and, inside its border, a labelled send button (T-4635 option C); the bar
  // grows with the question. Stop takes the button's place while an answer streams.
  function labelled(cls, key, name) {
    var b = el("button", "jc-send " + cls);
    b.appendChild(words(el("span"), key));
    b.appendChild(icon(name));
    return b;
  }
  var send = labelled("jc-submit", "send", "send");
  send.type = "submit";
  var stop = labelled("jc-stop", "stop", "stop");
  stop.type = "button";
  stop.hidden = true;
  input.addEventListener("input", grow);
  function grow() {
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 160) + "px";
  }
  form.appendChild(label);
  form.appendChild(input);
  form.appendChild(send);
  form.appendChild(stop);
  var foot = el("footer", "jc-foot");
  foot.appendChild(form);
  // Under every chat (API/05 §1.6, T-4670): verify what matters, and the privacy dialog, which
  // carries the deployment's disclaimer.
  var line = el("p", "jc-privacy-line");
  line.appendChild(words(el("span"), "verify"));
  line.appendChild(document.createTextNode(" · "));
  var privacyOpen = button("jc-more", "privacy");
  privacyOpen.setAttribute("aria-haspopup", "dialog");
  line.appendChild(privacyOpen);
  foot.appendChild(line);
  // A modal <dialog>: Escape closes it, the page behind is inert, and Tab cycles inside it.
  var about = el("dialog", "jc-about");
  about.setAttribute("aria-labelledby", "jc-about-title");
  var aboutTitle = words(el("h2"), "aboutTitle");
  aboutTitle.id = "jc-about-title";
  var aboutLead = el("p");
  var aboutBody = el("div", "jc-about-body");
  var rememberLabel = el("label", "jc-remember");
  var rememberBox = el("input");
  rememberBox.type = "checkbox";
  rememberBox.setAttribute("role", "switch");
  rememberLabel.appendChild(rememberBox);
  rememberLabel.appendChild(words(el("span"), "remember"));
  var rememberHint = words(el("p", "jc-hint"), "rememberHint");
  // On a docking host (T-4685): whether a done answer about one dataset opens it beside the
  // panel; on by default, kept in this browser.
  var FOLLOW = "jc-chat-follow:" + config.publicId;
  var followLabel = el("label", "jc-remember");
  var followBox = el("input");
  followBox.type = "checkbox";
  followBox.setAttribute("role", "switch");
  followBox.checked = following();
  followBox.addEventListener("change", function () {
    try {
      if (followBox.checked) localStorage.removeItem(FOLLOW);
      else localStorage.setItem(FOLLOW, "off");
    } catch (e) { /* this page only */ }
  });
  followLabel.appendChild(followBox);
  followLabel.appendChild(words(el("span"), "follow"));
  var aboutClose = button("jc-about-close", "close");
  [aboutTitle, aboutLead, rememberLabel, rememberHint].concat(docking ? [followLabel] : [], [aboutBody, aboutClose])
    .forEach(function (n) { about.appendChild(n); });
  function following() {
    try { return localStorage.getItem(FOLLOW) !== "off"; } catch (e) { return true; }
  }

  var connect = el("dialog", "jc-about jc-connect");
  connect.setAttribute("aria-labelledby", "jc-connect-title");
  function fillConnect() {
    var text = JCRender.connector(config.mcp, config.publicId, lang);
    connect.textContent = "";
    var heading = el("h2", "", text.title);
    heading.id = "jc-connect-title";
    connect.appendChild(heading);
    connect.appendChild(el("p", "", text.lead));
    var field = el("label", "jc-connect-address");
    field.appendChild(el("span", "", text.address));
    var address = el("input");
    address.type = "text";
    address.readOnly = true;
    address.value = config.mcp;
    address.addEventListener("focus", function () { address.select(); });
    field.appendChild(address);
    connect.appendChild(field);
    var copied = el("p", "jc-sr");
    copied.setAttribute("aria-live", "polite");
    var copyIt = el("button", "jc-about-close", text.copy);
    copyIt.type = "button";
    copyIt.addEventListener("click", function () {
      copyText(config.mcp, address, function () { copied.textContent = text.copied; }, function () { copied.textContent = text.selected; });
    });
    connect.appendChild(copyIt);
    connect.appendChild(copied);
    text.clients.forEach(function (client) {
      var part = el("details");
      part.appendChild(el("summary", "", client.name));
      var list = el("ol");
      client.steps.forEach(function (step) {
        var item = el("li", "", step.text);
        if (step.code) item.appendChild(el("pre", "jc-connect-code", step.code));
        list.appendChild(item);
      });
      part.appendChild(list);
      connect.appendChild(part);
    });
    var first = JCRender.suggestions(config.suggestions, lang, [])[0];
    if (first) {
      connect.appendChild(el("p", "jc-hint", text.tryIt));
      connect.appendChild(el("p", "jc-connect-try", first));
    }
    var shut = button("jc-about-close", "close");
    shut.addEventListener("click", function () { connect.close(); });
    connect.appendChild(shut);
  }
  if (connectOpen) {
    connectOpen.addEventListener("click", function () {
      fillConnect();
      connect.showModal();
    });
  }

  root.appendChild(head);
  if (tools) root.appendChild(tools);
  root.appendChild(log);
  root.appendChild(foot);
  root.appendChild(about);
  root.appendChild(connect);
  root.appendChild(announce);

  var conversation = null;
  var history = [];
  var turns = 0;
  var busy = null;
  var intro = null;
  // Questions the server counted in this conversation: an answer's `turn` for its feedback.
  var asked = 0;
  var kept = null;
  var KEY = "jc-chat:" + config.publicId;

  // sessionStorage may be off (a blocked third-party frame, a private window): the chat works
  // without it, it only forgets on a page change then.
  function keep() {
    try {
      if (kept) sessionStorage.setItem(KEY, JSON.stringify(kept));
      else sessionStorage.removeItem(KEY);
    } catch (e) { /* not kept */ }
    if (remembering) {
      try { localStorage.setItem(LOCAL, JCRender.lasting(kept, new Date())); } catch (e) { /* not kept */ }
    }
  }
  // The visitor's own switch (T-4670): off by default; the copy in localStorage lives 90 days
  // after its last change, and an expired one is deleted here.
  var LOCAL = "jc-chat-remember:" + config.publicId;
  var remembering = false;
  function kept0() {
    var held = null;
    try {
      held = JCRender.recalled(localStorage.getItem(LOCAL), new Date());
      if (!held) localStorage.removeItem(LOCAL);
    } catch (e) { /* storage off */ }
    remembering = Boolean(held);
    var session = null;
    try { session = JCRender.restore(sessionStorage.getItem(KEY)); } catch (e) { /* storage off */ }
    return session || (held && held.kept);
  }
  rememberBox.addEventListener("change", function () {
    remembering = rememberBox.checked;
    if (remembering) keep();
    else {
      try { localStorage.removeItem(LOCAL); } catch (e) { /* nothing kept */ }
    }
  });

  function fillAbout() {
    var lines = JCRender.privacyLines(w, config.transcriptDays, config.privacy, perLanguage(config.disclaimer));
    aboutLead.textContent = lines[0].text;
    aboutBody.textContent = "";
    lines.slice(1).forEach(function (l) {
      if (l.heading) aboutBody.appendChild(el("h3", "", l.heading));
      var p = el("p");
      if (l.href) p.appendChild(link(l.href, l.text));
      else p.textContent = l.text;
      if (l.mail) {
        var a = el("a", "", l.mail);
        a.href = JCRender.mailto(l.mail);
        p.appendChild(document.createTextNode(" "));
        p.appendChild(a);
      }
      aboutBody.appendChild(p);
    });
  }
  privacyOpen.addEventListener("click", function () {
    fillAbout();
    rememberBox.checked = remembering;
    about.showModal();
  });
  aboutClose.addEventListener("click", function () { about.close(); });
  about.addEventListener("keydown", function (event) {
    if (event.key !== "Tab") return;
    var stops = about.querySelectorAll("a[href], input, button");
    var first = stops[0];
    var last = stops[stops.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  });
  // The close event comes a task later: focus the visitor already moved (the question box) stays.
  about.addEventListener("close", function () {
    var at = document.activeElement;
    if (!at || at === document.body || about.contains(at)) privacyOpen.focus();
  });

  function perLanguage(map) {
    if (!map) return null;
    return map[lang] || map[config.languages && config.languages[0]] || null;
  }

  // The empty chat: the greeting and the suggested questions, nothing else above the fold.
  // Once asked, the greeting stays above the conversation and its suggestions go.
  function greeted() {
    if (!intro) return;
    var list = intro.querySelector(".jc-suggest");
    if (list) list.remove();
    if (intro.children.length === 0) intro.remove();
    intro = null;
  }

  function welcome() {
    intro = el("li", "jc-intro");
    var hello = JCRender.greeting(config.greeting, config.languages, lang, w.hello);
    if (hello) intro.appendChild(el("p", "jc-greeting", hello));
    // The deployment's own questions in this language, and none it did not set (T-4283).
    var asks = JCRender.suggestions(config.suggestions, lang, []);
    if (asks.length > 0) {
      var list = el("ul", "jc-suggest");
      list.setAttribute("aria-label", w.suggested);
      asks.forEach(function (question) {
        var item = el("li");
        var b = el("button", "", question);
        b.type = "button";
        b.addEventListener("click", function () { submit(question); });
        item.appendChild(b);
        list.appendChild(item);
      });
      intro.appendChild(list);
    }
    if (intro.children.length > 0) log.appendChild(intro);
  }

  function relabel(code) {
    lang = code;
    w = WORDS[code];
    document.documentElement.lang = code;
    bound.forEach(function (b) { b[0][b[2]] = typeof b[1] === "function" ? b[1]() : w[b[1]]; });
    if (picker) {
      Array.prototype.forEach.call(picker.children, function (b) { b.setAttribute("aria-pressed", String(b.value === code)); });
    }
    if (named) retitle();
    if (about.open) fillAbout();
    if (connect.open) fillConnect();
    if (turns === 0 && !busy && config.enabled !== false) {
      log.textContent = "";
      welcome();
    }
  }

  // `who` is the word's key, bound like every other word, so a language switch renames the
  // speakers of the turns already shown (T-4278: "Vy" stayed on an English page).
  function turn(cls, who) {
    var item = el("li", cls);
    var speaker = el("span", "jc-sr");
    speaker.appendChild(words(el("span"), who));
    speaker.appendChild(document.createTextNode(": "));
    item.appendChild(speaker);
    log.appendChild(item);
    log.scrollTop = log.scrollHeight;
    return item;
  }

  // When a message was written (T-4339): the visitor's HH:MM, the UTC minute on hover and to a
  // screen reader's time element; nothing when the moment is not known.
  function when(at) {
    var s = JCRender.stamp(at, lang);
    if (!s) return null;
    var node = el("time", "jc-time", s.text);
    node.setAttribute("datetime", s.datetime);
    node.title = s.title;
    return node;
  }

  function link(href, text, cls) {
    var a = el("a", cls || "", text);
    a.href = href;
    a.target = "_blank";
    a.rel = "noopener noreferrer";
    return a;
  }

  var ALLOWED = { p: 1, ul: 1, ol: 1, li: 1, strong: 1, em: 1, code: 1, br: 1, table: 1, tr: 1, th: 1, td: 1, pre: 1 };

  // The renderer's tree as DOM nodes: text as text, only the elements it may produce.
  function build(node, ids, position) {
    if (typeof node === "string") return document.createTextNode(node);
    var out;
    if (node.tag === "a") {
      out = link(node.href, "");
    } else if (node.tag === "cite") {
      out = el("sup", "jc-cite");
      node.numbers.map(function (n) { return position[n]; })
        .filter(function (p, i, all) { return p && all.indexOf(p) === i; })
        .forEach(function (p, i) {
          if (i > 0) out.appendChild(document.createTextNode(","));
          var to = el("a", "", String(p));
          to.href = "#" + ids + "-" + p;
          // A marker opens the closed source list first (T-4364).
          to.addEventListener("click", function () {
            var card = document.getElementById(ids + "-" + p);
            if (card && card.parentNode.hidden) reveal(card.parentNode, true);
          });
          to.setAttribute("aria-label", w.source + " " + p);
          out.appendChild(to);
        });
      return out;
    } else {
      out = el(ALLOWED[node.tag] ? node.tag : "span");
    }
    (node.children || []).forEach(function (child) { out.appendChild(build(child, ids, position)); });
    return out;
  }

  // The answer so far, formatted by the same renderer as the final view (T-4649): only the
  // blocks from the first changed one on are rebuilt, and the log follows the end only while the
  // visitor is at the bottom. Markers wait for their sources, which come with the final view.
  function paint(item) {
    item.frame = 0;
    if (!item.streaming) return;
    var follow = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
    var linkable = function (url) { return JCRender.linkHost(url, config.linkHosts || []); };
    var blocks = JCRender.markdown(JCRender.streamable(item.text), null, linkable);
    var keys = blocks.map(function (block) { return JSON.stringify(block); });
    var shown = item.painted || [];
    var same = 0;
    while (same < keys.length && keys[same] === shown[same]) same += 1;
    while (item.streaming.childNodes.length > same) item.streaming.removeChild(item.streaming.lastChild);
    blocks.slice(same).forEach(function (block) { item.streaming.appendChild(build(block, "", {})); });
    item.painted = keys;
    if (follow) log.scrollTop = log.scrollHeight;
  }

  // The answer, formatted, its markers linked to the source cards under it, and its copy button.
  function show(item, text, citations, turn, at) {
    if (item.frame) cancelAnimationFrame(item.frame);
    turns += 1;
    var ids = "jc-src-" + turns;
    var grouped = JCRender.sources(citations);
    var body = el("div", "jc-answer");
    // Links stay links only to the deployment's own hosts and the sources this answer cites (T-4149).
    var hosts = (config.linkHosts || []).concat(grouped.list.map(function (source) { return source.domain; }));
    var linkable = function (url) { return JCRender.linkHost(url, hosts); };
    JCRender.markdown(text, function (n) { return Boolean(grouped.position[n]); }, linkable).forEach(function (block) {
      body.appendChild(build(block, ids, grouped.position));
    });
    if (item.streaming) {
      item.replaceChild(body, item.streaming);
      item.streaming = null;
    } else {
      item.insertBefore(body, item.visuals || null);
    }
    announce.textContent = body.textContent;
    // One row under the answer (T-4364): who and when, copy, thumbs, then the sources' badges and
    // count, which open the cards under the row.
    var tools = el("div", "jc-answer-tools");
    var by = el("span", "jc-by");
    by.appendChild(icon("chat"));
    by.appendChild(words(el("span"), "bot"));
    var time = when(at === undefined ? Date.now() : at);
    if (time) {
      by.appendChild(document.createTextNode(" • "));
      by.appendChild(time);
    }
    tools.appendChild(by);
    var copy = iconButton("jc-copy", "copy", "copy");
    copy.addEventListener("click", function () {
      copyText(text, body, function () { announce.textContent = w.copied; }, function () { announce.textContent = w.selected; });
    });
    tools.appendChild(copy);
    if (turn && config.feedback !== false) thumbs(tools, turn);
    item.appendChild(tools);
    if (grouped.list.length === 0) return;
    var open = el("button", "jc-sources");
    open.type = "button";
    open.id = ids + "-toggle";
    var stack = el("span", "jc-badges");
    stack.setAttribute("aria-hidden", "true");
    // A page for web sources, a table for data, in the text's colour; no letter, no hue (T-4740).
    JCRender.badges(grouped.list).forEach(function (kind) {
      var b = icon(kind);
      b.setAttribute("class", "jc-icon jc-badge");
      b.setAttribute("data-kind", kind);
      stack.appendChild(b);
    });
    open.appendChild(stack);
    var n = grouped.list.length;
    open.appendChild(words(el("span", "jc-count"), function () { return JCRender.sourceCount(n, lang); }));
    open.appendChild(el("span", "jc-chevron")).setAttribute("aria-hidden", "true");
    tools.appendChild(open);
    var list = el("ul", "jc-cards");
    list.id = ids;
    list.setAttribute("aria-labelledby", open.id);
    open.setAttribute("aria-controls", list.id);
    grouped.list.forEach(function (source, i) {
      var li = el("li", "jc-card");
      li.id = ids + "-" + (i + 1);
      var name = (source.live ? w.live + ": " : "") + JCRender.label(source);
      // Only an http(s) address is a link: an uploaded file cites `upload:{source}/{name}` (T-4184).
      var href = JCRender.safeUrl(source.url);
      var heading = href ? link(href, name) : el("span", "", name);
      li.appendChild(el("span", "jc-card-n", String(i + 1)));
      li.appendChild(heading);
      if (source.domain && !source.live) li.appendChild(el("span", "jc-domain", source.domain));
      // Rows of a type no dataset publishes: said in words, nothing to open (T-4802).
      if (JCRender.unlisted(source)) li.appendChild(el("span", "jc-domain", w.unlisted));
      source.cards.forEach(function (card) { li.appendChild(showCard(card)); });
      list.appendChild(li);
    });
    list.toggle = open;
    open.addEventListener("click", function () { reveal(list, list.hidden); });
    reveal(list, false);
    tools.parentNode.insertBefore(list, tools.nextSibling);
  }

  // The source cards stay closed under the footer row until its count opens them (T-4364).
  function reveal(list, open) {
    list.hidden = !open;
    list.toggle.setAttribute("aria-expanded", String(open));
  }

  // What the cited page publishes (T-4155): text, and a link only where render.js checked one.
  function showCard(card) {
    var box = el("div", "jc-part jc-part-" + card.kind);
    var item = function (entry) {
      return entry.href ? link(entry.href, entry.text) : el("span", "", entry.text);
    };
    if (card.kind === "contact") {
      box.appendChild(el("strong", "", card.name || w.contact));
      card.phones.concat(card.emails).forEach(function (entry) {
        box.appendChild(el("p")).appendChild(item(entry));
      });
    } else if (card.kind === "place") {
      box.appendChild(el("p", "", card.address));
      box.appendChild(el("p")).appendChild(link(card.href, w.map));
    } else {
      box.appendChild(el("strong", "", w.steps));
      var steps = box.appendChild(el("ol"));
      card.steps.forEach(function (step) { steps.appendChild(el("li", "", step)); });
    }
    return box;
  }

  // Thumbs up or down on one answer, a thumbs down asking one optional note (API/05 §1.8).
  function thumbs(tools, turn) {
    var of = conversation;
    var up = iconButton("jc-thumb", "up", "up");
    var down = iconButton("jc-thumb", "down", "down");
    function said(sentence) {
      var p = el("p", "jc-thanks", sentence);
      p.setAttribute("role", "status");
      tools.parentNode.insertBefore(p, tools.nextSibling);
    }
    async function vote(verdict, note) {
      up.disabled = true;
      down.disabled = true;
      var body = { conversation: of, turn: turn, verdict: verdict };
      if (note) body.note = note;
      try {
        var response = await fetch(api + "/api/v1/d/" + encodeURIComponent(config.publicId) + "/feedback", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          credentials: "omit",
          body: JSON.stringify(body)
        });
        if (response.ok) return said(w.thanks);
        said(w[refusal(response.status)]);
        // A refused note may be sent again without it.
        up.disabled = false;
        down.disabled = false;
      } catch (e) {
        said(w.offline);
        up.disabled = false;
        down.disabled = false;
      }
    }
    up.addEventListener("click", function () { vote("up"); });
    var asking = null;
    down.addEventListener("click", function () {
      if (asking) return asking.querySelector("textarea").focus();
      var box = asking = el("form", "jc-note");
      var area = el("textarea");
      area.maxLength = 500;
      area.rows = 2;
      area.setAttribute("aria-label", w.missing);
      area.placeholder = w.missing;
      var go = el("button", "", w.sendNote);
      go.type = "submit";
      var skip = el("button", "", w.skip);
      skip.type = "button";
      box.appendChild(area);
      box.appendChild(go);
      box.appendChild(skip);
      box.addEventListener("submit", function (event) {
        event.preventDefault();
        box.remove();
        asking = null;
        vote("down", area.value.trim());
      });
      skip.addEventListener("click", function () {
        box.remove();
        asking = null;
        vote("down");
      });
      tools.parentNode.insertBefore(box, tools.nextSibling);
      area.focus();
    });
    tools.appendChild(up);
    tools.appendChild(down);
  }

  // Up to three follow-up questions as chips under the answer; one click asks it (T-4174).
  function chips(item, questions) {
    if (questions.length === 0) return;
    var list = el("ul", "jc-suggest jc-followups");
    words(list, "more", "ariaLabel");
    questions.forEach(function (question) {
      var li = el("li");
      var b = el("button", "", question);
      b.type = "button";
      b.addEventListener("click", function () { submit(question); });
      li.appendChild(b);
      list.appendChild(li);
    });
    item.appendChild(list);
    item.chips = list;
  }

  // A data round's one numeric column as a small line or bar chart under its table (T-4198). The
  // table beside it is the text alternative, so the drawing is hidden from screen readers; every
  // label goes in as text, never as markup.
  function plot(shown) {
    if (!shown) return null;
    var W = 300, H = 90, top = 14, base = top + H;
    var values = shown.points.map(function (p) { return p.value; });
    var low = Math.min(0, Math.min.apply(null, values)), high = Math.max(0, Math.max.apply(null, values));
    var span = high - low || 1;
    var y = function (v) { return top + H - ((v - low) / span) * H; };
    var step = W / shown.points.length;
    var drawing = svg("svg", { viewBox: "0 0 " + W + " " + (base + 14), class: "jc-data-chart", "aria-hidden": "true", focusable: "false" });
    drawing.appendChild(svg("line", { x1: 0, x2: W, y1: y(0), y2: y(0), class: "jc-chart-axis" }));
    if (shown.kind === "line") {
      var at = shown.points.map(function (p, i) { return (i * step + step / 2).toFixed(1) + "," + y(p.value).toFixed(1); });
      drawing.appendChild(svg("polyline", { points: at.join(" "), class: "jc-chart-line" }));
    } else {
      shown.points.forEach(function (p, i) {
        var a = y(Math.max(p.value, 0)), b = y(Math.min(p.value, 0));
        drawing.appendChild(svg("rect", { x: (i * step + step * 0.1).toFixed(1), width: (step * 0.8).toFixed(1), y: a.toFixed(1), height: Math.max(b - a, 1).toFixed(1), class: "jc-chart-bar" }));
      });
    }
    drawing.appendChild(svg("text", { x: 0, y: 10, class: "jc-chart-text" }, shown.value + " " + Number(high.toPrecision(4))));
    drawing.appendChild(svg("text", { x: 0, y: base + 12, class: "jc-chart-text" }, shown.points[0].label));
    drawing.appendChild(svg("text", { x: W, y: base + 12, "text-anchor": "end", class: "jc-chart-text" }, shown.points[shown.points.length - 1].label));
    return drawing;
  }

  // A data round's rows as a table with the dataset and download links (T-4140).
  // The renderer on viz.{domain} draws maps and charts (T-4657, ADR-N-053 §2.9): each frame gets
  // its spec by message to that origin alone, and only its own messages size it.
  var VIZ = JCRender.vizOrigin(config.viz);
  var vizFrames = [];
  function vizFrame(payload, title, broken) {
    var frame = el("iframe", "jc-viz");
    frame.title = title;
    frame.setAttribute("sandbox", "allow-scripts allow-same-origin allow-downloads allow-popups allow-popups-to-escape-sandbox");
    frame.setAttribute("referrerpolicy", "no-referrer");
    var message = { type: "jc-viz-show", spec: JCRender.vizSpec(payload.spec, lang, payload.labels), rows: payload.rows, lang: lang };
    frame.addEventListener("load", function () {
      frame.contentWindow.postMessage(message, VIZ);
    });
    frame.src = VIZ + "/#show";
    vizFrames.push({ frame: frame, broken: broken });
    return frame;
  }
  window.addEventListener("message", function (event) {
    if (!VIZ || event.origin !== VIZ) return;
    var hit = vizFrames.filter(function (f) { return f.frame.contentWindow === event.source; })[0];
    var data = event.data || {};
    if (!hit) return;
    if (data.type === "jc-viz-height" && typeof data.height === "number") {
      hit.frame.style.height = Math.min(Math.max(Math.round(data.height), 120), 900) + "px";
    }
    if (data.type === "jc-viz-error") hit.broken();
  });

  // A chart the model asked for, drawn from a table's rows by the server's code (T-4657).
  function showVisualization(item, data) {
    if (!VIZ || !data || !data.spec || typeof data.spec !== "object") return;
    var figure = el("figure", "jc-data jc-viz-card");
    var title = typeof data.spec.title === "string" && data.spec.title ? data.spec.title : w.chart;
    figure.appendChild(vizFrame(data, title, function () { figure.remove(); }));
    visual(item, figure);
  }

  // "Table | Map" over a data card whose rows have places; the map loads when it is first shown,
  // so it never holds up the answer, and a map that fails leaves the table (T-4657).
  function mapTabs(figure, scroll, drawn, data) {
    var tabs = el("div", "jc-tabs");
    tabs.setAttribute("role", "tablist");
    var box = el("div", "jc-viz-box");
    var tab = function (text) {
      var b = tabs.appendChild(el("button", "jc-tab", text));
      b.type = "button";
      b.setAttribute("role", "tab");
      return b;
    };
    var toTable = tab(w.tabTable);
    var toMap = tab(w.tabMap);
    var pick = function (map) {
      toTable.setAttribute("aria-selected", String(!map));
      toMap.setAttribute("aria-selected", String(map));
      scroll.hidden = map;
      if (drawn) drawn.hidden = map;
      box.hidden = !map;
      if (map && !box.firstChild) {
        box.appendChild(vizFrame({ spec: data.map }, w.tabMap, function () {
          tabs.remove();
          box.remove();
          pick(false);
        }));
      }
    };
    toTable.addEventListener("click", function () { pick(false); });
    toMap.addEventListener("click", function () { pick(true); });
    figure.insertBefore(tabs, scroll);
    figure.insertBefore(box, scroll);
    pick(JCRender.mapFirst(data));
  }

  // An answer's visuals sit under its text (T-4682): one box the text is put before, and with more
  // than one visual each is folded to its caption.
  function visual(item, figure) {
    if (!item.visuals) {
      item.visuals = el("div", "jc-visuals");
      item.insertBefore(item.visuals, item.status && item.status.parentNode === item ? item.status : null);
    }
    item.visuals.appendChild(figure);
    var folds = item.visuals.querySelectorAll("details.jc-data");
    if (item.visuals.children.length > 1) Array.prototype.forEach.call(folds, function (d) { d.open = false; });
  }

  function showData(item, data) {
    var shown = JCRender.table(data, lang);
    if (!shown || !JCRender.firstTable(item.tables = item.tables || [], data)) return;
    var figure = el("details", "jc-data");
    figure.open = true;
    figure.appendChild(el("summary", "", shown.caption || w.table));
    var scroll = el("div", "jc-data-scroll");
    scroll.tabIndex = 0;
    scroll.setAttribute("role", "region");
    scroll.setAttribute("aria-label", w.table);
    var table = el("table");
    if (shown.caption) table.appendChild(el("caption", "jc-sr", shown.caption));
    var headRow = el("tr");
    shown.columns.forEach(function (c, i) {
      var th = el("th", shown.numeric[i] ? "jc-num" : "", c);
      th.scope = "col";
      headRow.appendChild(th);
    });
    table.appendChild(el("thead")).appendChild(headRow);
    var body = table.appendChild(el("tbody"));
    shown.rows.forEach(function (row) {
      var tr = body.appendChild(el("tr"));
      row.forEach(function (cell, i) {
        // A long text keeps two lines; the whole of it is its title.
        var td = tr.appendChild(el("td", shown.numeric[i] ? "jc-num" : "", ""));
        // Below 600 px a row is a card and each value carries its column's name (T-4711).
        td.setAttribute("data-label", shown.columns[i]);
        if (cell && typeof cell === "object") {
          // A long address reads as one short word, the whole of it its title (T-4682).
          var a = td.appendChild(link(cell.href, cell.text));
          a.title = cell.href;
          return;
        }
        var text = td.appendChild(el("span", "jc-cell", cell));
        if (cell.length > 40) text.title = cell;
      });
    });
    scroll.appendChild(table);
    figure.appendChild(scroll);
    var drawn = plot(JCRender.chart(data));
    if (drawn) figure.appendChild(drawn);
    if (VIZ && data.map && typeof data.map === "object") mapTabs(figure, scroll, drawn, data);
    if (shown.dataset || shown.download || shown.geojson) {
      var links = el("p", "jc-data-links");
      // More rows than the panel shows: the dataset holds them all (T-4682).
      var more = typeof data.total === "number" && data.total > shown.rows.length;
      if (shown.dataset) links.appendChild(link(shown.dataset, more ? w.allRows : w.dataset));
      if (shown.download) links.appendChild(link(shown.download, w.download));
      // One area's own outline, as the Endpoint serves it (T-4697).
      if (shown.geojson) links.appendChild(link(shown.geojson, w.geojson));
      figure.appendChild(links);
    }
    visual(item, figure);
    item.data = figure;
  }

  // A failed answer as one quiet line in the visitor's language and a Retry that asks the same
  // question again, through the same route and its per-visitor limit (T-4253); a limit offers no
  // Retry, since nothing changes before it resets (T-4671).
  function fail(item, word, detail) {
    if (item.status) item.status.remove();
    if (item.streaming) {
      item.streaming.remove();
      item.streaming = null;
    }
    item.classList.add("jc-error");
    var p = el("p", "", JCRender.said(word, detail, lang, w));
    p.setAttribute("role", "alert");
    item.appendChild(p);
    if (word === "full") {
      // A used-up conversation offers a new one that asks the same question again (T-4660).
      var resend = JCRender.retry(history);
      if (resend) {
        var anew = button("jc-retry", "fresh");
        anew.addEventListener("click", function () {
          if (busy) return;
          fresh.click();
          submit(resend.question);
        });
        item.appendChild(anew);
      }
      return;
    }
    if (!JCRender.retryable(word)) return;
    var again = button("jc-retry", "retry");
    again.addEventListener("click", function () {
      var resend = JCRender.retry(history);
      if (!resend || busy) return;
      history = resend.history;
      var question = item.previousSibling;
      if (question && question.classList.contains("jc-you")) question.remove();
      item.remove();
      submit(resend.question);
    });
    item.appendChild(again);
  }

  function handle(item, name, data) {
    if (name === "conversation") {
      // The server's moment of the question, the one the answer is told, replaces this
      // browser's guess, so the time beside the question is the time the answer states (T-4339).
      var at = JCRender.stamp(data.at, lang);
      if (at && item.askedTime) {
        item.askedTime.textContent = at.text;
        item.askedTime.setAttribute("datetime", at.datetime);
        item.askedTime.title = at.title;
        item.askedAt = data.at;
      }
      if (data.id !== conversation) asked = 0;
      conversation = data.id;
      asked += 1;
      item.turn = asked;
      // The turn a page change resumes (T-4685, API/05 §1.6.2).
      if (typeof data.turn === "string") item.resumable = data.turn;
    } else if (name === "stage" && item.status) {
      (item.stages = item.stages || []).push(Object.assign({}, data, { ms: Date.now() - (item.startedMs || Date.now()) }));
      var line = JCRender.stage(data, lang);
      if (line) {
        item.status.textContent = line;
        item.staged = true;
      }
    } else if (name === "tool" && data.status === "started" && item.status && !item.staged) {
      // A visitor's stream names no Endpoint (T-4654), and a tool's own name is no word for them.
      item.status.textContent = w.reading + (data.endpoint ? " " + data.endpoint : "") + "…";
    } else if (name === "script") {
      var details = el("details", "jc-script");
      details.appendChild(el("summary", "", w.script));
      details.appendChild(el("pre", "", data.code));
      details.appendChild(el("pre", "", data.output !== undefined ? data.output : data.error));
      item.appendChild(details);
    } else if (name === "data") {
      showData(item, data);
    } else if (name === "visualization") {
      showVisualization(item, data);
    } else if (name === "delta" && typeof data.text === "string") {
      if (item.status) {
        item.status.remove();
        item.status = null;
      }
      if (!item.streaming) {
        item.streaming = el("div", "jc-answer jc-streaming");
        item.insertBefore(item.streaming, item.visuals || null);
      }
      item.text = (item.text || "") + data.text;
      if (!item.frame) item.frame = requestAnimationFrame(function () { paint(item); });
    } else if (name === "answer") {
      // Shown with its citations, which come next; `ask` shows it alone if they never do.
      if (item.status) {
        item.status.remove();
        item.status = null;
      }
      item.pending = String(data.text);
      history.push({ role: "assistant", text: item.pending.slice(0, 4000) });
    } else if (name === "citations") {
      var cited = Array.isArray(data) ? data : [];
      item.cited = cited;
      show(item, item.pending || "", cited, item.turn);
      stepsPanel(item);
      saved(item, item.pending || "", cited);
      item.pending = null;
    } else if (name === "done" && data && data.stagesMs) {
      // The stages by name and duration replace the progress lines (T-4729).
      item.timed = data.stagesMs;
      if (item.cited) stepsPanel(item);
    } else if (name === "step") {
      // A read of the answer, listed under it by its source, query and count (T-4801).
      (item.reads = item.reads || []).push(data);
    } else if (name === "followups") {
      chips(item, JCRender.followups(data));
      // Kept with the answer, so a page change shows them again under the last one (T-4685).
      var last = kept && kept.turns[kept.turns.length - 1];
      if (last && last.turn === item.turn && conversation === kept.conversation) {
        last.followups = JCRender.followups(data);
        keep();
      }
    } else if (name === "error") {
      fail(item, JCRender.trouble(data.status, data.title), data.detail);
    }
  }

  // The collapsed list of the steps an answer took, made again when its stage times arrive.
  function stepsPanel(item) {
    var took = JCRender.steps(item.stages, lang, item.timed, item.reads);
    if (item.stepsList) item.stepsList.remove();
    item.stepsList = null;
    if (!took) return;
    var list = el("details", "jc-steps");
    list.appendChild(el("summary", "", took.summary));
    var ol = el("ol");
    took.lines.forEach(function (line, i) {
      var li = ol.appendChild(el("li", "", line));
      var page = took.links && took.links[i];
      if (page) {
        li.appendChild(document.createTextNode(" "));
        li.appendChild(link(page, w.dataset));
      }
    });
    list.appendChild(ol);
    item.insertBefore(list, item.chips || null);
    item.stepsList = list;
  }

  // The answered turn into the panel's sessionStorage, with the question it answered.
  function saved(item, text, citations) {
    if (!conversation || !item.turn) return;
    kept = JCRender.remember(kept, conversation, asked,
      { question: item.question || "", text: text, turn: item.turn, citations: citations,
        askedAt: item.askedAt, at: new Date().toISOString() });
    keep();
  }

  // The server's detail is English and for the logs; the visitor reads the widget's words.
  function refusal(status, title) {
    return JCRender.trouble(status, title);
  }

  // The visitor token every question carries (T-4653, API/05 §1.2.1): a Web Worker proves the
  // service's challenge as soon as the chat opens, so the first question seldom waits for it.
  var visitor = null;
  function visitorToken(fresh) {
    if (fresh || !visitor) {
      visitor = earn().catch(function (error) {
        visitor = null;
        throw error;
      });
    }
    return visitor;
  }

  async function earn() {
    var base = api + "/api/v1/d/" + encodeURIComponent(config.publicId);
    var asked = await fetch(base + "/challenge", { credentials: "omit" });
    if (!asked.ok) throw new TypeError("no challenge");
    var challenge = await asked.json();
    var nonce = await new Promise(function (done, failed) {
      var worker = new Worker((config.assets || "/d/") + "visitor.js");
      worker.onmessage = function (event) { worker.terminate(); done(event.data); };
      worker.onerror = function () { worker.terminate(); failed(new TypeError("no proof")); };
      worker.postMessage({ challenge: challenge.challenge, difficulty: challenge.difficulty });
    });
    var earned = await fetch(base + "/token", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "omit",
      body: JSON.stringify({ challenge: challenge.challenge, nonce: nonce })
    });
    if (!earned.ok) throw new TypeError("no token");
    var token = await earned.json();
    return { token: token.token, until: Date.parse(token.expiresAt) || 0 };
  }

  async function ask(question, item, signal) {
    var body = { message: question, history: history.slice(-6) };
    if (conversation) body.conversation = conversation;
    if (toggles.length > 0) {
      body.connectors = toggles.filter(function (b) { return b.checked; }).map(function (b) { return b.value; });
    }
    history.push({ role: "user", text: question });
    var post = function (held) {
      return fetch(api + "/api/v1/d/" + encodeURIComponent(config.publicId) + "/chat", {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "text/event-stream", "X-JC-Visitor": held.token },
        credentials: "omit",
        body: JSON.stringify(body),
        signal: signal
      });
    };
    var held = await visitorToken();
    if (Date.now() > held.until - 60000) held = await visitorToken(true);
    var response = await post(held);
    // A token the service no longer knows (a restart, an hour gone) is earned again, once.
    if (response.status === 401) response = await post(await visitorToken(true));
    await read(response, item);
  }

  // A resumed answer (T-4685): every event of the turn again, into an item whose text starts
  // empty, so nothing shows twice and nothing is asked again.
  async function replay(id, item, signal) {
    var held = await visitorToken();
    var response = await fetch(api + "/api/v1/d/" + encodeURIComponent(config.publicId) + "/turns/" +
      encodeURIComponent(id) + "/resume?after=0", {
      headers: { Accept: "text/event-stream", "X-JC-Visitor": held.token },
      credentials: "omit",
      signal: signal
    });
    await read(response, item);
  }

  // The answer's events as they stream, then its final view and, when it is about one dataset,
  // the host is asked to open it.
  async function read(response, item) {
    if (!response.ok || !response.body) {
      var problem = await response.json().catch(function () { return {}; });
      fail(item, refusal(response.status, problem.title), problem.detail);
      return;
    }
    var reader = response.body.getReader();
    var decoder = new TextDecoder();
    var buffer = "";
    for (;;) {
      var chunk = await reader.read();
      if (chunk.done) break;
      var parsed = JCRender.events(buffer + decoder.decode(chunk.value, { stream: true }));
      buffer = parsed.rest;
      parsed.events.forEach(function (event) { handle(item, event.name, event.data); });
    }
    if (item.pending !== null && item.pending !== undefined) {
      show(item, item.pending, [], item.turn);
      saved(item, item.pending, []);
      item.pending = null;
    } else if (item.streaming) {
      show(item, item.text, [], item.turn);
    }
    log.scrollTop = log.scrollHeight;
    follow(item);
  }

  // T-4685, API/05 §1.6.1: a done answer whose citations link one dataset, an empty input and
  // the visitor's switch on; the host checks the rest and answers `jc-assistant-opened`.
  var opening = null;
  function follow(item) {
    var target = docking && following() && input.value.trim() === "" ? JCRender.dataset(item.cited) : null;
    if (!target) return;
    opening = { item: item, target: target, at: Date.now() };
    toParent({ type: "jc-assistant-dataset", url: target.url, title: target.title });
  }
  function openedLine(item, target) {
    var name = function () { return target.title || target.url; };
    var line = el("p", "jc-opened");
    line.appendChild(words(el("span"), function () { return w.opened.replace("{title}", name()) + " · "; }));
    var back = button("jc-more", "back");
    back.addEventListener("click", function () { toParent({ type: "jc-assistant-back" }); });
    line.appendChild(back);
    item.appendChild(line);
  }
  function noteOpened(done) {
    (kept ? kept.turns : []).forEach(function (t) { if (t.turn === done.item.turn) t.opened = done.target; });
    keep();
  }
  window.addEventListener("message", function (event) {
    var data = event.data || {};
    if (!embedded || event.source !== window.parent || data.type !== "jc-assistant-opened" ||
        !opening || data.url !== opening.target.url ||
        JCRender.closeTargets(location.ancestorOrigins, config.frameOrigins).indexOf(event.origin) < 0) return;
    var done = opening;
    opening = null;
    openedLine(done.item, done.target);
    noteOpened(done);
    announce.textContent = w.changed.replace("{title}", done.target.title || done.target.url);
  });

  function settle() {
    busy = null;
    send.hidden = false;
    stop.hidden = true;
    input.focus();
  }

  function submit(question) {
    question = String(question || "").trim();
    if (!question || busy) return;
    greeted();
    Array.prototype.forEach.call(log.querySelectorAll(".jc-followups"), function (l) { l.remove(); });
    var mine = turn("jc-you", "you");
    mine.appendChild(el("p", "", question));
    var askedAt = new Date().toISOString();
    var time = when(askedAt);
    if (time) mine.appendChild(time);
    input.value = "";
    var item = turn("jc-bot", "assistant");
    item.question = question;
    item.askedAt = askedAt;
    item.askedTime = time;
    item.status = el("p", "jc-status", w.thinking);
    item.status.setAttribute("role", "status");
    item.appendChild(item.status);
    item.startedMs = Date.now();
    run(item, function (signal) { return ask(question, item, signal); });
  }

  // One answer at a time, `start` reading it; Stop keeps what arrived.
  var live = null;
  function run(item, start) {
    busy = new AbortController();
    live = item;
    send.hidden = true;
    stop.hidden = false;
    var controller = busy;
    start(controller.signal).catch(function (error) {
      if (controller.signal.aborted) {
        // What arrived stays; the visitor sees that they stopped it.
        if (item.status) item.status.remove();
        if (item.streaming) show(item, item.text, []);
        item.appendChild(el("p", "jc-status", w.stopped));
        return;
      }
      fail(item, error instanceof TypeError ? "offline" : "failed");
    }).then(function () {
      if (busy === controller) settle();
    });
  }

  function closePanel() {
    // The launcher on the host page closes the panel and puts focus back on its button; the
    // message carries no data.
    toParent({ type: "jc-assistant-close" });
  }

  // The message goes to the framing page's origin alone, never `*` (T-4654).
  function toParent(message) {
    if (!embedded) return;
    JCRender.closeTargets(location.ancestorOrigins, config.frameOrigins).forEach(function (origin) {
      window.parent.postMessage(message, origin);
    });
  }

  // The launcher sizes the panel; the state lives in this tab's sessionStorage (T-4658):
  // `full`, `dock` (T-4685) or `panel`. Collapsed, a docking host docks it again.
  function resize(full) {
    place(full ? "full" : docked ? "dock" : "panel");
  }
  function place(mode) {
    expanded = mode === "full";
    if (!expanded) docked = mode === "dock";
    try { sessionStorage.setItem("jc-chat-size:" + config.publicId, mode); } catch (e) { /* this open only */ }
    if (size) {
      size.replaceChild(icon(expanded ? "collapse" : "expand"), size.firstChild);
      size.setAttribute("aria-pressed", String(expanded));
    }
    if (dock) {
      dock.replaceChild(icon(docked ? "undock" : "dock"), dock.firstChild);
      dock.setAttribute("aria-pressed", String(docked));
      dock.hidden = expanded;
    }
    relabel(lang);
    toParent({ type: "jc-assistant-size", size: mode });
  }

  form.addEventListener("submit", function (event) {
    event.preventDefault();
    submit(input.value);
  });
  stop.addEventListener("click", function () {
    if (busy) busy.abort();
  });
  fresh.addEventListener("click", function () {
    if (busy) busy.abort();
    conversation = null;
    history = [];
    turns = 0;
    asked = 0;
    kept = null;
    keep();
    log.textContent = "";
    announce.textContent = "";
    busy = null;
    settle();
    welcome();
  });
  input.addEventListener("keydown", function (event) {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      form.requestSubmit();
    }
  });
  document.addEventListener("keydown", function (event) {
    // Expanded, Escape first brings the panel back to its size; then it closes.
    // The privacy dialog closes itself on Escape; the panel stays.
    if (about.open) return;
    if (event.key === "Escape" && expanded) resize(false);
    else if (event.key === "Escape") closePanel();
  });

  relabel(lang);
  if (expanded || docking) toParent({ type: "jc-assistant-size", size: expanded ? "full" : docked ? "dock" : "panel" });
  // The conversation open before a page change on the host site, shown again as it was.
  kept = config.enabled === false ? null : kept0();
  if (kept && kept.turns.length > 0) {
    conversation = kept.conversation;
    asked = kept.asked;
    // As the panel looked: the greeting above, no suggestions (T-4685).
    log.textContent = "";
    welcome();
    greeted();
    kept.turns.forEach(function (t, i) {
      var mine = turn("jc-you", "you");
      mine.appendChild(el("p", "", t.question));
      var time = when(t.askedAt);
      if (time) mine.appendChild(time);
      var answered = turn("jc-bot", "assistant");
      show(answered, t.text, t.citations, t.turn, t.at || null);
      if (t.opened) openedLine(answered, t.opened);
      if (t.followups && i === kept.turns.length - 1) chips(answered, t.followups);
      history.push({ role: "user", text: t.question });
      history.push({ role: "assistant", text: t.text });
    });
    announce.textContent = "";
  }
  if (config.enabled === false) {
    // A paused deployment says so instead of offering a box that would only refuse.
    log.textContent = "";
    var paused = el("li", "jc-intro");
    paused.appendChild(words(el("p", "jc-greeting"), "paused"));
    log.appendChild(paused);
    form.hidden = true;
    fresh.hidden = true;
  }
  // The draft, its caret, the scroll and an answer still streaming, as they were before a page
  // change on the host site (T-4685, API/05 §1.6.1); the answer goes on through its resume.
  var VIEW = "jc-chat-view:" + config.publicId;
  var view = null;
  try {
    view = JCRender.view(sessionStorage.getItem(VIEW));
    sessionStorage.removeItem(VIEW);
  } catch (e) { /* storage off */ }
  if (view && config.enabled !== false) {
    var resumed = view.streaming;
    if (resumed && (!kept || kept.conversation === resumed.conversation)) resume(resumed);
    input.value = view.draft;
    grow();
    input.setSelectionRange(view.caret, view.caret);
    if (view.scroll !== null) log.scrollTop = view.scroll;
    if (view.focused) input.focus();
  }
  window.addEventListener("pagehide", function () {
    var now = { draft: input.value, caret: input.selectionStart, scroll: log.scrollTop, focused: document.activeElement === input };
    if (busy && live && live.resumable && conversation) {
      now.streaming = { turn: live.resumable, conversation: conversation, number: live.turn, question: live.question,
        text: live.text || "", askedAt: live.askedAt };
    }
    try { sessionStorage.setItem(VIEW, JSON.stringify(now)); } catch (e) { /* not kept */ }
    // The host answers `jc-assistant-opened` and changes the page at once, so the page can go
    // before that answer arrives; a page that goes within 2 s of the request is that change.
    if (opening && Date.now() - opening.at < 2000) noteOpened(opening);
  });
  function resume(s) {
    greeted();
    conversation = s.conversation;
    asked = Math.max(asked, s.number);
    var mine = turn("jc-you", "you");
    mine.appendChild(el("p", "", s.question));
    var time = when(s.askedAt);
    if (time) mine.appendChild(time);
    history.push({ role: "user", text: s.question });
    var item = turn("jc-bot", "assistant");
    item.question = s.question;
    item.askedAt = s.askedAt;
    item.turn = s.number;
    item.resumable = s.turn;
    item.startedMs = Date.now();
    if (s.text) {
      item.streaming = el("div", "jc-answer jc-streaming");
      item.appendChild(item.streaming);
      item.text = s.text;
      paint(item);
    }
    item.text = "";
    run(item, function (signal) { return replay(s.turn, item, signal); });
  }
  if (config.enabled) visitorToken().catch(function () {});
  if (JCRender.asksLanguage({ mode: mode, offered: offered, stored: stored, kept: Boolean(kept && kept.turns.length > 0),
    enabled: config.enabled, page: page, requested: params.get("lang") })) {
    choose();
  }

  // T-4283 item 13: on the first open the panel asks for the language before anything else,
  // Slovak first and focused; the answer is kept, so later opens go straight to the chat.
  function choose() {
    log.textContent = "";
    intro = null;
    root.classList.add("jc-choosing");
    var box = el("li", "jc-intro jc-choose");
    var heading = el("p", "jc-choose-title");
    heading.id = "jc-choose-title";
    heading.textContent = offered.map(function (code) { return WORDS[code].choose; })
      .filter(function (text, i, all) { return all.indexOf(text) === i; }).join(" / ");
    var group = el("div", "jc-choose-list");
    group.setAttribute("role", "group");
    group.setAttribute("aria-labelledby", heading.id);
    offered.forEach(function (code) {
      var b = el("button", "", NATIVE[code] || code.toUpperCase());
      b.type = "button";
      b.lang = code;
      b.addEventListener("click", function () {
        storeLanguage(code);
        root.classList.remove("jc-choosing");
        relabel(code);
        input.focus();
      });
      group.appendChild(b);
    });
    box.appendChild(heading);
    box.appendChild(group);
    log.appendChild(box);
    group.firstChild.focus();
  }
})();
