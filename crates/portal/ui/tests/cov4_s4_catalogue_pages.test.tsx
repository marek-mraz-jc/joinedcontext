// T-7706: the catalogue list and a dataset page: a failed read with Retry, paging, a cleared search, a ticked filter nobody counts, the publish button, the resources table search, a dataset with nothing to copy, and a refused clipboard.
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { App } from "../src/App";

const API = "joinedcontext.com/v1alpha1";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const ENDPOINT = `https://api.dev.joinedcontext.com/api/endpoint/${SLUG}/`;
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-viewer"] };
const facet = (value: string, label: string, count: number) => ({ value, label, count });

const dataset = (name: string, title: string) => ({
  name,
  title,
  publisher: { name: "helsinki", title: "Helsinki" },
  formats: ["CSV"],
  themes: ["TRAN"],
});
const PAGE = {
  total: 45,
  page: 2,
  pageSize: 20,
  datasets: [dataset("bikes", "City bikes"), dataset("trams", "Trams")],
  facets: { publisher: [facet("helsinki", "Helsinki", 2)], theme: [facet("TRAN", "Transport", 2)], format: [], licence: [], spatial: [], year: [] },
  unavailable: [],
};
const resource = (n: number) => ({ name: `Resource ${n}`, format: n % 2 === 0 ? "CSV" : "JSON", url: `${ENDPOINT}file-${n}` });
const DETAIL = {
  name: "bikes",
  title: "City bikes",
  keywords: [],
  publisher: { name: "helsinki", title: "Helsinki" },
  themes: [],
  spatial: [],
  catalogueUrl: "https://opendata.dev.joinedcontext.com/dataset/bikes",
  resources: [1, 2, 3, 4, 5, 6].map(resource),
  endpoint: { url: ENDPOINT, representations: ["ngsi-ld", "csv"] },
  model: { name: "bikes", classes: [{ name: "Station", description: "A docking station." }], docsUrl: `${ENDPOINT}schema/1/bikes.v1.md` },
};

type Answer = { status: number; body: unknown };
let answers: Record<string, Answer>;
let grants: unknown[];
let seen: URL[];

function renderAt(path: string) {
  window.history.pushState({}, "", path);
  seen = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
      seen.push(url);
      let answer: Answer;
      if (url.pathname.includes("/auth/me")) answer = { status: 200, body: IDENTITY };
      else if (url.pathname.endsWith("/permissions/me")) answer = { status: 200, body: { project: "helsinki", bootstrap: false, grants } };
      else answer = answers[url.pathname] ?? { status: 200, body: { apiVersion: API, kind: "List", items: [] } };
      // The page the person asked for, as the server numbers it.
      if (url.pathname === "/api/v1/catalogue" && answer.body === PAGE) answer = { status: 200, body: { ...PAGE, page: Number(url.searchParams.get("page") ?? 1) } };
      return Promise.resolve(
        new Response(JSON.stringify(answer.body), {
          status: answer.status,
          headers: { "Content-Type": answer.status >= 400 ? "application/problem+json" : "application/json" },
        }),
      );
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const reads = (pathname: string) => seen.filter((url) => url.pathname === pathname);

beforeEach(async () => {
  await i18n.changeLanguage("en");
  grants = [];
  answers = {
    "/api/v1/projects": { status: 200, body: { apiVersion: API, kind: "List", items: [{ name: "helsinki" }] } },
    "/api/v1/catalogue": { status: 200, body: PAGE },
    "/api/v1/catalogue/datasets/bikes": { status: 200, body: DETAIL },
  };
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the catalogue list", () => {
  it("says why the list did not come and asks again on Retry", async () => {
    answers["/api/v1/catalogue"] = { status: 500, body: { title: "Internal Server Error", status: 500, detail: "The catalogue index is rebuilding" } };
    renderAt("/catalogue");
    expect(await screen.findByText(/The catalogue index is rebuilding/)).toBeInTheDocument();
    const before = reads("/api/v1/catalogue").length;
    answers["/api/v1/catalogue"] = { status: 200, body: PAGE };
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("link", { name: "City bikes" })).toBeInTheDocument();
    expect(reads("/api/v1/catalogue").length).toBeGreaterThan(before);
  });

  it("steps back and forward through the pages, and asks for the page it steps to", async () => {
    renderAt("/catalogue?page=2");
    const previous = await screen.findByRole("button", { name: en.catalogue.pages.previous });
    await userEvent.click(previous);
    await waitFor(() => expect(window.location.search).not.toContain("page=2"));
    const next = await screen.findByRole("button", { name: en.catalogue.pages.next });
    await userEvent.click(next);
    await waitFor(() => expect(window.location.search).toContain("page=2"));
    await waitFor(() => expect(reads("/api/v1/catalogue").some((url) => url.searchParams.get("page") === "2")).toBe(true));
  });

  it("offers a clear button for a search that matches nothing, and the list is back after it", async () => {
    answers["/api/v1/catalogue"] = { status: 200, body: { ...PAGE, total: 0, datasets: [], facets: { ...PAGE.facets, publisher: [], theme: [] } } };
    renderAt("/catalogue?q=zzzz");
    expect(await screen.findByText(en.catalogue.results.noMatch)).toBeInTheDocument();
    answers["/api/v1/catalogue"] = { status: 200, body: PAGE };
    const empty = screen.getByText(en.catalogue.results.noMatch).closest("div") as HTMLElement;
    await userEvent.click(within(empty.parentElement as HTMLElement).getByRole("button", { name: en.catalogue.search.clear }));
    await waitFor(() => expect(window.location.search).not.toContain("zzzz"));
  });

  it("keeps a ticked publisher in the filter although this answer does not count it", async () => {
    renderAt("/catalogue?publisher=ghost");
    const filters = await screen.findByRole("complementary", { name: "Filters" });
    const publisher = await within(filters).findByRole("group", { name: "Publisher" });
    expect(await within(publisher).findByRole("checkbox", { name: /ghost/ })).toBeChecked();
  });

  it("offers a person with the right to propose an endpoint the way to publish a dataset", async () => {
    grants = [{ role: "r", binding: "b", scope: "project", rule: { kinds: ["Endpoint"], verbs: ["read", "propose"] } }];
    answers["/api/v1/catalogue"] = { status: 200, body: { ...PAGE, total: 0, datasets: [], facets: { ...PAGE.facets, publisher: [], theme: [] } } };
    renderAt("/catalogue");
    // An empty catalogue offers the way to fill it, in place of the clear button of a narrowed one.
    const empty = (await screen.findByText(en.catalogue.results.empty)).closest("div") as HTMLElement;
    await userEvent.click(await within(empty.parentElement as HTMLElement).findByRole("button", { name: en.catalogue.publish.open }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });
});

describe("a dataset page", () => {
  it("says why the dataset did not come and asks again on Retry", async () => {
    answers["/api/v1/catalogue/datasets/bikes"] = { status: 500, body: { title: "Internal Server Error", status: 500, detail: "The dataset is being rebuilt" } };
    renderAt("/catalogue/bikes");
    expect(await screen.findByText(/The dataset is being rebuilt/)).toBeInTheDocument();
    answers["/api/v1/catalogue/datasets/bikes"] = { status: 200, body: DETAIL };
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("heading", { level: 1, name: "City bikes" })).toBeInTheDocument();
  });

  it("searches a long list of resources by name and by format", async () => {
    renderAt("/catalogue/bikes");
    const grid = await screen.findByRole("region", { name: en.catalogue.dataset.resources });
    const search = within(grid).getByRole("searchbox", { name: en.dataGrid.searchRows });
    fireEvent.change(search, { target: { value: "Resource 3" } });
    expect(within(grid).getByText("Resource 3")).toBeInTheDocument();
    expect(within(grid).queryByText("Resource 4")).toBeNull();
    fireEvent.change(search, { target: { value: "CSV" } });
    await waitFor(() => expect(within(grid).queryByText("Resource 3")).toBeNull());
    expect(within(grid).getByText("Resource 4")).toBeInTheDocument();
    // A needle no column holds leaves no resource.
    fireEvent.change(search, { target: { value: "no such resource" } });
    await waitFor(() => expect(within(grid).queryByText("Resource 4")).toBeNull());
    expect(within(grid).queryByText("Resource 1")).toBeNull();
  });

  it("shows no 'use this data' block for an endpoint that serves nothing to copy", async () => {
    answers["/api/v1/catalogue/datasets/bikes"] = { status: 200, body: { ...DETAIL, endpoint: { url: ENDPOINT, representations: ["ogc-features"] } } };
    renderAt("/catalogue/bikes");
    await screen.findByRole("heading", { level: 1, name: "City bikes" });
    expect(screen.queryByRole("heading", { name: en.catalogue.use.title })).toBeNull();
  });

  it("says copied when the clipboard takes the snippet, and goes back to Copy when it refuses", async () => {
    const writeText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("denied"));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    renderAt("/catalogue/bikes");
    const copy = (await screen.findAllByRole("button", { name: new RegExp(`^${en.catalogue.use.copy.replace("{what}", "")}`) }))[0];
    await userEvent.click(copy);
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(String(writeText.mock.calls[0][0])).toContain(SLUG);
    const copied = await screen.findByRole("button", { name: en.catalogue.use.copied });
    await userEvent.click(copied);
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("button", { name: en.catalogue.use.copied })).toBeNull());
    Reflect.deleteProperty(navigator, "clipboard");
  });
});
