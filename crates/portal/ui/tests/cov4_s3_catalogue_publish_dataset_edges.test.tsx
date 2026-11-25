// T-7706: PublishDataset edges: the publisher picked from the catalogue or typed new, and Back from the draft to the endpoint pick.
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const LIST = "joinedcontext.com/v1alpha1";
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] };

const ENDPOINT = {
  apiVersion: LIST,
  kind: "Endpoint",
  metadata: { name: "bbsk-kpi", namespace: "bbsk", title: { sk: "Ukazovatele kraja" } },
  spec: { contextSpaceRef: "kpi", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "organization", enabledRepresentations: ["ngsi-ld", "csv"], projection: { hiddenAttributes: ["internalNote"] } },
  status: { phase: "Live" },
};

const DRAFT = {
  endpoint: "bbsk-kpi",
  makesPublic: true,
  catalog: {
    license: "CC_BY_4_0",
    themes: ["ECON"],
    keywords: { sk: ["ukazovateľ", "hodnota"] },
    publisher: { name: { sk: "Banskobystrický samosprávny kraj" } },
    contactPoint: { name: "Open data desk", email: "opendata@example.org" },
    attribution: { sk: "Zdroj: BBSK" },
  },
  publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "bbsk" } } },
  missing: ["spatial", "temporal", "frequency"],
  attributes: [
    { type: "KeyPerformanceIndicator", name: "kpiValue" },
    { type: "KeyPerformanceIndicator", name: "location" },
    { type: "KeyPerformanceIndicator", name: "comment" },
    { type: "KeyPerformanceIndicator", name: "reporter", withheld: "personal" },
    { type: "KeyPerformanceIndicator", name: "internalNote", withheld: "hidden" },
  ],
};

const PAGE = {
  total: 0,
  page: 1,
  pageSize: 20,
  datasets: [],
  facets: {
    publisher: [{ value: "mesto-bb", label: "Mesto Banská Bystrica", count: 4 }],
    theme: [], format: [], licence: [], spatial: [], year: [],
    keyword: [{ value: "ovzdušie", label: "ovzdušie", count: 7 }],
  },
  unavailable: [],
};


function stub(): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      const path = url.pathname;
      if (path.includes("/auth/me")) return json(IDENTITY);
      if (path === "/api/v1/projects") return json({ apiVersion: LIST, kind: "List", items: [{ name: "bbsk" }] });
      if (path.endsWith("/permissions/me")) return json({ project: "bbsk", bootstrap: true, grants: [] });
      if (path === "/api/v1/catalogue") return json(PAGE);
      if (path === "/api/v1/catalogue/readable") return json({ datasets: [], unanswered: 0 });
      if (path === "/api/v1/projects/bbsk/catalogue/drafts") return json(DRAFT);
      if (path === "/api/v1/projects/bbsk/endpoints") return json({ apiVersion: LIST, kind: "List", items: [ENDPOINT] });
      if (path === "/api/v1/projects/bbsk/endpoints/bbsk-kpi") return json(ENDPOINT);
      return json({ apiVersion: LIST, kind: "List", items: [] });
    }),
  );
}

async function openDraft(user: ReturnType<typeof userEvent.setup>): Promise<HTMLElement> {
  stub();
  window.history.pushState({}, "", "/catalogue");
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  await user.click(await screen.findByRole("button", { name: "Publish a dataset" }));
  const dialog = await screen.findByRole("dialog", { name: "Publish a dataset" });
  await user.click(within(dialog).getByRole("combobox", { name: "Endpoint" }));
  await user.click(await within(dialog).findByRole("option", { name: /Ukazovatele kraja/ }));
  await user.click(within(dialog).getByRole("button", { name: "Draft the description" }));
  await within(dialog).findByDisplayValue("Banskobystrický samosprávny kraj");
  return dialog;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the draft of a dataset, the edges", () => {
  it("takes the publisher the catalogue already holds, and a new one typed in", async () => {
    const user = userEvent.setup();
    const dialog = await openDraft(user);
    const publisher = within(dialog).getByRole("combobox", { name: "Publisher" });
    await user.click(publisher);
    await user.click(await within(dialog).findByRole("option", { name: /Mesto Banská Bystrica/ }));
    expect(await within(dialog).findByDisplayValue("Mesto Banská Bystrica")).toBeInTheDocument();
    const typed = within(dialog).getByRole("combobox", { name: "Publisher" });
    await user.clear(typed);
    await user.type(typed, "Kraj Nitra");
    await user.click(await within(dialog).findByRole("option", { name: /New: “Kraj Nitra”/ }));
    expect(await within(dialog).findByDisplayValue("Kraj Nitra")).toBeInTheDocument();
  });

  it("goes back from the draft to the choice of the endpoint", async () => {
    const user = userEvent.setup();
    const dialog = await openDraft(user);
    await user.click(within(dialog).getByRole("button", { name: "Back" }));
    expect(await within(dialog).findByRole("button", { name: "Draft the description" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Preview the entry" })).not.toBeInTheDocument();
  });
});
