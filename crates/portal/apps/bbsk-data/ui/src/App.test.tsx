/** The region's registers as grids over what the public endpoint answers (T-2784). */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider, projectRow, toRichRow } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { ALL, answer, keyOf } from "./fixtures/registre";
import { DATASETS, exportUrl } from "./datasets";
import type { Dataset } from "./datasets";
import { LOCALES } from "./locales";

const s = LOCALES.sk;
const SLUG = "tw3jx6pcam2qzk7nre5bdv4yfh";
let asked: URL[];

function show(withEndpoint = true, endpoints?: { name: string; slug: string; space: string; types: string[] }[]) {
  asked = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const url = new URL(path, "http://portal.test");
      asked.push(url);
      const rows = answer(keyOf(url));
      return new Response(JSON.stringify(rows), {
        status: 200,
        headers: { "content-type": "application/json", "NGSILD-Results-Count": String(rows.length) },
      });
    }),
  );
  // The grid reads through `fetch`, the entity panel through the client: the same rows on both.
  const client = stubClient({ entities: ALL.map((entity) => projectRow(toRichRow(entity, "sk"), "sk")) }, {
    slug: withEndpoint ? SLUG : "",
    orgDomain: "bbsk.sk",
    space: withEndpoint ? "bbsk-registre" : "elsewhere",
    transport: "origin",
    appName: "bbsk-data",
    language: "sk",
    ...(endpoints ? { endpoints } : {}),
    portal: "https://portal.bbsk.sk/projects/bbsk",
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("the region's registers", () => {
  it("opens on the hospitals, read from the endpoint by type, with the region's column labels", async () => {
    show();
    expect(screen.getByRole("tab", { name: s.dataset.hospitals })).toHaveAttribute("aria-selected", "true");
    const panel = screen.getByRole("tabpanel", { name: s.dataset.hospitals });
    await waitFor(() => expect(within(panel).getByText("Nemocnica Zvolen")).toBeInTheDocument());
    expect(within(panel).getByRole("columnheader", { name: new RegExp(s.column.hospitalKind) })).toBeInTheDocument();
    expect(asked.some((url) => url.pathname.includes(`/api/endpoint/${SLUG}/`) && url.searchParams.get("type") === "PointOfInterest" && url.searchParams.get("q")?.includes('category=="hospital"'))).toBe(true);
  });

  it("moves between registers with the arrow keys and shows each one's grid and downloads", async () => {
    show();
    screen.getByRole("tab", { name: s.dataset.hospitals }).focus();
    await userEvent.keyboard("{ArrowRight}");
    const social = screen.getByRole("tab", { name: s.dataset.social });
    expect(social).toHaveFocus();
    expect(social).toHaveAttribute("aria-selected", "true");
    const panel = screen.getByRole("tabpanel", { name: s.dataset.social });
    await waitFor(() => expect(within(panel).getByText("Domov sociálnych služieb Tisovec")).toBeInTheDocument());
    expect(within(panel).getByRole("link", { name: `${s.csv} ${s.dataset.social}` })).toHaveAttribute(
      "href",
      `/api/endpoint/${SLUG}/file.csv?type=PointOfInterest&q=category%3D%3D%22socialService%22&humanHeaders=true`,
    );
    await userEvent.keyboard("{End}");
    expect(screen.getByRole("tab", { name: s.dataset.areas })).toHaveFocus();
    await userEvent.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: s.dataset.hospitals })).toHaveFocus();
  });

  it("says it has nothing to read when the app has no endpoint of the register space", () => {
    show(false);
    expect(screen.getByText(s.noEndpoint)).toBeInTheDocument();
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("names its source and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(screen.getByText("Nemocnica Zvolen")).toBeInTheDocument());
    expect(screen.getByText(s.source)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });

  it("moves back with the left arrow, to the first with Home, and ignores other keys", async () => {
    show();
    screen.getByRole("tab", { name: s.dataset.hospitals }).focus();
    await userEvent.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: s.dataset.areas })).toHaveFocus();
    await userEvent.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: s.dataset.hospitals })).toHaveFocus();
    await userEvent.keyboard("a");
    expect(screen.getByRole("tab", { name: s.dataset.hospitals })).toHaveAttribute("aria-selected", "true");
  });

  it("opens a row by its name in the SDK's panel, links it to the Portal and offers no edit", async () => {
    show();
    const panel = screen.getByRole("tabpanel", { name: s.dataset.hospitals });
    fireEvent.click(await within(panel).findByRole("button", { name: `${s.grid.openRow}: Nemocnica Zvolen` }));
    const dialog = await screen.findByRole("dialog", { name: "Nemocnica Zvolen" });
    expect(await within(dialog).findByText(/Kuzmányho nábrežie 28/)).toBeInTheDocument();
    const link = within(dialog).getByRole("link", { name: "Otvoriť v Portáli" });
    expect(link).toHaveAttribute("href", expect.stringContaining(`entityId=${encodeURIComponent(ALL[1].id)}`));
    // Following the link leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    fireEvent.click(link);
    expect(followed).toHaveBeenCalledTimes(1);
    expect(within(dialog).queryByRole("button", { name: /upraviť|edit/i })).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Zavrieť" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("reads the register space through the endpoint the Portal names for it and opens a row from it", async () => {
    show(false, [{ name: "registre", slug: SLUG, space: "bbsk-registre", types: [] }]);
    const panel = screen.getByRole("tabpanel", { name: s.dataset.hospitals });
    fireEvent.click(await within(panel).findByRole("button", { name: `${s.grid.openRow}: Nemocnica Zvolen` }));
    const dialog = await screen.findByRole("dialog", { name: "Nemocnica Zvolen" });
    expect(await within(dialog).findByText(/Kuzmányho nábrežie 28/)).toBeInTheDocument();
    expect(asked.every((url) => url.pathname.startsWith(`/api/endpoint/${SLUG}/`))).toBe(true);
  });

  // The App's own controls on every register: its tab, its download, and a row it opens in the
  // shell's panel. The grid's own controls (sort, filter, details) are the SDK suite's (T-3373).
  it.each(DATASETS)("%s: the tab, the register's downloads and a row opened by its name", async (dataset: Dataset) => {
    show();
    fireEvent.click(screen.getByRole("tab", { name: s.dataset[dataset] }));
    const panel = screen.getByRole("tabpanel", { name: s.dataset[dataset] });
    expect(panel.querySelectorAll(".downloads a"), "one download: the gateway serves no GeoJSON file (T-4597, T-4603)").toHaveLength(1);
    for (const format of [s.csv]) {
      const link = within(panel).getByRole("link", { name: `${format} ${s.dataset[dataset]}` });
      expect(link).toHaveAttribute("download");
      const kept = vi.fn((event: Event) => event.preventDefault());
      link.addEventListener("click", kept);
      fireEvent.click(link);
      expect(kept).toHaveBeenCalledTimes(1);
    }
    for (const row of answer(keyOf(new URL(exportUrl(SLUG, dataset), "http://portal.test"))) as { id: string; name?: { languageMap: { sk: string } } }[]) {
      // A row without a name opens by its identifier's last part, and the panel names it the same (T-5619).
      const name = row.name?.languageMap.sk ?? row.id.slice(row.id.lastIndexOf(":") + 1);
      fireEvent.click(await within(panel).findByRole("button", { name: `${s.grid.openRow}: ${name}` }));
      const dialog = await screen.findByRole("dialog", { name });
      fireEvent.click(within(dialog).getByRole("button", { name: "Zavrieť" }));
    }
  });
});

