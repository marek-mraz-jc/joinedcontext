// Helsinki events: plain HTML, CSS and JavaScript, no build step (AP-83, T-2597).
//
// Data comes only from the app's own endpoint, named in the `#jc-config` element the Portal
// static host writes into index.html (SDK-02, AP-04), and is read with a same-origin `fetch`
// (AP-11: no other host). The functions above the DOM part are pure and are what
// `node --test test/*.test.mjs` checks.

/** The endpoint this app reads: the one serving Event, else the host's primary. */
export function pickEndpoint(config) {
  const endpoints = Array.isArray(config?.endpoints) ? config.endpoints : [];
  const named = endpoints.find((e) => e?.name === "helsinki-events");
  const serving = endpoints.find((e) => Array.isArray(e?.types) && e.types.includes("Event"));
  return (named ?? serving)?.slug ?? config?.slug ?? null;
}

/** The query for events that have not ended before `from` (NGSI-LD q, a DateTime is unquoted). */
export function eventsUrl(slug, from) {
  const params = new URLSearchParams({ type: "Event", limit: "100" });
  if (from) params.set("q", `endDate>=${from.toISOString().replace(/\.\d{3}Z$/, "Z")}`);
  return `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities?${params}`;
}

/** An attribute's value in the normalized or the simplified form, a LanguageMap in the reader's language. */
export function text(attribute, languages = ["en"]) {
  if (attribute == null) return "";
  if (typeof attribute !== "object") return String(attribute);
  const map = attribute.languageMap;
  if (map && typeof map === "object") {
    for (const language of [...languages, "en", "fi", "sv"]) {
      if (typeof map[language] === "string" && map[language]) return map[language];
    }
    const first = Object.values(map).find((value) => typeof value === "string");
    return first ?? "";
  }
  return attribute.value == null ? "" : String(attribute.value);
}

function date(attribute) {
  const raw = typeof attribute === "object" && attribute !== null ? attribute.value : attribute;
  if (typeof raw !== "string" && !(raw instanceof Date)) return null;
  const parsed = new Date(raw);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

/** One Event entity as the page shows it; `null` for something that is not one. */
export function toView(entity, languages = ["en"]) {
  if (!entity || typeof entity !== "object" || typeof entity.id !== "string") return null;
  const location = entity.location?.value ?? entity.location;
  const coordinates = location?.type === "Point" ? location.coordinates : null;
  const [lon, lat] = Array.isArray(coordinates) ? coordinates.map(Number) : [NaN, NaN];
  const source = text(entity.source);
  return {
    id: entity.id,
    name: text(entity.name, languages) || entity.id,
    description: text(entity.description, languages),
    start: date(entity.startDate),
    end: date(entity.endDate),
    status: text(entity.eventStatus),
    address: text(entity.address),
    point: Number.isFinite(lon) && Number.isFinite(lat) ? { lon, lat } : null,
    source: /^https:\/\//.test(source) ? source : "",
  };
}

const fold = (value) =>
  value
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase();

/**
 * The events a reader asked for, soonest first: overlapping [from, to] (an open side is no
 * limit, an event with no dates is kept) and holding every word of `query` in its name, place or
 * description, ignoring case and accents.
 */
export function filterEvents(events, { from = null, to = null, query = "" } = {}) {
  const words = fold(query).split(/\s+/).filter(Boolean);
  return events
    .filter((event) => {
      const end = event.end ?? event.start;
      const start = event.start ?? event.end;
      if (from && end && end < from) return false;
      if (to && start && start > to) return false;
      const haystack = fold(`${event.name} ${event.address} ${event.description}`);
      return words.every((word) => haystack.includes(word));
    })
    .sort((a, b) => (a.start?.getTime() ?? Infinity) - (b.start?.getTime() ?? Infinity));
}

/** "22 Sep 2026, 10:00 – 23 Sep 2026, 12:00", in the given locale and Helsinki's time zone. */
export function when(event, locale = "en-GB") {
  const format = (d) =>
    d.toLocaleString(locale, { dateStyle: "medium", timeStyle: "short", timeZone: "Europe/Helsinki" });
  if (event.start && event.end) return `${format(event.start)} – ${format(event.end)}`;
  if (event.start) return `from ${format(event.start)}`;
  if (event.end) return `until ${format(event.end)}`;
  return "date not given";
}

/** The events' points in a `width`×`height` box around them, north up, with a margin. */
export function project(events, width, height, margin = 12) {
  const located = events.filter((event) => event.point);
  if (located.length === 0) return [];
  const lons = located.map((e) => e.point.lon);
  const lats = located.map((e) => e.point.lat);
  const [west, east, south, north] = [Math.min(...lons), Math.max(...lons), Math.min(...lats), Math.max(...lats)];
  // Longitude shrinks with latitude; without this Helsinki is drawn twice as wide as it is.
  const squeeze = Math.cos((((south + north) / 2) * Math.PI) / 180);
  const spanX = Math.max((east - west) * squeeze, 1e-6);
  const spanY = Math.max(north - south, 1e-6);
  const scale = Math.min((width - 2 * margin) / spanX, (height - 2 * margin) / spanY);
  const offsetX = (width - spanX * scale) / 2;
  const offsetY = (height - spanY * scale) / 2;
  return located.map((event) => ({
    id: event.id,
    x: offsetX + (event.point.lon - west) * squeeze * scale,
    y: offsetY + (north - event.point.lat) * scale,
  }));
}

/** A date input's value as the start (or, with `endOfDay`, the end) of that day; `null` when empty. */
export function dayOf(value, endOfDay = false) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value ?? "")) return null;
  const day = new Date(`${value}T${endOfDay ? "23:59:59" : "00:00:00"}`);
  return Number.isNaN(day.getTime()) ? null : day;
}

// --- The page. Runs only in a browser; the tests import the functions above. ---

if (typeof document !== "undefined") {
  const SVG = "http://www.w3.org/2000/svg";
  const languages = [...(navigator.languages ?? [navigator.language ?? "en"])].map((l) => l.slice(0, 2));
  const $ = (id) => document.getElementById(id);
  const status = $("status");
  let all = [];
  let selected = null;

  // The reader's own calendar day: sv-SE formats a date as YYYY-MM-DD, what a date input takes.
  $("from").value = new Date().toLocaleDateString("sv-SE");

  function config() {
    try {
      return JSON.parse($("jc-config")?.textContent ?? "{}");
    } catch {
      return {};
    }
  }

  function render() {
    const shown = filterEvents(all, {
      from: dayOf($("from").value),
      to: dayOf($("to").value, true),
      query: $("query").value,
    });
    status.textContent = `${shown.length} of ${all.length} events`;

    const list = $("events");
    list.replaceChildren(
      ...shown.map((event) => {
        const item = document.createElement("li");
        const button = document.createElement("button");
        button.type = "button";
        button.setAttribute("aria-pressed", String(event.id === selected));
        const name = document.createElement("span");
        name.className = "name";
        name.textContent = event.name;
        const time = document.createElement("span");
        time.className = "when";
        time.textContent = when(event);
        // A space between the parts, so a screen reader reads them as words, not one run.
        button.append(name, " ", time);
        if (event.address) {
          const where = document.createElement("span");
          where.className = "where";
          where.textContent = event.address;
          button.append(" ", where);
        }
        button.addEventListener("click", () => select(event.id, true));
        item.append(button);
        return item;
      }),
    );

    const map = $("map");
    map.replaceChildren(
      ...project(shown, 320, 240).map((point) => {
        const dot = document.createElementNS(SVG, "circle");
        dot.setAttribute("cx", point.x.toFixed(1));
        dot.setAttribute("cy", point.y.toFixed(1));
        dot.setAttribute("r", point.id === selected ? "6" : "4");
        if (point.id === selected) dot.setAttribute("class", "selected");
        const label = document.createElementNS(SVG, "title");
        label.textContent = shown.find((event) => event.id === point.id)?.name ?? "";
        dot.append(label);
        dot.addEventListener("click", () => select(point.id, false));
        return dot;
      }),
    );
  }

  function select(id, focus) {
    selected = id;
    const event = all.find((each) => each.id === id);
    const detail = $("detail");
    const heading = document.createElement("h2");
    heading.id = "detail-heading";
    heading.textContent = event ? event.name : "Details";
    const parts = [heading];
    if (event) {
      for (const [label, value] of [
        ["When", when(event)],
        ["Where", event.address],
        ["Status", event.status.replace(/^Event/, "")],
      ]) {
        if (!value) continue;
        const line = document.createElement("p");
        const strong = document.createElement("strong");
        strong.textContent = `${label}: `;
        line.append(strong, value);
        parts.push(line);
      }
      if (event.description) {
        const about = document.createElement("p");
        about.textContent = event.description;
        parts.push(about);
      }
      if (event.source) {
        const link = document.createElement("a");
        link.href = event.source;
        link.rel = "noreferrer noopener";
        link.textContent = "Source register";
        parts.push(link);
      }
    }
    detail.replaceChildren(...parts);
    render();
    if (focus) detail.focus();
  }

  async function load() {
    const slug = pickEndpoint(config());
    if (!slug) {
      status.textContent = "This application has no endpoint to read: the page was not served by the platform.";
      return;
    }
    try {
      const response = await fetch(eventsUrl(slug, dayOf($("from").value)), {
        headers: { Accept: "application/ld+json" },
        credentials: "same-origin",
      });
      if (!response.ok) {
        status.textContent = `The events could not be read (HTTP ${response.status}).`;
        return;
      }
      const body = await response.json();
      all = (Array.isArray(body) ? body : []).map((entity) => toView(entity, languages)).filter(Boolean);
      render();
    } catch {
      status.textContent = "The events could not be read: the network request failed.";
    }
  }

  $("query").addEventListener("input", render);
  $("to").addEventListener("change", render);
  // An earlier start date may reach events the first read left out, so it reads again.
  $("from").addEventListener("change", load);
  document.querySelector("form.filters").addEventListener("submit", (e) => e.preventDefault());
  load();
}
