/** The bridge desk over what the app's endpoint answers (T-2784). */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider, projectRow, toRichRow } from "@joinedcontext/sdk";
import type { AccessDocument, EntitySource } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App, { loadBridges, MOST } from "./App";
import { BRIDGES } from "./fixtures/mosty";
import { LOCALES } from "./locales";

const s = LOCALES.sk;
const SLUG = "pk7zc3mwa5qtx2nrh6bdv4yje2";

/** What the gateway answers a signed-in road worker on this App's endpoint: reading only (README). */
const READS: AccessDocument = {
  permissions: [{ resource: { type: "Bridge" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }],
  prohibitions: [],
};

function show(
  answer: () => Response = () => new Response(JSON.stringify(BRIDGES), { status: 200, headers: { "content-type": "application/json" } }),
  withEndpoint = true,
  endpoints?: { name: string; slug: string; space: string; types: string[] }[],
) {
  vi.stubGlobal("fetch", vi.fn(async () => answer()));
  // The table reads through `fetch`, the entity panel through the client: the same bridges on both.
  const client = stubClient({ entities: BRIDGES.map((bridge) => projectRow(toRichRow(bridge, "sk"), "sk")), access: READS }, {
    slug: withEndpoint ? SLUG : "",
    orgDomain: "bbsk.sk",
    space: withEndpoint ? "bbsk-registre" : "elsewhere",
    transport: "origin",
    appName: "bbsk-mosty",
    language: "sk",
    user: { id: "u-1", name: "Cestmír Správca", roles: ["viewer"] },
    ...(endpoints ? { endpoints } : {}),
    portal: "https://portal.bbsk.sk/projects/bbsk",
  });
  return render(
    <JcProvider client={client}>
      <App year={2026} />
    </JcProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const table = () => screen.getByRole("table");
const rowNames = () => within(table()).getAllByRole("rowheader").map((cell) => cell.firstChild?.textContent);

describe("the bridge desk", () => {
  it("shows the region's figures and the bridges oldest first, a missing value said", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const tile = (label: string) => screen.getByText(label).closest("div")!;
    expect(within(tile(s.tiles.bridges)).getByText("12")).toBeInTheDocument();
    expect(within(tile(s.tiles.listed)).getByText("2")).toBeInTheDocument();
    expect(rowNames()[0]).toBe("Kamenný most v Štiavnici");
    const missing = within(table()).getByRole("row", { name: /Most bez údajov/ });
    expect(within(missing).getAllByText(s.noValue).length).toBeGreaterThanOrEqual(5);
  });

  it("marks the oldest tenth in words beside the age", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const row = within(table()).getByRole("row", { name: /Kamenný most/ });
    expect(within(row).getByText(new RegExp(s.oldest))).toBeInTheDocument();
    expect(within(row).getByText(s.years(176), { exact: false })).toBeInTheDocument();
  });

  it("narrows by district, road class, listed status and search", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.roadClass }), "firstClass");
    expect(rowNames()).toHaveLength(4);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.district }), "Zvolen");
    expect(rowNames()).toEqual(["Most cez Slatinu"]);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.district }), "");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.roadClass }), "");
    await userEvent.click(screen.getByRole("checkbox", { name: s.listedOnly }));
    expect(rowNames()).toEqual(["Kamenný most v Štiavnici", "Most cez Hron v Banskej Bystrici"]);
    await userEvent.click(screen.getByRole("checkbox", { name: s.listedOnly }));
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "ssc");
    expect(rowNames()).toEqual(["Most cez Hron v Banskej Bystrici"]);
  });

  it("sorts by length and says so", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: s.sortBy(s.column.length) }));
    expect(screen.getByRole("button", { name: s.sortBy(s.column.length) }).closest("th")).toHaveAttribute("aria-sort", "descending");
    expect(rowNames()[0]).toBe("Most v Žiari");
  });

  it("downloads the table as shown, the values in words and a formula-looking name made harmless", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    let blob: Blob | undefined;
    vi.spyOn(URL, "createObjectURL").mockImplementation((made) => {
      blob = made as Blob;
      return "blob:mosty";
    });
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.district }), "Poltár");
    await userEvent.click(screen.getByRole("button", { name: s.download }));
    expect(Array.from(new Uint8Array(await blob!.arrayBuffer()).slice(0, 3))).toEqual([0xef, 0xbb, 0xbf]);
    const lines = (await blob!.text()).trimEnd().split("\r\n");
    expect(lines[0]).toBe(s.csvHeader.join(","));
    expect(lines).toHaveLength(3);
    expect(lines.some((line) => line.startsWith("'=CMD(),x-2,účelová"))).toBe(true);
  });

  it("says why when the endpoint refuses, and when there is no endpoint", async () => {
    show(() => new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "sign in as road staff" }), { status: 403, headers: { "content-type": "application/problem+json" } }));
    expect(await screen.findByRole("alert")).toHaveTextContent(s.failed("sign in as road staff"));
    show(undefined, false);
    expect(screen.getAllByText(s.noEndpoint)).toHaveLength(1);
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("opens a bridge by its name in the SDK's panel: a signed-in reader without a write right gets the Portal link, never Edit", async () => {
    const { container } = show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const name = within(table()).getByRole("button", { name: "Kamenný most v Štiavnici" });
    await userEvent.click(name);
    const panel = await screen.findByRole("dialog", { name: "Kamenný most v Štiavnici" });
    expect(await within(panel).findByText("kameň")).toBeInTheDocument();
    expect(name.closest("tr")).toHaveAttribute("aria-selected", "true");
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    const link = within(panel).getByRole("link", { name: "Otvoriť v Portáli" });
    expect(link).toHaveAttribute("href", expect.stringContaining(`entityId=${encodeURIComponent(String(BRIDGES.find((one) => (one.name as { languageMap: { sk: string } }).languageMap.sk === "Kamenný most v Štiavnici")?.id))}`));
    // Following the link leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    fireEvent.click(link);
    expect(followed).toHaveBeenCalledTimes(1);
    expect((await axe.run(container, { rules: { region: { enabled: false } } })).violations.map((v) => v.id)).toEqual([]);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(name).toHaveFocus());
  });

  it("opens every bridge of the table, through the endpoint the Portal lists", async () => {
    show(undefined, false, [{ name: "registre", slug: SLUG, space: "bbsk-registre", types: [] }]);
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    for (const name of rowNames()) {
      await userEvent.click(within(table()).getByRole("button", { name: name! }));
      const panel = await screen.findByRole("dialog", { name: name! });
      await userEvent.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    }
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("sorts by each sortable column, both ways", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    for (const key of ["name", "age", "spans", "length"] as const) {
      const button = () => screen.getByRole("button", { name: s.sortBy(s.column[key]) });
      await userEvent.click(button());
      const first = button().closest("th")!.getAttribute("aria-sort");
      await userEvent.click(button());
      expect(button().closest("th")).toHaveAttribute("aria-sort", first === "ascending" ? "descending" : "ascending");
    }
    // By name ascending first: the missing value last, whichever the direction.
    await userEvent.click(screen.getByRole("button", { name: s.sortBy(s.column.name) }));
    expect(rowNames()[0]).toBe("=CMD()");
  });

  it("reads at most five thousand bridges, page by page, and says when there are more", async () => {
    const asked: number[] = [];
    const page = (count: number) => Array.from({ length: count }, (_, at) => toRichRow({ ...BRIDGES[0], id: `${BRIDGES[0].id}-${at}` }));
    const full = { query: async (_q: unknown, at: { offset: number; limit: number }) => (asked.push(at.offset), { rows: page(at.limit) }) } as unknown as EntitySource;
    const loaded = await loadBridges(full, "sk", 2026);
    expect(asked).toHaveLength(MOST / 200);
    expect(loaded).toMatchObject({ truncated: true });
    expect(loaded.bridges).toHaveLength(MOST);
    const short = { query: async () => ({ rows: page(3) }) } as unknown as EntitySource;
    expect(await loadBridges(short, "sk", 2026)).toMatchObject({ truncated: false, bridges: { length: 3 } });
  });

  it("says a failure that is no endpoint answer in its own words", async () => {
    show(() => {
      throw new TypeError("offline");
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(s.failed("offline"));
  });

  it("has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
