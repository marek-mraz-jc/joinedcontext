// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/catalogue/PublishDataset.tsx, PublicationPreview.tsx and publish.ts: the one-step publish
// flow (EP-83, T-2726) and its CKAN page preview (T-4847).
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { applyFields, catalogOf, columnsOf, formOf, hiddenOf, problems, textsWith } from "../src/pages/catalogue/publish";
import { PublishDatasetDialog } from "../src/pages/catalogue/PublishDataset";
import { PublicationPreview } from "../src/pages/catalogue/PublicationPreview";

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

const CHANGE = {
  apiVersion: LIST,
  kind: "Change",
  metadata: { name: "chg-0000a1b2", namespace: "bbsk" },
  status: { lane: "red", phase: "PendingApproval", plan: { update: 1 } },
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

type Sent = { method: string; url: URL; body: unknown };

describe("applyFields", () => {
  it("never walks into the prototype of every object", () => {
    // F-UI2-002: a Change field path is data; `__proto__` must not reach Object.prototype.
    const out = applyFields({}, [
      { path: "metadata.__proto__.polluted", to: true },
      { path: "constructor.prototype.polluted", to: true },
      { path: "metadata.name", to: "kept" },
    ] as Parameters<typeof applyFields>[1]);
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    expect(out).toEqual({ metadata: { name: "kept" } });
  });
});

describe("publish a dataset in one step (EP-83)", () => {
  let sent: Sent[];
  let draftAnswer: { status: number; body: unknown };

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    sent = [];
    draftAnswer = { status: 200, body: DRAFT };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  let readBesideDraft = false;
  function stub() {
    readBesideDraft = false;
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const text = request.method === "GET" ? "" : await request.text();
      sent.push({ method: request.method, url, body: text ? JSON.parse(text) : undefined });
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
        });
      const path = url.pathname;
      if (path.includes("/auth/me")) return json(IDENTITY);
      if (path === "/api/v1/projects") return json({ apiVersion: LIST, kind: "List", items: [{ name: "bbsk" }] });
      if (path.endsWith("/permissions/me")) return json({ project: "bbsk", bootstrap: true, grants: [] });
      if (path === "/api/v1/catalogue") return json(PAGE);
      if (path === "/api/v1/catalogue/readable") return json({ datasets: [], unanswered: 0 });
      if (path === "/api/v1/projects/bbsk/catalogue/drafts") {
        // F-R2UI2-018: the endpoint is read beside the draft, not after its answer.
        await new Promise((resolve) => setTimeout(resolve, 20));
        readBesideDraft = sent.some((s) => s.method === "GET" && s.url.pathname === "/api/v1/projects/bbsk/endpoints/bbsk-kpi");
        return json(draftAnswer.body, draftAnswer.status);
      }
      if (path === "/api/v1/projects/bbsk/endpoints" && request.method === "GET") {
        return json({ apiVersion: LIST, kind: "List", items: [ENDPOINT] });
      }
      if (path === "/api/v1/projects/bbsk/endpoints/bbsk-kpi" && request.method === "GET") return json(ENDPOINT);
      if (path === "/api/v1/projects/bbsk/endpoints/bbsk-kpi" && request.method === "PUT") {
        return url.searchParams.get("dryRun") === "All" ? json({ valid: true }) : json(CHANGE, 202);
      }
      return json({ apiVersion: LIST, kind: "List", items: [] });
    });
    vi.stubGlobal("fetch", fetchMock);
  }

  function client() {
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }

  it("drafts, lets the person correct it, previews and proposes one Change that makes the endpoint public", async () => {
    const user = userEvent.setup();
    stub();
    window.history.pushState({}, "", "/catalogue");
    render(
      <QueryClientProvider client={client()}>
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

    expect(await within(dialog).findByDisplayValue("Banskobystrický samosprávny kraj")).toBeInTheDocument();
    const drafts = sent.find((s) => s.url.pathname.endsWith("/catalogue/drafts"));
    expect(drafts?.body).toEqual({ endpoint: "bbsk-kpi" });
    expect(readBesideDraft).toBe(true);
    expect(within(dialog).getAllByText(/not public yet/).length).toBeGreaterThan(0);
    expect(within(dialog).getByText(/area, period, update frequency/)).toBeInTheDocument();
    expect(within(dialog).getByRole("checkbox", { name: "Economy and finance" })).toBeChecked();

    // A licence is required: the flow says so on the field rather than proposing without one.
    await user.selectOptions(within(dialog).getByRole("combobox", { name: /Licence/ }), "");
    await user.click(within(dialog).getByRole("button", { name: "Preview the entry" }));
    expect(await within(dialog).findByText("Choose a licence.")).toBeInTheDocument();
    await user.selectOptions(within(dialog).getByRole("combobox", { name: /Licence/ }), "CC0");
    await user.click(within(dialog).getByRole("checkbox", { name: "Environment" }));
    // Keywords and publishers the catalogue holds are offered, free entry kept (T-4948).
    const keywords = within(dialog).getByRole("combobox", { name: "Keywords" });
    expect(keywords).toHaveAccessibleDescription(/type a new one/);
    expect(within(dialog).getByRole("list", { name: /Keywords/ })).toHaveTextContent(/ukazovateľ.*hodnota/);
    await user.type(keywords, "ovz");
    await user.click(await within(dialog).findByRole("option", { name: /ovzdušie/ }));
    await user.type(keywords, "smog");
    await user.click(await within(dialog).findByRole("option", { name: /New: “smog”/ }));
    await user.click(within(dialog).getByRole("combobox", { name: "Publisher" }));
    expect(await within(dialog).findByRole("option", { name: /Mesto Banská Bystrica/ })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog", { name: "Publish a dataset" })).toBeInTheDocument();
    await user.selectOptions(within(dialog).getByRole("combobox", { name: "Area on the map" }), "SK032");

    // A title is required and in the space's language; the description is optional.
    const title = within(dialog).getByRole("textbox", { name: /Dataset title/ });
    expect(title).toHaveValue("Ukazovatele kraja");
    await user.clear(title);
    await user.click(within(dialog).getByRole("button", { name: "Preview the entry" }));
    expect(await within(dialog).findByText("Give the dataset a title.")).toBeInTheDocument();
    await user.type(title, "Ukazovatele kraja 2026");
    await user.type(within(dialog).getByRole("textbox", { name: /Description/ }), "Mesačné ukazovatele kraja.");

    // Personal and already hidden attributes are shown greyed with the reason, never offered.
    expect(within(dialog).getByRole("checkbox", { name: /reporter/ })).toBeDisabled();
    expect(within(dialog).getByText("Personal data in the model: a public dataset never carries it.")).toBeInTheDocument();
    expect(within(dialog).getByRole("checkbox", { name: /internalNote/ })).toBeDisabled();
    expect(within(dialog).getByRole("checkbox", { name: /kpiValue/ })).toBeChecked();
    await user.click(within(dialog).getByRole("checkbox", { name: /comment/ }));
    // CKAN holds no rows (T-5060): the form offers no table to fill.
    expect(within(dialog).queryByRole("checkbox", { name: /CKAN table/ })).toBeNull();
    fireEvent.change(within(dialog).getByLabelText("Covers data from"), { target: { value: "2020-01-01" } });
    await user.click(within(dialog).getByRole("button", { name: "Preview the entry" }));

    const preview = await within(dialog).findByRole("region", { name: "3. Preview" });
    expect(within(preview).getByText("Creative Commons Zero 1.0")).toBeInTheDocument();
    expect(within(preview).getByText("Environment")).toBeInTheDocument();
    expect(within(preview).getByRole("heading", { name: "Ukazovatele kraja 2026" })).toBeInTheDocument();
    expect(within(preview).getByText("Banskobystrický kraj")).toBeInTheDocument();
    const resources = within(preview).getByRole("list", { name: "Data and resources" });
    expect(within(resources).queryByText(/Table in CKAN/)).toBeNull();
    expect(within(resources).getByText(/Map of the records/)).toBeInTheDocument();
    expect(within(resources).getByText(/DCAT-AP record/)).toBeInTheDocument();
    const columns = within(preview).getByRole("list", { name: "Columns" });
    expect(within(columns).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["kpiValue", "location"]);

    // Public is the red lane: the proposer types the endpoint's name before the button acts.
    const proposeButton = within(dialog).getByRole("button", { name: "Make public and propose publication" });
    expect(proposeButton).toHaveAttribute("aria-disabled", "true");
    await user.click(proposeButton);
    expect(sent.some((s) => s.method === "PUT")).toBe(false);
    await user.type(within(dialog).getByRole("textbox", { name: /Type the endpoint's name/ }), "bbsk-kpi");
    await user.click(within(dialog).getByRole("button", { name: "Make public and propose publication" }));
    await within(dialog).findByText(/next reconcile/);

    const puts = sent.filter((s) => s.method === "PUT");
    expect(puts.map((p) => p.url.searchParams.get("dryRun"))).toEqual(["All", null]);
    const body = puts[1].body as { metadata: Record<string, unknown>; spec: Record<string, unknown>; status?: unknown };
    expect(body.status).toBeUndefined();
    expect(body.spec.audience).toBe("public");
    expect(body.spec.publish).toEqual(DRAFT.publish);
    expect(body.metadata.title).toEqual({ sk: "Ukazovatele kraja 2026" });
    expect(body.metadata.description).toEqual({ sk: "Mesačné ukazovatele kraja." });
    // The personal slot is hidden, so the public projection is never refused (DM-122).
    expect(body.spec.projection).toEqual({ hiddenAttributes: ["comment", "internalNote", "reporter"] });
    expect(body.spec.contextSpaceRef).toBe("kpi");
    expect(body.spec.catalog).toEqual({
      license: "CC0",
      themes: ["ECON", "ENVI"],
      keywords: { sk: ["ukazovateľ", "hodnota", "ovzdušie", "smog"] },
      publisher: { name: { sk: "Banskobystrický samosprávny kraj" } },
      contactPoint: { name: "Open data desk", email: "opendata@example.org" },
      attribution: { sk: "Zdroj: BBSK" },
      spatial: ["SK032"],
      temporal: { start: "2020-01-01" },
    });
  });

  it("says what the API said when the project has no catalogue to publish to", async () => {
    const user = userEvent.setup();
    draftAnswer = {
      status: 409,
      body: { title: "Conflict", status: 409, detail: "project 'bbsk' has no CkanInstance to publish to; add one on its Open data page first" },
    };
    stub();
    window.history.pushState({}, "", "/catalogue");
    render(
      <QueryClientProvider client={client()}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await user.click(await screen.findByRole("button", { name: "Publish a dataset" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("combobox", { name: "Endpoint" }));
    await user.click(await within(dialog).findByRole("option", { name: /Ukazovatele kraja/ }));
    await user.click(within(dialog).getByRole("button", { name: "Draft the description" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("has no CkanInstance");
    expect(sent.some((s) => s.method === "PUT")).toBe(false);
  });

  it("proposes an already public endpoint without changing its audience", async () => {
    const user = userEvent.setup();
    draftAnswer = { status: 200, body: { ...DRAFT, makesPublic: false } };
    stub();
    window.history.pushState({}, "", "/catalogue");
    render(
      <QueryClientProvider client={client()}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await user.click(await screen.findByRole("button", { name: "Publish a dataset" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("combobox", { name: "Endpoint" }));
    await user.click(await within(dialog).findByRole("option", { name: /Ukazovatele kraja/ }));
    await user.click(within(dialog).getByRole("button", { name: "Draft the description" }));
    await user.click(await within(dialog).findByRole("button", { name: "Preview the entry" }));
    expect(within(dialog).queryByText(/not public yet/)).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Propose publication" }));
    await within(dialog).findByText(/next reconcile/);
    const put = sent.filter((s) => s.method === "PUT")[1].body as { spec: Record<string, unknown> };
    expect(put.spec.audience).toBe("organization");
  });

  it("opens nothing while closed", () => {
    stub();
    render(
      <QueryClientProvider client={client()}>
        <I18nextProvider i18n={i18n}>
          <PublishDatasetDialog project="bbsk" open={false} onOpenChange={() => undefined} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(sent).toEqual([]);
  });
});

describe("the publication's attributes and the approval's preview", () => {
  const attributes = DRAFT.attributes as Parameters<typeof hiddenOf>[1];

  it("hides every personal attribute and each one left out, keeping what was hidden", () => {
    expect(hiddenOf(["internalNote"], attributes, ["comment"])).toEqual(["comment", "internalNote", "reporter"]);
    expect(hiddenOf(undefined, [], [])).toEqual([]);
    expect(hiddenOf("not a list", attributes, [])).toEqual(["reporter"]);
    expect(columnsOf(attributes, ["comment"])).toEqual(["kpiValue", "location"]);
  });

  it("sets a text in one language and keeps the others", () => {
    expect(textsWith({ sk: "A", en: "B" }, "sk", " C ")).toEqual({ sk: "C", en: "B" });
    expect(textsWith("A", "sk", "")).toBeUndefined();
    expect(textsWith(undefined, "de", "X")).toEqual({ de: "X" });
  });

  it("previews a dataset without known columns: the map stays, no column list", async () => {
    await i18n.changeLanguage("en");
    render(
      <I18nextProvider i18n={i18n}>
        <PublicationPreview label="Preview" title="Dopravné nehody" description="" catalog={{ license: "CC0" }} />
      </I18nextProvider>,
    );
    const preview = screen.getByRole("region", { name: "Preview" });
    expect(within(preview).getByRole("heading", { name: "Dopravné nehody" })).toBeInTheDocument();
    expect(within(preview).queryByText(/Table in CKAN/)).toBeNull();
    expect(within(preview).getByText(/Map of the records/)).toBeInTheDocument();
    expect(within(preview).queryByRole("list", { name: "Columns" })).toBeNull();
  });

  it("applies a change's field diff to the live manifest", () => {
    const live = { metadata: { name: "e", title: { sk: "Old" } }, spec: { catalog: { themes: ["ECON", "ENVI"] } } };
    const head = applyFields(live, [
      { path: "metadata.title.sk", to: "New" },
      { path: "spec.catalog.themes[1]" },
      { path: "spec.catalog.keywords.sk[0]", to: "kpi" },
      { path: "spec.publish.ckan.name", to: "kpi" },
    ]);
    expect(head).toEqual({
      metadata: { name: "e", title: { sk: "New" } },
      spec: {
        catalog: { themes: ["ECON"], keywords: { sk: ["kpi"] } },
        publish: { ckan: { name: "kpi" } },
      },
    });
    expect(live.spec.catalog.themes).toEqual(["ECON", "ENVI"]);
  });
});

describe("the publish form", () => {
  const draft = DRAFT.catalog;

  it("reads the draft in its own language and writes back only what changed", () => {
    const form = formOf(draft, "en");
    expect(form.language).toBe("sk");
    expect(form.keywords).toBe("ukazovateľ, hodnota");
    expect(catalogOf(draft, form)).toEqual(draft);
  });

  it("drops a member the person emptied and keeps the other languages", () => {
    const both = { ...draft, keywords: { sk: ["a"], en: ["b"] } };
    const form = { ...formOf(both, "en"), keywords: " , ", publisher: "", contactName: "", contactEmail: "" };
    const out = catalogOf(both, form);
    expect(out.keywords).toEqual({ en: ["b"] });
    expect(out.publisher).toBeUndefined();
    expect(out.contactPoint).toBeUndefined();
    expect(out.attribution).toEqual({ sk: "Zdroj: BBSK" });
  });

  it("refuses a missing licence, half a contact, a bad address and a period that ends before it starts", () => {
    const form = formOf(draft, "en");
    expect(problems(form)).toEqual({});
    expect(problems({ ...form, license: "" })).toEqual({ license: "catalogue.publish.error.licence" });
    expect(problems({ ...form, contactEmail: "" })).toEqual({ contactEmail: "catalogue.publish.error.contactBoth" });
    expect(problems({ ...form, contactName: "" })).toEqual({ contactName: "catalogue.publish.error.contactBoth" });
    expect(problems({ ...form, contactEmail: "desk@" })).toEqual({ contactEmail: "catalogue.publish.error.email" });
    expect(problems({ ...form, temporalStart: "2025-02-01", temporalEnd: "2025-01-01" })).toEqual({
      temporalEnd: "catalogue.publish.error.period",
    });
  });

  it("a draft with nothing in it is a form with nothing in it", () => {
    const form = formOf({}, "de");
    expect(form.language).toBe("de");
    expect(catalogOf({}, form)).toEqual({});
  });
});
