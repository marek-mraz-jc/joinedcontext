// Layout tab, remaining branches: page form and removal, adding pages, every block form, the row menu and the preview of featured datasets.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { PortalPage } from "../src/pages/portals/portalPages";
import { jsonResponse, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk" }] };
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const CHANGE = { metadata: { name: "chg-9", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };
const SHA = "a".repeat(64);
const HERO_PATH = `/files/${SHA}/hero.png`;
const FILES = {
  items: [{ id: "f-1", name: "hero.png", contentType: "image/png", size: 2048, sha256: SHA, path: HERO_PATH, uploadedBy: "Ana", uploadedAt: "2026-11-19T19:00:00Z", usedBy: [] }],
  limits: { files: 1, bytes: 2048, maxFiles: 500, maxBytes: 200 * 1024 * 1024, maxFileBytes: 10 * 1024 * 1024 },
};
const DATASETS = {
  total: 2,
  items: [
    { name: "zastavky", title: "Zastavky MHD", source: "endpoints", hidden: false },
    { name: "kvalita", title: "Kvalita ovzdusia", source: "endpoints", hidden: false },
  ],
};

type Sent = { url: string; body: { spec: { pages?: PortalPage[] } } };

async function openLayout(pages: PortalPage[], sent: Sent[] = []) {
  await renderRoute({
    path: "/portals/bbsk-mobility",
    permissions: ADMIN,
    answer: (path, request) => {
      if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
      if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "GET") {
        return jsonResponse({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Portal",
          metadata: { name: "bbsk-mobility", namespace: "org" },
          spec: { organizationRef: "bbsk", path: "mobility", title: { en: "Mobility" }, languages: ["en", "sk"], pages },
        });
      }
      if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT") {
        return request.json().then((body: Sent["body"]) => {
          sent.push({ url: request.url, body });
          return jsonResponse(request.url.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
        });
      }
      if (path === "/api/v1/projects/org/portals/bbsk-mobility/files") return jsonResponse(FILES);
      if (path === "/api/v1/projects/org/portals/bbsk-mobility/datasets") return jsonResponse(DATASETS);
      if (path.startsWith("/api/v1/projects/org/changes/history")) return jsonResponse({ apiVersion: "v1", kind: "List", items: [] });
      return undefined;
    },
  });
  fireEvent.click(await screen.findByRole("tab", { name: "Layout" }));
}

const HOME_AND_PAGE: PortalPage[] = [
  { slug: "home", rows: [{ blocks: [{ type: "text", markdown: { en: "Welcome" } }] }] },
  { slug: "doprava", title: { en: "Doprava" }, rows: [{ blocks: [{ type: "text", markdown: { en: "Buses" } }] }] },
];

describe("the layout tab, pages", () => {
  it("renames a page, hides it from the menu, publishes, and removes it", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openLayout(HOME_AND_PAGE, sent);
    await user.click(await screen.findByRole("button", { name: "Doprava" }));
    const title = screen.getByLabelText(/^Title and menu entry/);
    await user.clear(title);
    await user.type(title, "Autobusy");
    await user.click(screen.getByRole("checkbox", { name: /^Show in the header menu/ }));
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.pages?.[1]).toMatchObject({ slug: "doprava", title: { en: "Autobusy" }, menu: false });
    expect(await screen.findByText(/chg-9/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove the page" }));
    expect(screen.queryByRole("button", { name: "Autobusy" })).toBeNull();
    expect(screen.getByRole("button", { name: "Home page" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(/You have changes that are not published/)).toBeInTheDocument();
  });

  it("offers to add a home page when only other pages exist, and refuses bad addresses", async () => {
    const user = userEvent.setup();
    await openLayout([{ slug: "doprava", title: { en: "Doprava" }, rows: [] }]);
    await user.click(await screen.findByRole("button", { name: "Add a home page" }));
    expect(screen.queryByRole("button", { name: "Add a home page" })).toBeNull();
    expect(screen.getByRole("button", { name: "Home page" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Add a page" }));
    const slug = screen.getByLabelText(/^Address/);
    await user.type(slug, "home");
    expect(screen.getByText("Lower-case letters, digits and dashes, not home.")).toBeInTheDocument();
    fireEvent.submit(slug.closest("form") as HTMLFormElement);
    expect(screen.queryByRole("button", { name: "Rozpocet" })).toBeNull();
    await user.clear(slug);
    await user.type(slug, "doprava");
    expect(screen.getByText("Another page of this portal has this address.")).toBeInTheDocument();
    await user.clear(slug);
    await user.type(slug, "rozpocet");
    // No title yet: submitting the form creates nothing.
    fireEvent.submit(slug.closest("form") as HTMLFormElement);
    expect(screen.getByLabelText(/^Address/)).toHaveValue("rozpocet");
    const newTitle = screen.getAllByLabelText(/^Title and menu entry/)[0] as HTMLElement;
    await user.type(newTitle, "Rozpocet");
    await user.click(screen.getByRole("button", { name: "Create the page" }));
    expect(await screen.findByRole("button", { name: "Rozpocet" })).toBeInTheDocument();
  });
});

describe("the layout tab, block forms", () => {
  it("edits a hero: heading, introduction, image and the search box", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openLayout([{ slug: "home", rows: [{ blocks: [{ type: "hero", title: { en: "Welcome" } }] }] }], sent);
    await user.click(await screen.findByRole("button", { name: "Edit Hero" }));
    const heading = screen.getByLabelText(/^Heading/);
    await user.clear(heading);
    await user.type(heading, "Hello");
    await user.type(screen.getByLabelText(/^Introduction/), "Data of the city");
    await user.selectOptions(screen.getByLabelText(/^Image/), "hero.png");
    await user.click(screen.getByRole("checkbox", { name: /^Show the dataset search box/ }));
    expect(screen.getByText("Image: hero.png")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.pages?.[0]?.rows?.[0]?.blocks[0]).toEqual({ type: "hero", title: { en: "Hello" }, intro: { en: "Data of the city" }, image: HERO_PATH, search: false });
  });

  it("edits a dataset gallery: keyword, theme, format, count and table layout", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openLayout([{ slug: "home", rows: [{ blocks: [{ type: "dataset-gallery", title: { en: "Newest" } }] }] }], sent);
    await user.click(await screen.findByRole("button", { name: "Edit Dataset gallery" }));
    await user.type(screen.getByLabelText(/^Keyword/), "doprava");
    await user.selectOptions(screen.getByLabelText(/^Theme/), "Transport");
    await user.selectOptions(screen.getByLabelText(/^Format/), "CSV");
    const limit = screen.getByLabelText(/^How many/);
    fireEvent.change(limit, { target: { value: "9" } });
    await user.click(screen.getByRole("radio", { name: "Table" }));
    expect(screen.getByText("The newest 9 matching datasets")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.pages?.[0]?.rows?.[0]?.blocks[0]).toMatchObject({ type: "dataset-gallery", tag: "doprava", format: "CSV", limit: 9, layout: "table" });
    // Cards is the default and is not written.
    await user.click(screen.getByRole("radio", { name: "Cards" }));
    await user.selectOptions(screen.getByLabelText(/^Theme/), "Any");
    await user.clear(screen.getByLabelText(/^Keyword/));
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    await waitFor(() => expect(sent.length).toBe(4));
    const block = sent[3]?.body.spec.pages?.[0]?.rows?.[0]?.blocks[0] as Record<string, unknown>;
    expect(block.layout).toBeUndefined();
    expect(block.theme).toBeUndefined();
    expect(block.tag).toBeUndefined();
  });

  it("picks featured datasets once each, shows them in the preview and takes one off", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openLayout([{ slug: "home", rows: [{ blocks: [{ type: "featured-datasets", title: { en: "Chosen" }, datasets: ["zastavky"] }] }] }], sent);
    await user.click(await screen.findByRole("button", { name: "Edit Featured datasets" }));
    expect(screen.getAllByText("Zastavky MHD").length).toBeGreaterThan(1);
    const pick = screen.getByLabelText(/^Add a dataset/);
    await user.selectOptions(pick, "Kvalita ovzdusia");
    await user.selectOptions(pick, "Kvalita ovzdusia");
    expect(screen.getAllByText("Kvalita ovzdusia").length).toBeGreaterThan(1);
    await user.selectOptions(pick, "Zastavky MHD");
    await user.click(screen.getByRole("button", { name: "Remove Zastavky MHD" }));
    expect(screen.queryByRole("button", { name: "Remove Zastavky MHD" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Publish layout" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.pages?.[0]?.rows?.[0]?.blocks[0]).toMatchObject({ type: "featured-datasets", datasets: ["kvalita"] });
  });

  it("types the heading of a block that has one", async () => {
    const user = userEvent.setup();
    await openLayout([{ slug: "home", rows: [{ blocks: [{ type: "map", dataset: "zastavky" }] }] }]);
    await user.click(await screen.findByRole("button", { name: "Edit Map" }));
    await user.type(screen.getByLabelText(/^Heading/), "Stops");
    expect(screen.getByLabelText(/^Heading/)).toHaveValue("Stops");
    expect(screen.getByText("Map of Zastavky MHD")).toBeInTheDocument();
  });
});

describe("the layout tab, the row menu", () => {
  it("moves a block right, left and out to its own row from its actions menu", async () => {
    const user = userEvent.setup();
    await openLayout([
      {
        slug: "home",
        rows: [{ blocks: [{ type: "text", markdown: { en: "first" } }, { type: "numbers", items: ["datasets"] }] }, { blocks: [{ type: "map", dataset: "zastavky" }] }],
      },
    ]);
    await screen.findAllByRole("heading", { name: "Text", level: 4 });
    const order = () => screen.getAllByRole("heading", { level: 4 }).map((h) => h.textContent);
    expect(order()).toEqual(["Text", "Numbers", "Map"]);
    await user.click(screen.getByRole("button", { name: "More actions for Text" }));
    await user.click(await screen.findByRole("menuitem", { name: "Move right" }));
    expect(order()).toEqual(["Numbers", "Text", "Map"]);
    await user.click(screen.getByRole("button", { name: "More actions for Text" }));
    await user.click(await screen.findByRole("menuitem", { name: "Move left" }));
    expect(order()).toEqual(["Text", "Numbers", "Map"]);
    await user.click(screen.getByRole("button", { name: "More actions for Numbers" }));
    await user.click(await screen.findByRole("menuitem", { name: "Move down" }));
    expect(screen.getAllByRole("region", { name: /^Row \d$/ }).length).toBe(3);
  });

  it("lists no picked datasets for a featured block that has none yet", async () => {
    await openLayout([{ slug: "home", rows: [{ blocks: [{ type: "featured-datasets", datasets: [] }] }] }]);
    // An incomplete block opens with its form.
    expect(await screen.findByLabelText(/^Add a dataset/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Remove / })).toBeNull();
  });
});
