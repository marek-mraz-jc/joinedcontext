/**
 * The records screen, over what a raw space answers (T-2436, T-2437).
 *
 * `fetch` is what is stubbed and nothing below it, so every case goes through the SDK's own
 * endpoint source and the grid it feeds: the same URLs, the same NGSI-LD bodies and the same
 * refusal handling the published bundle uses.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import type { AccessDocument } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { answer, CITY, REGION } from "./fixtures/records";
import { LOCALES, noteWords, SPACE_OF } from "./locales";
import { NOTE, NOTE_MAX } from "./records";
import { datasetName } from "./series";

const CITY_SLUG = "ovr4ttzywhad2oiogf67n7zyn2g2elfc";
const REGION_SLUG = "7u4ns3cdg2mqlx5gmxhk7rqai6pmokdj";
const sk = LOCALES.sk;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "ngsild-results-count": String(CITY.length) },
  });

const problem = (status: number, detail: string) =>
  new Response(JSON.stringify({ title: "Refused", status, detail }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });

/** What the region's seed grants every member: the rows, read (Policy app-bbsk-zaznamy-1). */
const MEMBER: AccessDocument = {
  permissions: [{ resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }],
  prohibitions: [],
};
/** A steward also holds app-bbsk-zaznamy-2-steward: updateAttrs on the note alone (T-5091). */
const STEWARD: AccessDocument = {
  permissions: [
    ...MEMBER.permissions,
    { resource: { type: "KeyPerformanceIndicator" }, actions: ["updateAttrs"], attributes: [NOTE] },
  ],
  prohibitions: [],
};

let calls: Array<{ path: string; method: string; body?: unknown; init?: RequestInit }>;

/** The cube a chart read asks for (`q=dataSet=="…"`), or `undefined` for the table's page. */
function cubeOf(path: string): string | undefined {
  const q = new URL(path, "http://localhost").searchParams.get("q") ?? "";
  return /dataSet=="([^"]+)"/.exec(q)?.[1];
}

function serving(options: { rows?: () => Response; write?: () => Response; cube?: (dataSet: string) => Response } = {}) {
  return vi.fn(async (path: string, init?: RequestInit) => {
    const method = (init?.method ?? "GET").toUpperCase();
    calls.push({
      path,
      method,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
      init,
    });
    if (method === "PATCH") {
      return options.write ? options.write() : new Response(null, { status: 204 });
    }
    if (path.includes("/entities")) {
      const cube = cubeOf(path);
      if (cube !== undefined && options.cube) return options.cube(cube);
      if (cube !== undefined && !options.rows) {
        return json(answer(CITY).filter((entity) => (entity.dataSet as { value: string }).value === cube));
      }
      return options.rows ? options.rows() : json(answer(CITY));
    }
    throw new Error(`no stub for ${method} ${path}`);
  });
}

function show(
  options: Parameters<typeof serving>[0] = {},
  over: { language?: string; space?: string; slug?: string; access?: AccessDocument } = {},
) {
  vi.stubGlobal("fetch", serving(options));
  const client = stubClient({ access: over.access ?? STEWARD }, {
    slug: over.slug ?? CITY_SLUG,
    orgDomain: "banskabystrica.sk",
    space: over.space ?? SPACE_OF.banskabystrica,
    transport: "origin",
    appName: "banskabystrica-zaznamy",
    language: over.language ?? "sk",
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

const rows = () => screen.getAllByRole("row").filter((row) => within(row).queryAllByRole("gridcell").length > 0);
const noteBox = (index: number) => screen.getAllByRole("textbox", { name: `${sk.grid.edit} ${sk.column[NOTE]}` })[index];

beforeEach(() => {
  calls = [];
  vi.unstubAllGlobals();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the records of one publisher", () => {
  it("shows one row per entity, under this body's own heading", async () => {
    show();
    expect(screen.getByRole("heading", { name: sk.title.banskabystrica, level: 1 })).toBeInTheDocument();
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));
    expect(screen.getByText(sk.source.banskabystrica)).toBeInTheDocument();
    expect(screen.queryByText(sk.title.bbsk)).toBeNull();
  });

  it("shows the region's rows under the region's heading, from the space alone", async () => {
    show({ rows: () => json(answer(REGION, SPACE_OF.bbsk, "bbsk.sk")) }, { space: SPACE_OF.bbsk, slug: REGION_SLUG });
    expect(screen.getByRole("heading", { name: sk.title.bbsk, level: 1 })).toBeInTheDocument();
    await waitFor(() => expect(rows()).toHaveLength(REGION.length));
    // The same bundle, and it reads the region's endpoint because that is what it was served.
    expect(calls[0].path).toContain(`/api/endpoint/${REGION_SLUG}/`);
  });

  it("says on the page why the figures cannot be edited", () => {
    show();
    expect(screen.getByText(sk.readOnlyWhy)).toBeInTheDocument();
  });

  it("opens the note and no other column: a figure has no editable control at all", async () => {
    show();
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));
    // One text box per row, and it is the note's.
    expect(screen.getAllByRole("textbox", { name: `${sk.grid.edit} ${sk.column[NOTE]}` })).toHaveLength(CITY.length);
    for (const column of ["kpiValue", "indicator", "refArea", "refPeriod", "dateObserved"]) {
      expect(
        screen.queryAllByRole("textbox", { name: `${sk.grid.edit} ${sk.column[column]}` }),
        column,
      ).toHaveLength(0);
      expect(screen.queryAllByRole("spinbutton", { name: `${sk.grid.edit} ${sk.column[column]}` }), column).toHaveLength(0);
    }
  });

  it("asks the endpoint for one page of one type, and says how many match", async () => {
    show();
    // The table's reads, not the charts' read of one cube: one page of rows and, beside it, the
    // total in a read of its own (limit 0), so a slow count never holds the rows back (T-6541).
    const table = () =>
      calls
        .filter((call) => call.path.includes("/entities") && !cubeOf(call.path))
        .map((call) => new URL(call.path, "http://localhost").searchParams);
    await waitFor(() => expect(table().some((asked) => asked.get("limit") === "25")).toBe(true));
    await waitFor(() => expect(table().some((asked) => asked.get("limit") === "0")).toBe(true));
    const page = table().find((asked) => asked.get("limit") === "25");
    const counted = table().find((asked) => asked.get("limit") === "0");
    expect(page?.get("type")).toBe("KeyPerformanceIndicator");
    expect(page?.get("count")).not.toBe("true");
    expect(counted?.get("type")).toBe("KeyPerformanceIndicator");
    expect(counted?.get("count")).toBe("true");
  });
});

describe("the table reads as names (T-2966)", () => {
  it("names the indicator and the territory beside their codes, puts the unit on the number and folds the URN", async () => {
    show();
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));
    const table = screen.getByRole("region", { name: sk.records });
    const water = rows().find((row) => within(row).queryByText("3 987,5") !== null);
    expect(water).toBeDefined();
    const cells = within(water as HTMLElement);
    expect(cells.getByText("Spotreba pitnej vody - spolu")).toBeInTheDocument();
    expect(cells.getByText("U03084")).toBeInTheDocument();
    expect(cells.getByText("Banská Bystrica")).toBeInTheDocument();
    expect(cells.getByText(/v tis\. m3 fakturovanej vody/)).toBeInTheDocument();
    // No column of UN/CEFACT codes, and the identifier sits behind its disclosure.
    expect(within(table).queryByText("MTQ")).toBeNull();
    expect(within(table).queryByText("C62")).toBeNull();
    expect(cells.getByText(sk.showId).closest("details")).not.toHaveAttribute("open");
  });
});

describe("the charts above the table (T-2966)", () => {
  it("draws the picked cube's indicator under the office's name, never its code", async () => {
    const user = userEvent.setup();
    show();
    const overview = screen.getByRole("region", { name: sk.overview });
    await user.click(within(overview).getByRole("button", { name: datasetName("vh5003rr", "sk") }));
    const chart = await within(overview).findByRole("figure", { name: /Spotreba pitnej vody - spolu/ });
    expect(within(chart).getByText("Banská Bystrica")).toBeInTheDocument();
    expect(within(overview).queryByText(/U03084/)).toBeNull();
    expect(within(overview).getByRole("button", { name: datasetName("vh5003rr", "sk") })).toHaveAttribute("aria-pressed", "true");
    // The read is the endpoint's own query for that cube, one page as large as the broker answers.
    const read = calls.map((call) => new URL(call.path, "http://localhost")).find((url) => cubeOf(url.href) === "vh5003rr");
    expect(read?.searchParams.get("limit")).toBe("1000");
  });

  it("says when the charts cannot be read, and keeps the table", async () => {
    show({ cube: () => problem(503, "Upstream unavailable") });
    const overview = screen.getByRole("region", { name: sk.overview });
    expect(await within(overview).findByRole("alert")).toHaveTextContent(sk.chartsFailed);
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));
  });

  it("reads the figure from kpiValue in the table and the chart, and none from a value member (T-3966)", async () => {
    const user = userEvent.setup();
    const figure = new Intl.NumberFormat(sk.chart.locale, { maximumFractionDigits: 2 }).format(4061.2);
    show();
    await waitFor(() => expect(rows().some((row) => row.textContent?.includes(figure))).toBe(true));
    const overview = screen.getByRole("region", { name: sk.overview });
    await user.click(within(overview).getByRole("button", { name: datasetName("vh5003rr", "sk") }));
    const chart = await within(overview).findByRole("figure", { name: /Spotreba pitnej vody - spolu/ });
    expect(chart.textContent).toContain(figure);
  });

  it("draws no figure for a record that names its number value, the member T-3964 renamed", async () => {
    const renamed = () =>
      json(answer(CITY).map(({ kpiValue, ...rest }) => ({ ...rest, value: kpiValue })));
    show({ rows: renamed, cube: renamed });
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));
    const figure = new Intl.NumberFormat(sk.chart.locale, { maximumFractionDigits: 2 }).format(4061.2);
    expect(rows().some((row) => row.textContent?.includes(figure))).toBe(false);
  });

  it("says when a cube has no records yet", async () => {
    show({ cube: () => json([]) });
    const overview = screen.getByRole("region", { name: sk.overview });
    expect(await within(overview).findByText(sk.chartsEmpty)).toBeInTheDocument();
  });
});

describe("a filter is the endpoint's own query", () => {
  it("sends what the filter row says it sends, and never filters in the browser", async () => {
    const user = userEvent.setup();
    show();
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));

    await user.selectOptions(
      screen.getByRole("combobox", { name: `${sk.grid.filter}: ${sk.column.refPeriod}` }),
      "equals",
    );
    await user.type(screen.getByRole("textbox", { name: `${sk.grid.value}: ${sk.column.refPeriod}` }), "2023");

    await waitFor(() => {
      const last = new URL(calls[calls.length - 1].path, "http://localhost");
      expect(last.searchParams.get("q")).toBe('refPeriod=="2023"');
    });
    // The query the screen shows is the query it sent.
    expect(screen.getByText(`q=refPeriod=="2023"`)).toBeInTheDocument();
  });
});

describe("who may write the note (T-5137)", () => {
  const noteBoxes = () => screen.queryAllByRole("textbox", { name: `${sk.grid.edit} ${sk.column[NOTE]}` });

  it("shows a member of the region the notes read-only, and says who edits them", async () => {
    show({ rows: () => json(answer(REGION, SPACE_OF.bbsk, "bbsk.sk")) }, { space: SPACE_OF.bbsk, slug: REGION_SLUG, access: MEMBER });
    expect(await screen.findByText(sk.noteStewardsOnly.bbsk)).toBeInTheDocument();
    await waitFor(() => expect(rows()).toHaveLength(REGION.length));
    expect(noteBoxes()).toHaveLength(0);
    expect(screen.queryByRole("button", { name: sk.grid.review! })).toBeNull();
    expect(calls.filter((call) => call.method === "PATCH")).toEqual([]);
  });

  it("opens the note to a steward and says nothing about stewards", async () => {
    show({ rows: () => json(answer(REGION, SPACE_OF.bbsk, "bbsk.sk")) }, { space: SPACE_OF.bbsk, slug: REGION_SLUG });
    await waitFor(() => expect(noteBoxes()).toHaveLength(REGION.length));
    expect(screen.queryByText(sk.noteStewardsOnly.bbsk)).toBeNull();
  });

  it("keeps the note closed when a prohibition takes the write away from a steward", async () => {
    show({}, { access: { ...STEWARD, prohibitions: [{ resource: { type: "KeyPerformanceIndicator" }, actions: ["updateAttrs"], attributes: "*" }] } });
    expect(await screen.findByText(sk.noteStewardsOnly.banskabystrica)).toBeInTheDocument();
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));
    expect(noteBoxes()).toHaveLength(0);
  });

  it("says it in English too", async () => {
    show({}, { language: "en", access: MEMBER });
    expect(await screen.findByText(LOCALES.en.noteStewardsOnly.banskabystrica)).toBeInTheDocument();
  });
});

describe("writing the note", () => {
  /** Pastes a long note in one input: typed key by key, 501 characters re-render the grid 501 times
   * and passed vitest's 5 s on a busy CI runner (T-5342, run 39845897785). */
  async function typeNote(user: ReturnType<typeof userEvent.setup>, text: string, how: "type" | "paste" = "type") {
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));
    if (how === "paste") {
      await user.click(noteBox(0));
      await user.paste(text);
    } else {
      await user.type(noteBox(0), text);
    }
    await user.click(screen.getByRole("button", { name: sk.grid.review! }));
    const review = screen.getByRole("region", { name: sk.grid.review });
    // The review says in Slovak what each column holds, and "Uložiť" is only the button.
    expect(within(review).getAllByRole("columnheader").map((header) => header.textContent)).toEqual([
      "Záznam",
      "Atribút",
      "Pred zmenou",
      "Po zmene",
    ]);
    await user.click(within(review).getByRole("button", { name: sk.grid.apply! }));
  }

  it("sends one PATCH carrying the note and nothing else, with the person's own session", async () => {
    const user = userEvent.setup();
    show();
    await typeNote(user, "Overené.");

    const writes = calls.filter((call) => call.method === "PATCH");
    expect(writes).toHaveLength(1);
    expect(writes[0].path).toContain(`/api/endpoint/${CITY_SLUG}/ngsi-ld/v1/entities/`);
    expect(writes[0].path.endsWith("/attrs")).toBe(true);
    expect(writes[0].body).toEqual({ [NOTE]: { type: "Property", value: "Overené." } });
    // The person carries the write: the same-origin session, never a credential of the bundle.
    expect(writes[0].init?.credentials).toBe("same-origin");
    const headers = (writes[0].init?.headers ?? {}) as Record<string, string>;
    expect(Object.keys(headers).map((name) => name.toLowerCase())).not.toContain("authorization");
  });

  it("shows a 403 from the Policy in the gateway's own words, and keeps what was typed", async () => {
    const user = userEvent.setup();
    const refusal = "writing value needs the steward role";
    show({ write: () => problem(403, refusal) });
    await typeNote(user, "Overené.");

    expect((await screen.findAllByText(new RegExp(refusal)))[0]).toBeInTheDocument();
    // The value is still the person's: it stays pending rather than being thrown away.
    expect(noteBox(0)).toHaveValue("Overené.");
    expect(screen.getAllByRole("status").map((one) => one.textContent).join(" ")).toContain(
      sk.grid.pending!,
    );
  });

  it("does not retry a refused write without the person, and asks once (AP-40)", async () => {
    const user = userEvent.setup();
    show({ write: () => problem(401, "no session") });
    await typeNote(user, "Overené.");

    const writes = calls.filter((call) => call.method === "PATCH");
    expect(writes).toHaveLength(1);
    expect((writes[0].init?.headers ?? {}) as Record<string, string>).not.toHaveProperty("Authorization");
  });

  it("never sends a note the model would refuse", async () => {
    const user = userEvent.setup();
    show();
    await typeNote(user, "a".repeat(NOTE_MAX + 1), "paste");

    expect(calls.filter((call) => call.method === "PATCH")).toHaveLength(0);
    expect((await screen.findAllByText(new RegExp(noteWords(sk).tooLong(NOTE_MAX))))[0]).toBeInTheDocument();
  });
});

describe("what the screen says about itself", () => {
  it("is Slovak by default and English when the configuration asks", async () => {
    const { unmount } = show();
    expect(screen.getByRole("heading", { name: sk.title.banskabystrica, level: 1 })).toBeInTheDocument();
    unmount();

    show({}, { language: "en" });
    expect(
      screen.getByRole("heading", { name: LOCALES.en.title.banskabystrica, level: 1 }),
    ).toBeInTheDocument();
  });

  it("says so rather than guessing when it is configured for a space it does not know", () => {
    show({}, { space: "helsinki" });
    expect(screen.getByRole("alert")).toHaveTextContent(sk.unknownSpace);
    expect(calls).toHaveLength(0);
  });

  it("has no axe violation with the records on screen", async () => {
    const { container } = show();
    await waitFor(() => expect(rows()).toHaveLength(CITY.length));
    const results = await axe.run(container, {
      rules: { "color-contrast": { enabled: false }, region: { enabled: false } },
    });
    expect(results.violations.map((violation) => violation.id)).toEqual([]);
  });
});
