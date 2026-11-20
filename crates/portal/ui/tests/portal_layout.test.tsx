// The Layout tab of a portal (T-7434): pages built from blocks, each editor action on the model,
// the tab as an admin meets it (the built-in home, a home and a page built and published as one
// Change, a refusal named, the dropdowns holding only what the server lists for this portal).
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  addRow,
  addToRow,
  cleanPages,
  duplicateBlock,
  faultsOfPages,
  moveBlock,
  newBlock,
  removeBlock,
  setBlock,
  starterHome,
} from "../src/pages/portals/portalPages";
import type { PortalPage } from "../src/pages/portals/portalPages";
import { refusedFields, touchesLayout } from "../src/pages/portals/PortalLayout";
import { expectAxeClean, jsonResponse, problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const text = (sk: string) => ({ type: "text" as const, markdown: { sk } });
const page = (...rows: ReturnType<typeof text>[][]): PortalPage[] => [{ slug: "home", rows: rows.map((blocks) => ({ blocks })) }];

describe("the layout model", () => {
  it("adds a row, adds beside a block up to three, and never past the limits", () => {
    let pages = addRow(page(), 0, text("a"));
    pages = addToRow(pages, 0, 0, text("b"));
    pages = addToRow(pages, 0, 0, text("c"));
    expect(addToRow(pages, 0, 0, text("d"))).toBe(pages);
    expect(pages[0]?.rows?.[0]?.blocks.map((b) => (b.type === "text" ? b.markdown.sk : ""))).toEqual(["a", "b", "c"]);
    let full = page();
    for (let i = 0; i < 20; i += 1) full = addRow(full, 0, text(String(i)));
    expect(addRow(full, 0, text("x"))).toBe(full);
  });

  it("moves a block left, right, up into its own row and back, and a lone block moves its row", () => {
    const start = page([text("a"), text("b")], [text("c")]);
    const right = moveBlock(start, { page: 0, row: 0, block: 0 }, "right");
    expect(right[0]?.rows?.[0]?.blocks.map((b) => (b.type === "text" ? b.markdown.sk : ""))).toEqual(["b", "a"]);
    const out = moveBlock(start, { page: 0, row: 0, block: 1 }, "down");
    expect(out[0]?.rows?.map((r) => r.blocks.length)).toEqual([1, 1, 1]);
    const swapped = moveBlock(start, { page: 0, row: 1, block: 0 }, "up");
    expect(swapped[0]?.rows?.[0]?.blocks[0]).toEqual(text("c"));
    expect(moveBlock(start, { page: 0, row: 1, block: 0 }, "down")).toBe(start);
    expect(moveBlock(start, { page: 0, row: 0, block: 0 }, "left")).toBe(start);
  });

  it("duplicates beside a block, or below when the row is full, and removes one with its empty row", () => {
    const start = page([text("a")]);
    const two = duplicateBlock(start, { page: 0, row: 0, block: 0 });
    expect(two[0]?.rows?.[0]?.blocks).toHaveLength(2);
    expect(two[0]?.rows?.[0]?.blocks[1]).not.toBe(two[0]?.rows?.[0]?.blocks[0]);
    const full = page([text("a"), text("b"), text("c")]);
    expect(duplicateBlock(full, { page: 0, row: 0, block: 1 })[0]?.rows?.map((r) => r.blocks.length)).toEqual([3, 1]);
    expect(removeBlock(start, { page: 0, row: 0, block: 0 })[0]?.rows).toEqual([]);
    expect(setBlock(start, { page: 0, row: 0, block: 0 }, text("z"))[0]?.rows?.[0]?.blocks[0]).toEqual(text("z"));
  });

  it("starts from today's home and names each incomplete field by the server's path", () => {
    expect(starterHome().rows?.map((r) => r.blocks[0]?.type)).toEqual(["hero", "numbers", "dataset-gallery"]);
    expect(faultsOfPages([starterHome()])).toEqual({});
    const faults = faultsOfPages([
      { slug: "home", rows: [{ blocks: [newBlock("map", "sk"), newBlock("text", "sk"), newBlock("link-cards", "sk")] }] },
      { slug: "Doprava", rows: [] },
      { slug: "home" },
    ]);
    expect(Object.keys(faults).sort()).toEqual([
      "spec.pages[0].rows[0].blocks[0].dataset",
      "spec.pages[0].rows[0].blocks[1].markdown",
      "spec.pages[0].rows[0].blocks[2].cards[0].title",
      "spec.pages[0].rows[0].blocks[2].cards[0].url",
      "spec.pages[1].slug",
      "spec.pages[1].title",
      "spec.pages[2].slug",
    ]);
    const evil = faultsOfPages([{ slug: "home", rows: [{ blocks: [{ type: "link-cards", cards: [{ title: { sk: "x" }, url: "javascript:alert(1)" }] }] }] }]);
    expect(evil).toHaveProperty("spec.pages[0].rows[0].blocks[0].cards[0].url");
  });

  it("counts as layout history only a Change that wrote spec.pages", () => {
    expect(touchesLayout([{ path: "spec.pages[0].rows" }])).toBe(true);
    expect(touchesLayout([{ path: "spec.sources[0]" }, { path: "spec.hidden" }])).toBe(false);
    expect(touchesLayout(null)).toBe(false);
  });

  it("splits a refusal into the fields of spec.pages it names and the rest", () => {
    expect(
      refusedFields(["spec.pages[1].rows[0].blocks[2].dataset: unknown dataset", "spec.pages[0].rows[1].blocks[0].markdown.sk: Markdown of 1 to 8 KiB", "spec.title: required", "git forge down"]),
    ).toEqual({
      fields: { "spec.pages[1].rows[0].blocks[2].dataset": "Unknown dataset", "spec.pages[0].rows[1].blocks[0].markdown": "Markdown of 1 to 8 KiB" },
      rest: ["spec.title: required", "git forge down"],
    });
  });

  it("writes only filled members to the manifest", () => {
    const cleaned = cleanPages([
      { slug: "home", menu: true, title: { sk: "" }, rows: [{ blocks: [{ type: "hero", search: true, title: { sk: "Dáta", en: "" } }] }] },
    ]);
    expect(cleaned).toEqual([{ slug: "home", rows: [{ blocks: [{ type: "hero", title: { sk: "Dáta" } }] }] }]);
  });
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk" }] };
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const CHANGE = { metadata: { name: "chg-1", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };
const SHA = "a".repeat(64);
const FILES = {
  items: [
    { id: "f-1", name: "hero.png", contentType: "image/png", size: 2048, sha256: SHA, path: `/files/${SHA}/hero.png`, uploadedBy: "Ana", uploadedAt: "2026-11-19T19:00:00Z", usedBy: [] },
    { id: "f-2", name: "plan.pdf", contentType: "application/pdf", size: 4096, sha256: "b".repeat(64), path: `/files/${"b".repeat(64)}/plan.pdf`, uploadedBy: "Ana", uploadedAt: "2026-11-19T19:00:00Z", usedBy: [] },
  ],
  limits: { files: 2, bytes: 6144, maxFiles: 500, maxBytes: 200 * 1024 * 1024, maxFileBytes: 10 * 1024 * 1024 },
};
const DATASETS = {
  total: 2,
  items: [
    { name: "zastavky", title: "Zastávky MHD", source: "endpoints", hidden: false },
    { name: "skryty", title: "Skrytý", source: "endpoints", hidden: true },
  ],
};

type Answer = (path: string, request: Request) => Response | Promise<Response> | undefined;

async function openLayout(stored: Record<string, unknown>, answer: Answer = () => undefined) {
  const rendered = await renderRoute({
    path: "/portals/bbsk-mobility",
    permissions: ADMIN,
    answer: (path, request) => {
      if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
      if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "GET") {
        return jsonResponse({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Portal",
          metadata: { name: "bbsk-mobility", namespace: "org" },
          spec: { organizationRef: "bbsk", path: "mobility", title: { en: "Mobility" }, languages: ["en", "sk"], ...stored },
        });
      }
      if (path === "/api/v1/projects/org/portals/bbsk-mobility/files") return jsonResponse(FILES);
      if (path === "/api/v1/projects/org/portals/bbsk-mobility/datasets") return jsonResponse(DATASETS);
      if (path.startsWith("/api/v1/projects/org/changes/history")) return jsonResponse({ apiVersion: "v1", kind: "List", items: [] });
      return answer(path, request);
    },
  });
  fireEvent.click(await screen.findByRole("tab", { name: "Layout" }));
  return rendered;
}

function proposals(sent: { url: string; body: { spec: { pages?: PortalPage[] } } }[]): Answer {
  return (path, request) => {
    if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT") {
      return request.json().then((body) => {
        sent.push({ url: request.url, body });
        return jsonResponse(request.url.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
      });
    }
    return undefined;
  };
}

describe("the layout tab", () => {
  it("says the portal shows the built-in home and starts from it", async () => {
    const { container } = await openLayout({});
    expect(await screen.findByText("This portal shows the built-in home")).toBeInTheDocument();
    expect(screen.getByText(/The portal stays as it is until an approver accepts the Change/)).toBeInTheDocument();
    await expectAxeClean(container);
    fireEvent.click(screen.getByRole("button", { name: "Start from the built-in home" }));
    expect(await screen.findByRole("button", { name: "Home page", pressed: true })).toBeInTheDocument();
    for (const type of ["Hero", "Numbers", "Dataset gallery"]) {
      expect(screen.getByRole("heading", { name: type, level: 4 })).toBeInTheDocument();
    }
    expect(screen.getByText(/You have changes that are not published/)).toBeInTheDocument();
  });

  it("builds a home and one page, picks only listed datasets and files, and publishes one checked Change", async () => {
    const user = userEvent.setup();
    const sent: { url: string; body: { spec: { pages?: PortalPage[] } } }[] = [];
    const { container } = await openLayout({}, proposals(sent));
    await user.click(await screen.findByRole("button", { name: "Start from the built-in home" }));
    // The hero's image: raster files of this portal only.
    const image = screen.getByLabelText("Image") as HTMLSelectElement;
    expect(Array.from(image.options).map((o) => o.textContent)).toEqual(["No image", "hero.png"]);
    fireEvent.change(image, { target: { value: `/files/${SHA}/hero.png` } });
    // A map row: the dataset dropdown lists the portal's datasets, a hidden one not.
    fireEvent.change(screen.getByLabelText(/^Block type/), { target: { value: "map" } });
    await user.click(screen.getByRole("button", { name: "Add a row" }));
    const dataset = screen.getByLabelText(/^Dataset on the map/) as HTMLSelectElement;
    expect(Array.from(dataset.options).map((o) => o.value)).toEqual(["", "zastavky"]);
    fireEvent.change(dataset, { target: { value: "zastavky" } });
    // A further page with its own address and title.
    await user.click(screen.getByRole("button", { name: "Add a page" }));
    expect(screen.getByText(/the page answers at \/page\/doprava/)).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^Address/), "doprava");
    await user.type(screen.getByLabelText(/^Title and menu entry/), "Transport");
    await user.click(screen.getByRole("button", { name: "Create the page" }));
    expect(await screen.findByRole("button", { name: "Transport", pressed: true })).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^Text/), "## Buses and trams");
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[0]?.url).toContain("dryRun=All");
    const pages = sent[1]?.body.spec.pages ?? [];
    expect(pages.map((p) => p.slug)).toEqual(["home", "doprava"]);
    expect(pages[0]?.rows?.[0]?.blocks[0]).toEqual({ type: "hero", image: `/files/${SHA}/hero.png` });
    expect(pages[0]?.rows?.[3]?.blocks[0]).toEqual({ type: "map", dataset: "zastavky" });
    expect(pages[1]).toEqual({ slug: "doprava", title: { en: "Transport" }, rows: [{ blocks: [{ type: "text", markdown: { en: "## Buses and trams" } }] }] });
    expect(await screen.findByText(/chg-1/)).toBeInTheDocument();
    await expectAxeClean(container);
  });

  it("names incomplete fields before it sends anything", async () => {
    const user = userEvent.setup();
    const sent: { url: string; body: { spec: { pages?: PortalPage[] } } }[] = [];
    await openLayout({ pages: [{ slug: "home", rows: [{ blocks: [{ type: "text", markdown: { en: "x" } }] }] }] }, proposals(sent));
    fireEvent.change(await screen.findByLabelText(/^Block type/), { target: { value: "chart" } });
    await user.click(screen.getByRole("button", { name: "Add a row" }));
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    expect(await screen.findByText("Some blocks are not complete")).toBeInTheDocument();
    expect(screen.getByText("Choose a dataset of this portal.")).toBeInTheDocument();
    expect(sent).toEqual([]);
  });

  it("shows the server's refusal and a non-admin's 403 in words", async () => {
    const user = userEvent.setup();
    await openLayout({ pages: [{ slug: "home", rows: [{ blocks: [{ type: "text", markdown: { en: "x" } }] }] }] }, (path, request) =>
      path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT"
        ? problem(403, "You may not propose a change to Portal bbsk-mobility.")
        : undefined,
    );
    await user.type(await screen.findByLabelText(/^Text/), " more");
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    const alert = (await screen.findByText("The portal refused the layout")).closest("[role=alert]") as HTMLElement;
    expect(within(alert).getByText(/You may not propose a change to Portal bbsk-mobility/)).toBeInTheDocument();
  });

  it("adds the block type chosen in a row beside that row's blocks", async () => {
    const user = userEvent.setup();
    await openLayout({ pages: [{ slug: "home", rows: [{ blocks: [{ type: "text", markdown: { en: "x" } }] }] }] });
    await user.click(await screen.findByRole("button", { name: "Add a block to this row" }));
    await user.selectOptions(screen.getByLabelText(/^Block to add to this row/), "map");
    await user.click(screen.getByRole("button", { name: "Add to this row" }));
    expect(await screen.findByRole("heading", { name: "Map", level: 4 })).toBeInTheDocument();
    expect(screen.getByLabelText(/^Dataset on the map/)).toBeInTheDocument();
  });

  it("puts a refusal that names a field on that field, on its page", async () => {
    const user = userEvent.setup();
    const pages = [
      { slug: "home", rows: [{ blocks: [{ type: "text", markdown: { en: "x" } }] }] },
      { slug: "doprava", title: { en: "Transport" }, rows: [{ blocks: [{ type: "text", markdown: { en: "y" } }] }] },
    ];
    await openLayout({ pages }, (path, request) =>
      path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT"
        ? jsonResponse({ type: "about:blank", title: "Refused", status: 422, detail: "Refused", errors: ["spec.pages[1].rows[0].blocks[0].markdown.en: Markdown of 1 to 8 KiB"] }, 422)
        : undefined,
    );
    await user.type(await screen.findByLabelText(/^Text/), " more");
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    const alert = (await screen.findByText("The portal refused the layout")).closest("[role=alert]") as HTMLElement;
    expect(within(alert).getByText("Fix the marked field, then publish again.")).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "/page/doprava" })).toBeInTheDocument();
    expect(screen.getByLabelText(/^Text/)).toHaveAccessibleDescription(/Markdown of 1 to 8 KiB/);
  });

  it("moves, duplicates and removes a block from its actions menu and discards back to the published layout", async () => {
    const user = userEvent.setup();
    await openLayout({ pages: [{ slug: "home", rows: [{ blocks: [{ type: "text", markdown: { en: "first" } }] }, { blocks: [{ type: "numbers", items: ["datasets"] }] }] }] });
    const card = (await screen.findAllByRole("heading", { name: "Numbers", level: 4 }))[0];
    expect(card).toBeDefined();
    await user.click(screen.getByRole("button", { name: "More actions for Numbers" }));
    await user.click(await screen.findByRole("menuitem", { name: "Move up" }));
    const headings = screen.getAllByRole("heading", { level: 4 }).map((h) => h.textContent);
    expect(headings).toEqual(["Numbers", "Text"]);
    await user.click(screen.getByRole("button", { name: "More actions for Text" }));
    await user.click(await screen.findByRole("menuitem", { name: "Duplicate" }));
    expect(screen.getAllByRole("heading", { name: "Text", level: 4 })).toHaveLength(2);
    await user.click(screen.getAllByRole("button", { name: "More actions for Text" })[0] as HTMLElement);
    await user.click(await screen.findByRole("menuitem", { name: "Remove" }));
    expect(screen.getAllByRole("heading", { name: "Text", level: 4 })).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(screen.getAllByRole("heading", { level: 4 }).map((h) => h.textContent)).toEqual(["Text", "Numbers"]);
    expect(screen.getByText("No unpublished changes.")).toBeInTheDocument();
  });
});
