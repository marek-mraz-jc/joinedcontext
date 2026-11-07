/**
 * The catalog results (T-0579, UI-46, AG-58, AG-75): one compact row per item the assistant
 * found, four before "Show more", a "Use" that puts an endpoint into the conversation's data bar,
 * and every string drawn as text (AG-46).
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { expectDenied } from "./checks";
import { CatalogCards, catalogDatasetsOf, catalogItemsOf } from "../src/pages/apps/CatalogCards";
import { ago } from "../src/components/ago";
import type { CatalogDataset, CatalogItem } from "../src/pages/apps/CatalogCards";

const NOW = Date.parse("2026-09-13T08:00:12Z");

const ITEMS: CatalogItem[] = [
  {
    kind: "Endpoint",
    name: "helsinki-bikes",
    space: "helsinki",
    owner: "helsinki",
    title: "Helsinki city bikes <script>alert(1)</script>",
    endpointSlug: "bikesslug00000000000000000000000",
    matchReason: ["title", "description"],
    access: { verdict: "allowed", reason: "audience public" },
    freshness: { pipeline: "hsl-bikes", scrapedAt: "2026-09-13T08:00:00Z", received: 1200, errors: 0 },
  },
  {
    kind: "Endpoint",
    name: "helsinki-partners",
    space: "helsinki",
    owner: "helsinki",
    matchReason: ["title"],
    access: { verdict: "restricted", reason: "audience project-list does not name helsinki" },
    freshness: null,
  },
  {
    kind: "ContextSpace",
    name: "helsinki",
    space: "helsinki",
    owner: "helsinki",
    title: "Helsinki city context",
    matchReason: ["endpoint helsinki-bikes"],
    access: { verdict: "allowed", reason: "endpoint helsinki-bikes admits you" },
    freshness: null,
  },
];

function renderCards(
  items: CatalogItem[] = ITEMS,
  props: {
    onUseEndpoint?: (name: string) => void;
    usedEndpoints?: string[];
    words?: string;
    datasets?: CatalogDataset[];
  } = {},
) {
  const rootRoute = createRootRoute({
    component: () => <CatalogCards project="helsinki" items={items} now={NOW} {...props} />,
  });
  const router = createRouter({ routeTree: rootRoute });
  return render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
}

function endpointItem(n: number): CatalogItem {
  return {
    kind: "Endpoint",
    name: `helsinki-${String(n)}`,
    space: "helsinki",
    owner: "helsinki",
    title: `Helsinki ${String(n)}`,
    matchReason: ["title"],
    access: { verdict: "allowed", reason: "audience public" },
    freshness: null,
  };
}

describe("the catalog results", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("draws one compact row per item: kind, title, name, an access dot and freshness when known", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(3);

    const bikes = rows[0];
    expect(within(bikes).getByRole("img", { name: "Endpoint" })).toBeInTheDocument();
    expect(within(bikes).getByText("helsinki-bikes")).toBeInTheDocument();
    expect(within(bikes).getByRole("img", { name: `${en.agentRun.catalog.allowed} · audience public` })).toBeInTheDocument();
    const fed = within(bikes).getByText("fed 12 s ago");
    expect(fed.getAttribute("title")).toBe("Fed 12 s ago by hsl-bikes");

    const partners = rows[1];
    expect(
      within(partners).getByRole("img", {
        name: `${en.agentRun.catalog.restricted} · audience project-list does not name helsinki`,
      }),
    ).toBeInTheDocument();
    expect(within(partners).queryByText(/^fed /)).toBeNull();

    // The icon's accessible name is the translated kind, not the raw API value it used to
    // announce — this is the one meaningful icon in the Portal (T-1748).
    expect(within(rows[2]).getByRole("img", { name: "Context space" })).toBeInTheDocument();
  });

  it("keeps every row inside a narrow chat column: rows wrap and shrink, long text truncates with a title", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    for (const row of within(list).getAllByRole("listitem")) {
      expect(row).toHaveClass("min-w-0", "flex-wrap");
    }
    const bikes = within(list).getAllByRole("listitem")[0];
    const title = within(bikes).getByRole("link");
    expect(title).toHaveClass("min-w-0", "truncate");
    expect(title).toHaveAttribute("title", ITEMS[0].title);
    // The access reason rides on the dot's tooltip, never as a line that runs past the edge.
    expect(within(bikes).queryByText(/audience public/)).toBeNull();
  });

  it("links an open item's title to the explorer and a restricted one to its kind's page", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const rows = within(list).getAllByRole("listitem");

    const href = within(rows[0]).getByRole("link").getAttribute("href") ?? "";
    expect(href).toContain("/projects/helsinki/explore");
    expect(href).toContain("space=helsinki");
    expect(href).toContain("endpoint=helsinki-bikes");

    expect(within(rows[1]).getByRole("link", { name: "helsinki-partners" }).getAttribute("href")).toBe(
      "/projects/helsinki/endpoints",
    );

    const space = within(rows[2]).getByRole("link", { name: "Helsinki city context" }).getAttribute("href") ?? "";
    expect(space).toContain("space=helsinki");
    expect(space).not.toContain("endpoint=");
  });

  it("shows four rows, then the rest behind Show more", async () => {
    const user = userEvent.setup();
    renderCards([1, 2, 3, 4, 5, 6].map(endpointItem));
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    expect(within(list).getAllByRole("listitem")).toHaveLength(4);

    await user.click(screen.getByRole("button", { name: "Show 2 more" }));
    expect(within(list).getAllByRole("listitem")).toHaveLength(6);

    await user.click(screen.getByRole("button", { name: en.agentRun.catalog.showLess }));
    expect(within(list).getAllByRole("listitem")).toHaveLength(4);
  });

  it("offers Use on an open endpoint, spent once the conversation reads it; spaces and restricted rows have none", async () => {
    const user = userEvent.setup();
    const used: string[] = [];
    renderCards(ITEMS, {
      onUseEndpoint: (name) => {
        used.push(name);
      },
      usedEndpoints: ["helsinki-kpi"],
    });
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const rows = within(list).getAllByRole("listitem");

    await user.click(within(rows[0]).getByRole("button", { name: "Use helsinki-bikes in this conversation" }));
    expect(used).toEqual(["helsinki-bikes"]);
    expect(within(rows[1]).queryByRole("button")).toBeNull();
    expect(within(rows[2]).queryByRole("button")).toBeNull();
  });

  it("marks an endpoint already in the conversation as in use", async () => {
    renderCards(ITEMS, { onUseEndpoint: () => undefined, usedEndpoints: ["helsinki-bikes"] });
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const button = within(within(list).getAllByRole("listitem")[0]).getByRole("button", {
      name: "helsinki-bikes is in use",
    });
    // Refused and still reachable, so the reason can be read: a hard-disabled control leaves
    // the tab order and takes its explanation with it (T-1743, UI-44).
    expectDenied(button, "helsinki-bikes is in use");
    expect(button).toHaveTextContent(en.agentRun.catalog.inUse);
  });

  it("offers no Use without a conversation to add to", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    expect(within(list).queryAllByRole("button")).toHaveLength(0);
  });

  it("renders what the run stream carried as text, never as markup (AG-46)", async () => {
    renderCards();
    expect(await screen.findByText("Helsinki city bikes <script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });

  it("says when nothing matched, naming the words it searched for", async () => {
    renderCards([], { words: " electric scooters " });
    expect(
      await screen.findByText(
        "Nothing in this project, the endpoints shared with it or the open data catalogue matches “electric scooters”. Try a single word, such as a type or a place.",
      ),
    ).toBeInTheDocument();
  });

  /// T-6104: "air quality in Banská Bystrica" from Helsinki answered nine blank rows and then
  /// "Nothing in this project matches" while the open-data catalogue held public-air.
  it("draws open datasets in their own group when no project item matched", async () => {
    const datasets: CatalogDataset[] = [
      { title: "Kvalita ovzdušia", url: "https://opendata.example/dataset/public-air", publisher: "Banská Bystrica", formats: ["CSV"] },
    ];
    renderCards([], { words: "air quality banska bystrica", datasets });
    const group = await screen.findByRole("region", { name: en.agentRun.catalog.datasets });
    expect(within(group).getByRole("link", { name: /Kvalita ovzdušia/ })).toHaveAttribute(
      "href",
      "https://opendata.example/dataset/public-air",
    );
    expect(within(group).getByText("Banská Bystrica")).toBeInTheDocument();
    expect(screen.queryByText(/Nothing in this project/)).toBeNull();
  });

  it("names a row by its name when its title is blank, and an item shared from another project by its owner", async () => {
    renderCards([
      { ...endpointItem(1), title: "" },
      { ...endpointItem(2), name: "app-banskabystrica-ovzdusie", title: "  ", owner: "banskabystrica" },
    ]);
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    expect(within(list).getByRole("link", { name: "helsinki-1" })).toBeInTheDocument();
    // Another project's pages are not this one's: the row is text, with who shares it.
    expect(within(list).getByText("app-banskabystrica-ovzdusie")).toBeInTheDocument();
    expect(within(list).queryByRole("link", { name: "app-banskabystrica-ovzdusie" })).toBeNull();
    expect(within(list).getByText("shared by banskabystrica")).toBeInTheDocument();
  });

  it("takes only the datasets with a title and an address", () => {
    expect(catalogDatasetsOf({ datasets: [{ title: "a", url: "https://x/a" }, { title: "b" }, null] })).toHaveLength(1);
    expect(catalogDatasetsOf({ items: [] })).toEqual([]);
  });

  it("says when nothing matched a search whose words it does not know", async () => {
    renderCards([]);
    expect(await screen.findByText(en.agentRun.catalog.none)).toBeInTheDocument();
  });

  it("takes only the items the Portal wrote", () => {
    expect(catalogItemsOf({ items: ITEMS })).toHaveLength(3);
    expect(catalogItemsOf({ items: [{ kind: "Endpoint" }, "x", null] })).toHaveLength(0);
    expect(catalogItemsOf("items")).toHaveLength(0);
    expect(catalogItemsOf(null)).toHaveLength(0);
  });

  it("formats a relative time from an instant and leaves other text alone", () => {
    const t = (key: string, values?: Record<string, unknown>) => `${key}:${String(values?.n)}`;
    expect(ago("2026-09-13T08:00:00Z", NOW, t)).toBe("agentRun.catalog.agoSeconds:12");
    expect(ago("2026-09-13T07:55:00Z", NOW, t)).toBe("agentRun.catalog.agoMinutes:5");
    expect(ago("2026-09-13T05:00:00Z", NOW, t)).toBe("agentRun.catalog.agoHours:3");
    expect(ago("2026-09-11T08:00:00Z", NOW, t)).toBe("agentRun.catalog.agoDays:2");
    expect(ago("not a date", NOW, t)).toBe("not a date");
  });
});
