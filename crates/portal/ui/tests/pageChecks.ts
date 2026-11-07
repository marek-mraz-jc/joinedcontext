/**
 * T-2730: what every page of the Portal owes a person, as one check over the rendered DOM
 * (UI-01, UI-15, UI-16, UI-44, UI-84).
 *
 * Two walkers run it: `tests/walker_pages.test.tsx` over every address of the router with the
 * API mocked, and `e2e/live/walker.spec.ts` on dev at 1440 and 2560 px. The function is written
 * to be handed to `page.evaluate` as it is, so it holds everything it uses inside its own body
 * and imports nothing: Playwright sends its source to the browser, not its closure.
 *
 * Each finding is `check: detail`, and the check names are what the walkers' allow lists name.
 */

export interface PageCheckOptions {
  /** The top-level keys of `en.json`: a text that is one of their dotted paths is an untranslated key. */
  namespaces: string[];
  /** Judge what needs a layout engine (overflow, form columns); jsdom has none. */
  layout: boolean;
}

export type PageCheck =
  // The Portal never drew the page (index.html's fallback, T-4201): nothing else can be judged.
  | "boot"
  | "one-h1"
  | "purpose-line"
  | "empty-state-next-step"
  | "field-hint"
  | "error-text"
  | "raw-i18n-key"
  | "raw-json"
  | "bare-urn"
  | "overflow"
  | "form-columns"
  | "row-link"
  // What the walkers add around the DOM check: axe's serious and critical rules, and (live)
  // the console, a request answering >= 400, and a write control a viewer can press.
  | "axe"
  | "console"
  | "http"
  | "viewer-write";

/**
 * How long the walker waits before it loads a page the edge refused with 429 again: the seconds the
 * refusal's `Retry-After` names plus one, kept between 2 and 61 s; no or a garbled header waits 20 s,
 * a third of the edge's minute window (T-5078).
 */
export function edgeRetryMs(retryAfter: string | undefined): number {
  const seconds = (retryAfter ?? "").trim();
  return /^\d+$/.test(seconds) ? (Math.min(Number(seconds), 60) + 1) * 1000 : 20_000;
}

export function pageFindings(options: PageCheckOptions): string[] {
  // The app never replaced index.html's fallback (its script or a chunk did not load, as when the
  // edge answers 429): the rules below would judge the fallback, so the walk says that instead (T-5069).
  const boot = document.getElementById("boot-failed");
  if (boot) return [`boot: the Portal did not load, ${(boot.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 120)}`];
  const found: string[] = [];
  const root = document.querySelector("main") ?? document.body;
  const say = (check: string, detail: string) => found.push(`${check}: ${detail.replace(/\s+/g, " ").trim().slice(0, 160)}`);
  const shown = (element: Element) => {
    // A closed disclosure's body keeps its boxes in Chromium (content-visibility), yet nobody sees it.
    if (element.closest("[hidden], [aria-hidden='true'], .sr-only, details:not([open]) > :not(summary)")) return false;
    if (!options.layout) return true;
    const style = getComputedStyle(element);
    return style.display !== "none" && style.visibility !== "hidden" && element.getClientRects().length > 0;
  };
  const text = (element: Element | null) => (element?.textContent ?? "").replace(/\s+/g, " ").trim();

  // One H1 names the page (UI-16); a second one is a second page in one.
  // Counted behind an open dialog too: a modal hides the page from assistive technology while it
  // is open, and the page's h1 is still the page's.
  const headings = [...document.querySelectorAll("h1")].filter(
    (heading) => !heading.closest("[hidden]") && (!options.layout || heading.getClientRects().length > 0),
  );
  if (headings.length !== 1) say("one-h1", `${headings.length} h1: ${headings.map(text).join(" | ")}`);

  // The line under the title that says what the page manages (PageHeader's description, UI-01).
  const h1 = headings[0];
  if (h1) {
    const next = h1.nextElementSibling;
    // A list page's compact header (T-6095) says it behind an info mark beside the title instead.
    const line = next?.tagName === "P" ? next : next?.querySelector('[role="tooltip"]');
    if (!line || text(line).length < 10) say("purpose-line", `no line under "${text(h1)}"`);
  }

  // An empty list says what to do next: a hint or the action that creates the first item.
  for (const empty of root.querySelectorAll("[data-empty-state]")) {
    if (!shown(empty)) continue;
    const hint = empty.querySelectorAll("p").length > 1;
    const action = empty.querySelector("button, a[href]");
    if (!hint && !action) say("empty-state-next-step", text(empty));
  }

  // Every field of a form carries a one-line hint wired to it (Field's description, UI-16).
  const skip = new Set(["hidden", "submit", "button", "reset", "image"]);
  for (const control of root.querySelectorAll("form input, form select, form textarea, [data-testid='form-page'] input, [data-testid='form-page'] select, [data-testid='form-page'] textarea")) {
    // A search box filters what the page shows; its label is the whole instruction.
    if (!shown(control) || skip.has((control as HTMLInputElement).type) || control.closest("[role='search']")) continue;
    if ((control as HTMLInputElement).type === "search") continue;
    const hinted = (element: Element | null) =>
      (element?.getAttribute("aria-describedby") ?? "")
        .split(/\s+/)
        .filter(Boolean)
        .map((id) => document.getElementById(id))
        .some((hint) => hint !== null && text(hint).length > 0);
    // A box of a list is one choice of its group, and the group carries the one-line hint: the
    // fieldset's description, read once before its boxes (T-3852).
    const box = ["checkbox", "radio"].includes((control as HTMLInputElement).type);
    const described = hinted(control) || (box && hinted(control.closest("fieldset")));
    if (!described) {
      const labels = (control as HTMLInputElement).labels;
      const label =
        control.getAttribute("aria-label") ??
        (labels && labels.length > 0 ? text(labels[0]) : text(control.id ? document.querySelector(`label[for="${CSS.escape(control.id)}"]`) : null));
      say("field-hint", label || control.getAttribute("name") || control.getAttribute("placeholder") || control.outerHTML.slice(0, 100));
    }
  }

  // A refused value says what to change (T-4950): a field marked invalid carries a message wired
  // to it, and the message is more than a bare "Invalid"; a form's own refusal names what to fix,
  // not only that something failed. The field's message names the field by being wired to it.
  const generic = /^(error|invalid( (value|input))?|required|this field is required|failed|request failed|bad request|forbidden|unprocessable( entity)?|something went wrong|\d{3}( [a-z ]+)?)[.!]?$/i;
  const wired = new Set<string>();
  for (const control of root.querySelectorAll("[aria-invalid='true']")) {
    if (!shown(control)) continue;
    const ids = (control.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean);
    ids.forEach((id) => wired.add(id));
    const labels = (control as HTMLInputElement).labels;
    const name = control.getAttribute("aria-label") ?? (labels && labels.length > 0 ? text(labels[0]) : control.getAttribute("name") ?? "a field");
    const messages = ids.map((id) => document.getElementById(id)).filter((hint) => hint?.getAttribute("role") === "alert" || hint?.closest("[role='alert']"));
    const said = messages.map(text).filter(Boolean);
    if (said.length === 0) say("error-text", `${name}: marked invalid with no message`);
    else if (said.every((message) => generic.test(message))) say("error-text", `${name}: "${said.join(" ")}" does not say what to change`);
  }
  for (const alert of root.querySelectorAll("form [role='alert']")) {
    const owner = alert.closest("[id]");
    if (!shown(alert) || (owner !== null && wired.has(owner.id))) continue;
    const said = text(alert);
    if (said && generic.test(said)) say("error-text", `the form says only "${said}"`);
  }

  // What a person reads is words, never the machinery behind them.
  const namespaces = new Set(options.namespaces);
  const code = "pre, code, textarea, kbd, samp, [contenteditable], .monaco-editor, [data-raw]";
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const seen = new Set<string>();
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const value = (node.textContent ?? "").trim();
    const parent = node.parentElement;
    if (value.length < 3 || !parent || parent.closest(code) || !shown(parent)) continue;
    const key = /^([a-z][A-Za-z]+)(\.[A-Za-z0-9_]+)+$/.exec(value);
    if (key && namespaces.has(key[1]) && !seen.has(`k${value}`)) {
      seen.add(`k${value}`);
      say("raw-i18n-key", value);
    }
    if (/^\s*[[{]\s*"|"\s*:\s*[[{"]/.test(value) && !seen.has(`j${value}`)) {
      seen.add(`j${value}`);
      say("raw-json", value);
    }
  }
  // Where a label belongs, a URN is not one (a cell may show an id; a heading, a button, a link,
  // a column header, a tab or a field label names a thing in words).
  for (const label of root.querySelectorAll("h1, h2, h3, button, a, th, label, legend, [role='tab'], dt")) {
    const value = text(label);
    if (/^urn:ngsi-ld:\S+$/.test(value) && shown(label) && !label.closest(code) && !label.closest("td, [role='gridcell']"))
      say("bare-urn", value);
  }

  // Every record of a list opens on a click (T-2875): each row of a table of records
  // (`data-records`) holds its record's link, the one `TableRow` hands a click to. The empty
  // row and the loading skeleton are not records.
  for (const table of root.querySelectorAll("table[data-records]")) {
    if (table.getAttribute("aria-busy") === "true" || !shown(table)) continue;
    for (const row of table.querySelectorAll(":scope > tbody > tr")) {
      const cells = row.querySelectorAll(":scope > td, :scope > th");
      if (cells.length === 1 && Number(cells[0].getAttribute("colspan") ?? "1") > 1) continue;
      if (!row.querySelector("a[data-row-link]")) say("row-link", `${table.querySelector("caption")?.textContent ?? "a table"}: ${text(row)}`);
    }
  }

  if (options.layout) {
    const wide = document.documentElement.scrollWidth - document.documentElement.clientWidth;
    if (wide > 1) say("overflow", `${wide}px sideways`);
    // On a big screen a form uses the width: its field grid shows three columns (T-2713, T-2755).
    if (window.innerWidth >= 1800) {
      for (const form of document.querySelectorAll("[data-testid='form-page'] form, [role='dialog'] form")) {
        const grids = [...form.querySelectorAll("*")].filter(
          (element) => getComputedStyle(element).display === "grid" && element.children.length >= 3,
        );
        const columns = Math.max(0, ...grids.map((grid) => getComputedStyle(grid).gridTemplateColumns.split(" ").length));
        if (grids.length > 0 && columns < 3) say("form-columns", `${columns} column(s) at ${window.innerWidth}px`);
      }
    }
  }
  return found;
}

/** A finding the walkers excuse: a route, a check, and the open task that owes the fix. */
/**
 * The tab shows a blank shell rather than a Portal page: no index.html `#root`, or neither `<html lang>` nor a
 * `<title>` (an edge's 502 page, an empty answer). The walker reports such a load as one `document`
 * finding instead of judging its lang, title and h1 (T-5857). Self-contained for `page.evaluate`.
 */
export function blankDocument(): boolean {
  if (!document.getElementById("root")) return true;
  return !document.documentElement.getAttribute("lang") && document.title.trim() === "";
}

/**
 * Pages of the person's own things, outside any project, where a viewer writes: the connectors
 * they added (UI-98) and their AI connectors (AG-142, T-5951). The server keeps each one inside
 * the person's own rights: a connector reaches only a project the person reads, and its calls
 * read those rights again (connectors.rs `reachable`). The walker skips `viewerWriteFindings` there.
 */
export const OWN_ROUTES: ReadonlySet<string> = new Set(["/connectors", "/ai-connectors"]);

/**
 * A write control the viewer can press, or one refused without a reason (UI-44). A tab is a place
 * to go, and a control that only edits the draft in hand writes nothing: a term added to the filter
 * being tried, a row of the Explore filters (EntityFilters, T-6139), a condition of the grid's filter
 * panel (FilterPanel, T-5005), a sort key or group field of the view in hand (KeyList, T-6427), a
 * colour rule of one's own view, a line of a mapping whose Propose is refused. "Add to my connectors" writes the person's
 * own preferences (UI-98): a viewer may add an Endpoint they read, and the server refuses one they
 * cannot (T-4949, connectors_tests.rs). Self-contained for `page.evaluate`, as `pageFindings` is;
 * without `layout` (jsdom) a control counts as shown.
 */
export function viewerWriteFindings(layout: boolean): string[] {
  const found: string[] = [];
  const draftOnly =
    /^(Add to the query|Add filter|Add condition|Add condition group|Remove condition|Remove group|Remove filter: .+|Add a sort key|Add a group field|Add a colour rule|Add the line for .+ to the mapping|Add to my connectors)$/;
  for (const button of document.querySelectorAll("main button:not([role='tab']), main a[role='button']")) {
    const name = (button.getAttribute("aria-label") ?? button.textContent ?? "").replace(/\s+/g, " ").trim();
    if (!/^(New|Edit|Remove|Delete|Propose|Add|Create|Publish|Approve)\b/.test(name) || draftOnly.test(name)) continue;
    if ((layout && button.getClientRects().length === 0) || button.closest("[hidden], [inert]")) continue;
    const refused = button.getAttribute("aria-disabled") === "true" || (button as HTMLButtonElement).disabled;
    const reason = (button.getAttribute("aria-describedby") ?? "")
      .split(/\s+/)
      .some((id) => (document.getElementById(id)?.textContent ?? "").trim().length > 0);
    if (!refused) found.push(`viewer-write: "${name}" can be pressed`);
    else if (!reason) found.push(`viewer-write: "${name}" is refused without a reason`);
  }
  return found;
}

export interface Excused {
  route: string;
  check: PageCheck;
  task: string;
  why: string;
}

/** The findings of one address that no allow-list entry excuses. */
export function unexcused(route: string, findings: string[], allow: readonly Excused[]): string[] {
  return findings.filter(
    (finding) => !allow.some((entry) => entry.route === route && finding.startsWith(`${entry.check}:`)),
  );
}
