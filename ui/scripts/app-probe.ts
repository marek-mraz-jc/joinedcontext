/**
 * T-2795: what the App probe saw of one published App, as the verdict the board and the Apps
 * page read (AP-136).
 *
 * `e2e/live/app-probe.spec.ts` gathers an `Observation` per App and writes `summaryOf` as the
 * summary of the check `apps`: one result per App, keyed `{project}/{name}`, which
 * `tasks/file-failures` files tasks from and `scripts/publish-health.py --passed` publishes for
 * the chips. Titles are what a person reads on the chip, so they name the step that failed and
 * never carry a password or a token; console messages are cut to one line.
 */

export interface Observation {
  project: string;
  name: string;
  visibility: string;
  /** The probe itself was refused the App: it is not in the App's default group. */
  refused: boolean;
  /** Milliseconds until the first row read inside the Portal, or null when none came. */
  dataMs: number | null;
  /** The same on the App's own host, opened from "Open in new window". */
  windowDataMs: number | null;
  consoleErrors: string[];
  /** Responses of 400 and above its own window received, as `status url` (T-3580). */
  failedRequests: string[];
  /** Its own window laid out at each of `WIDTHS`; empty when it has no own address. */
  layout: Layout[];
  /** Data items its own window's first view shows at 1440 px (`firstViewItems`), null unmeasured. */
  items: number | null;
  /** What a visitor who did not sign in got: rows, a sign-in or refusal, or neither. */
  anonymous: "data" | "refused" | "blank";
}

/** One width of an App's own window: how many h1 it shows and whether the page scrolls sideways. */
export interface Layout {
  width: number;
  h1: number;
  sideways: boolean;
}

/** Phone, tablet, laptop and wide screen: the widths every App is laid out at (T-3580). */
export const WIDTHS = [375, 768, 1440, 2560] as const;

/**
 * How many data items a page's first view shows: table rows with content, map markers, chart
 * marks, elements an App marks `data-item`, stats above zero, and each drawn canvas (a WebGL map
 * or a canvas chart) as one. No interaction, no layout: what the page rendered from the data it
 * read. Self-contained, so the probe can hand it to `page.evaluate` as it is.
 * ponytail: a canvas counts whatever it draws, so a basemap with no features passes here; the
 * probe's row check (`firstRow`) is what proves data arrived. Read the canvas's features when a
 * blank-but-drawn map slips through.
 */
export function firstViewItems(root: Document = document): number {
  const rows = [...root.querySelectorAll("tbody tr, [role=row]")].filter(
    (row) => row.querySelector("th, [role=columnheader]") === null && (row.textContent ?? "").trim() !== "",
  ).length;
  const marks = root.querySelectorAll(
    ".leaflet-marker-icon, .leaflet-interactive, .maplibregl-marker, .recharts-bar-rectangle, .recharts-dot, .recharts-sector, [data-item]",
  ).length;
  const stats = [...root.querySelectorAll("[data-stat], dd, output")].filter(
    (stat) => Number.parseFloat((stat.textContent ?? "").replace(/\s/g, "").replace(",", ".")) > 0,
  ).length;
  const canvases = [...root.querySelectorAll("canvas")].filter((canvas) => canvas.width > 0 && canvas.height > 0).length;
  return rows + marks + stats + canvases;
}

export interface ProbeResult {
  key: string;
  verdict: "pass" | "fail" | "skip";
  title: string;
  detail?: string;
}

export interface ProbeSummary {
  check: "apps";
  repo: "joinedcontext-portal";
  run: string;
  requirements: string[];
  results: ProbeResult[];
}

/** How long the probe waits for a first row, in seconds; the titles name it. */
export const DATA_WAIT_S = 60;

const oneLine = (text: string, max = 120): string => {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
};

export function verdictOf(seen: Observation): ProbeResult {
  const key = `${seen.project}/${seen.name}`;
  const fail = (title: string, detail?: string): ProbeResult => ({ key, verdict: "fail", title, detail });
  if (seen.refused) {
    return { key, verdict: "skip", title: `the probe is not a member of ${seen.name}'s default group` };
  }
  if (seen.dataMs === null) return fail(`no row read inside the Portal in ${DATA_WAIT_S} s`);
  if (seen.windowDataMs === null) return fail(`no row read in its own window in ${DATA_WAIT_S} s`);
  if (seen.consoleErrors.length > 0) {
    const count = seen.consoleErrors.length;
    return fail(
      `${count} console error${count === 1 ? "" : "s"}: ${oneLine(seen.consoleErrors[0])}`,
      seen.consoleErrors.map((error) => oneLine(error, 300)).join("\n"),
    );
  }
  if (seen.failedRequests.length > 0) {
    const count = seen.failedRequests.length;
    return fail(
      `${count} failed request${count === 1 ? "" : "s"}: ${oneLine(seen.failedRequests[0])}`,
      seen.failedRequests.map((request) => oneLine(request, 300)).join("\n"),
    );
  }
  const headings = seen.layout.find((at) => at.h1 !== 1);
  if (headings) return fail(`${headings.h1} h1 at ${headings.width} px, one expected`);
  const sideways = seen.layout.find((at) => at.sideways);
  if (sideways) return fail(`scrolls sideways at ${sideways.width} px`);
  if (seen.items === 0) return fail("its first view shows no data item");
  const isPublic = seen.visibility === "public";
  if (isPublic && seen.anonymous !== "data") return fail("a visitor who did not sign in read nothing");
  if (!isPublic && seen.anonymous === "data") return fail("a visitor who did not sign in read its data");
  if (!isPublic && seen.anonymous === "blank") return fail("a visitor who did not sign in was not sent to sign in");
  return { key, verdict: "pass", title: `opened with data in ${(seen.dataMs / 1000).toFixed(1)} s` };
}

export function summaryOf(observations: Observation[], run: string): ProbeSummary {
  return {
    check: "apps",
    repo: "joinedcontext-portal",
    run,
    requirements: ["AP-136", "AP-34", "SDK-39"],
    results: observations.map(verdictOf),
  };
}
