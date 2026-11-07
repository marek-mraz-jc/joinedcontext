/** The school desk over what the app's endpoint answers (T-2782). */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider, projectRow, toRichRow } from "@joinedcontext/sdk";
import type { AccessDocument, EntitySource } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App, { loadSchools, MOST } from "./App";
import { SCHOOLS } from "./fixtures/skoly";
import { LOCALES } from "./locales";

const s = LOCALES.sk;
const SLUG = "hm6ypd3qzv2xcnr7wtb4kaf5je";

/** What the gateway answers a signed-in member of staff on this App's endpoint: reading only (README). */
const READS: AccessDocument = {
  permissions: [{ resource: { type: "Organization" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }],
  prohibitions: [],
};

function show(
  answer: () => Response = () => new Response(JSON.stringify(SCHOOLS), { status: 200, headers: { "content-type": "application/json" } }),
  withEndpoint = true,
  endpoints?: { name: string; slug: string; space: string; types: string[] }[],
) {
  vi.stubGlobal("fetch", vi.fn(async () => answer()));
  // The table reads through `fetch`, the entity panel through the client: the same schools on both.
  const client = stubClient({ entities: SCHOOLS.map((one) => projectRow(toRichRow(one, "sk"), "sk")), access: READS }, {
    slug: withEndpoint ? SLUG : "",
    orgDomain: "banskabystrica.sk",
    space: withEndpoint ? "banskabystrica-verejne" : "elsewhere",
    transport: "origin",
    appName: "banskabystrica-skoly",
    language: "sk",
    user: { id: "u-1", name: "Referentka", roles: ["viewer"] },
    ...(endpoints ? { endpoints } : {}),
    portal: "https://portal.banskabystrica.sk/projects/banskabystrica",
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const table = () => screen.getByRole("table");
const rowNames = () => within(table()).getAllByRole("rowheader").map((cell) => cell.firstChild?.textContent);

describe("the school desk", () => {
  it("shows the city's figures and every school, a ratio missing where a count is", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const tile = (label: string) => screen.getByText(label).closest("div")!;
    expect(within(tile(s.tiles.schools)).getByText("12")).toBeInTheDocument();
    expect(within(tile(s.tiles.incomplete)).getByText("1")).toBeInTheDocument();
    const missing = within(table()).getByRole("row", { name: /Stredná odborná škola/ });
    expect(within(missing).getAllByText(s.noValue).length).toBeGreaterThanOrEqual(4);
  });

  it("marks the city's highest tenth of pupils per teacher in words, not only colour", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const flagged = within(table()).getAllByText(new RegExp(s.highest)).map((mark) => mark.closest("tr")?.querySelector("th")?.firstChild?.textContent);
    expect(flagged).toContain("Základná škola, Tatranská 10");
    expect(screen.getByText(s.tenthNote)).toBeInTheDocument();
  });

  it("sorts by a column, says so, and keeps a missing value last", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: s.sortBy(s.column.pupils) }));
    expect(screen.getByRole("button", { name: s.sortBy(s.column.pupils) }).closest("th")).toHaveAttribute("aria-sort", "descending");
    expect(rowNames()[0]).toBe("Gymnázium J. G. Tajovského, Tajovského 25");
    expect(rowNames().at(-1)).toBe("Stredná odborná škola, Tajovského 30");
  });

  it("narrows by search and to complete figures", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "tajovskeho");
    expect(rowNames()).toEqual(["Gymnázium J. G. Tajovského, Tajovského 25", "Stredná odborná škola, Tajovského 30"]);
    await userEvent.click(screen.getByRole("checkbox", { name: s.completeOnly }));
    expect(rowNames()).toEqual(["Gymnázium J. G. Tajovského, Tajovského 25"]);
  });

  it("downloads the table as shown, a formula-looking name made harmless", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    let blob: Blob | undefined;
    vi.spyOn(URL, "createObjectURL").mockImplementation((made) => {
      blob = made as Blob;
      return "blob:skoly";
    });
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "namestie");
    await userEvent.click(screen.getByRole("button", { name: s.download }));
    const text = await blob!.text();
    // The byte-order mark is what makes a spreadsheet read the file as UTF-8; text() decodes it away.
    expect(Array.from(new Uint8Array(await blob!.arrayBuffer()).slice(0, 3))).toEqual([0xef, 0xbb, 0xbf]);
    expect(text.split("\r\n")[0]).toBe(s.csvHeader.join(","));
    expect(text).toContain(`"'=HYPERLINK(`);
    expect(text.trimEnd().split("\r\n")).toHaveLength(2);
  });

  it("says why when the endpoint refuses, and when there is no endpoint", async () => {
    show(() => new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "sign in as city staff" }), { status: 403, headers: { "content-type": "application/problem+json" } }));
    expect(await screen.findByRole("alert")).toHaveTextContent(s.failed("sign in as city staff"));
    show(undefined, false);
    expect(screen.getAllByText(s.noEndpoint)).toHaveLength(1);
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("opens a school by its name in the SDK's panel: a signed-in reader without a write right gets the Portal link, never Edit", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const name = within(table()).getByRole("button", { name: "Základná škola, Tatranská 10" });
    fireEvent.click(name);
    const panel = await screen.findByRole("dialog", { name: "Základná škola, Tatranská 10" });
    expect(name.closest("tr")).toHaveAttribute("aria-selected", "true");
    const link = await within(panel).findByRole("link", { name: "Otvoriť v Portáli" });
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    fireEvent.click(link);
    expect(followed).toHaveBeenCalledTimes(1);
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens every school of the table, through the endpoint the Portal lists", async () => {
    show(undefined, false, [{ name: "verejne", slug: SLUG, space: "banskabystrica-verejne", types: [] }]);
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    for (const button of within(table()).getAllByRole("button").filter((one) => one.classList.contains("row-open"))) {
      fireEvent.click(button);
      const panel = await screen.findByRole("dialog");
      fireEvent.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    }
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("sorts by each sortable column, both ways", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    for (const key of ["name", "pupils", "teachers", "pupilsPerTeacher", "budgetPerPupil"] as const) {
      const button = () => screen.getByRole("button", { name: s.sortBy(s.column[key]) });
      fireEvent.click(button());
      const first = button().closest("th")!.getAttribute("aria-sort");
      fireEvent.click(button());
      expect(button().closest("th")).toHaveAttribute("aria-sort", first === "ascending" ? "descending" : "ascending");
    }
  });

  it("reads at most a thousand schools, page by page, and says when there are more", async () => {
    const asked: number[] = [];
    const page = (count: number) => Array.from({ length: count }, (_, at) => toRichRow({ ...SCHOOLS[0], id: `${String(SCHOOLS[0].id)}-${at}` }));
    const full = { query: async (_q: unknown, at: { offset: number; limit: number }) => (asked.push(at.offset), { rows: page(at.limit) }) } as unknown as EntitySource;
    const loaded = await loadSchools(full, "sk");
    expect(asked).toHaveLength(MOST / 200);
    expect(loaded).toMatchObject({ truncated: true });
    expect(s.truncated(MOST)).toContain(String(MOST));
    expect(LOCALES.en.truncated(MOST)).toContain(String(MOST));
  });

  it("says a failure that is no endpoint answer in its own words", async () => {
    show(() => {
      throw "offline";
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
  });

  it("says why nothing is marked with fewer than ten schools, and that no school matches a search", async () => {
    const three = SCHOOLS.slice(0, 3);
    show(() => new Response(JSON.stringify(three), { status: 200, headers: { "content-type": "application/json" } }));
    expect(await screen.findByText(s.tenthUnavailable)).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "nikde taká" } });
    expect(screen.getByText(s.empty)).toBeInTheDocument();
  });

  it("names a school the map publishes without a name or an address in words", async () => {
    const bare = { id: "urn:ngsi-ld:Organization:banskabystrica.sk:banskabystrica-verejne:bare", type: "Organization" };
    show(() => new Response(JSON.stringify([bare]), { status: 200, headers: { "content-type": "application/json" } }));
    const row = (await screen.findByRole("button", { name: s.noValue })).closest("tr") as HTMLElement;
    expect(row.querySelector(".address")).toBeNull();
    fireEvent.click(within(row).getByRole("button", { name: s.noValue }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("drops an answer that arrives after the desk is gone", async () => {
    let reply: () => void = () => undefined;
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>((_, reject) => (reply = () => reject(new TypeError("late"))))));
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "banskabystrica.sk", space: "banskabystrica-verejne", transport: "origin", appName: "banskabystrica-skoly" });
    const view = render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    view.unmount();
    const errors = vi.spyOn(console, "error");
    reply();
    await new Promise((settle) => setTimeout(settle, 0));
    expect(errors).not.toHaveBeenCalled();
  });

  it("has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
