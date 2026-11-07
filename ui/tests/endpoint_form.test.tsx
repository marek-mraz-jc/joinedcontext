import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { stringify as stringifyYaml } from "yaml";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { expectOpen } from "./checks";
import en from "../src/locales/en.json";
import { rememberPrefill } from "../src/assistant/state";
import { App } from "../src/App";
import { greenVerdict, isCheck } from "./verdict";
import { findFormPage } from "./formPage";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom: the stand-in
// is a textarea with the same contract, so the YAML view's own work is what runs.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const ORG = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Organization",
      metadata: { name: "bb", namespace: "banskabystrica" },
      spec: { domain: "hel.fi" },
    },
  ],
};

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "ovzdusie", namespace: "banskabystrica" },
      spec: {},
      status: { phase: "Live" },
    },
  ],
};

const LINKML_SOURCE = `
id: https://hel.fi/models/mobility
name: mobility
classes:
  Vehicle:
    slots:
      - id
      - name
      - type
      - location
      - speed
  User:
    slots:
      - id
      - name
      - type
      - age
      - email
slots:
  id:
    range: string
  type:
    range: string
  name:
    range: string
  location:
    range: string
  speed:
    range: integer
  age:
    range: integer
  email:
    range: string
`;

const DATAMODELS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name: "mobility-model", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "ovzdusie",
        version: "1",
        linkml: LINKML_SOURCE,
      },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-test-123", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 3 } },
};

const EXISTING = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "vehicles-live", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "ovzdusie",
        slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
        audience: "organization",
        enabledRepresentations: ["ngsi-ld"],
        projectionRef: { kind: "ModelProjection", name: "vehicles" },
      },
      status: { phase: "Live" },
    },
  ],
};

function setupTest(endpoints: unknown = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = typeof input === "string" ? new URL(input) : new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/organizations")) return json(ORG);
    if (url.pathname.endsWith("/spaces")) return json(SPACES);
    if (url.pathname.endsWith("/datamodels")) return json(DATAMODELS);
    if (url.pathname.endsWith("/projections")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    if (url.pathname.endsWith("/import")) {
      // The import door refuses a name the project already holds unless the caller says what to
      // do with it (MF-23). The form's own bundle names the projection after the endpoint, so a
      // second proposal for a space that already has one is refused here, not in the browser.
      return request
        .clone()
        .json()
        .then((body: { manifests?: { metadata?: { name?: string } }[]; conflictPolicy?: string }) => {
          const collides = (body.manifests ?? []).some(
            (m) => m.metadata?.name === "vehicles-again",
          );
          if (collides && body.conflictPolicy !== "replace") {
            return json(
              {
                detail:
                  "ModelProjection 'vehicles-again' already exists in project 'banskabystrica'; choose skip, replace or rename (MF-23)",
              },
              409,
            );
          }
          return json(CHANGE, 202);
        });
    }
    // A check answers a verdict fresh for the manifest it judged; without it the form proposes
    // nothing under strict validation (AG-62, PF-57, T-0779).
    if (isCheck(request, url)) {
      return greenVerdict(request, { valid: true, lane: "yellow" }).then((body) => json(body));
    }
    if (url.pathname.endsWith("/endpoints") && request.method === "POST") return json({ valid: true, lane: "yellow" });
    if (url.pathname.endsWith("/endpoints/vehicles-live")) return json(CHANGE, 202);
    if (url.pathname.endsWith("/endpoints")) return json(endpoints);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe("endpoint form with ModelPicker (T-0564)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
    document.cookie = "__Host-jc_csrf=tok-1; Secure; path=/";
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens with no complaint about its own arrangement while no rate is set (T-3142)", async () => {
    // The burst field is left out until a rate per minute is picked; the form said "the schema
    // has no field rateLimits.burst" in red over every new endpoint on dev.
    setupTest();
    // The header's button; an empty list offers the same one in its empty state (T-3246).
    await userEvent.click((await screen.findAllByRole("button", { name: en.endpoints.add }))[0]);
    const page = await findFormPage();
    expect(within(page).queryByText(en.form.uischemaProblems)).toBeNull();
    expect(within(page).queryByText(/rateLimits\.burst/)).toBeNull();
  });

  it("shows read-only slug and public URL without slug input or generate button", async () => {
    setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));

    expect(screen.getByTestId("endpoint-slug")).toBeInTheDocument();
    expect(screen.getByTestId("endpoint-url")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.endpoints.generateSlug })).not.toBeInTheDocument();
  });

  it("ticking classes and slots posts bundle to /import with ModelProjection, Endpoint and Policies", async () => {
    const fetchMock = setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));

    const dialog = await findFormPage();
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "public-vehicles");

    // Wait for ModelPicker
    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());

    // Tick Vehicle (ticks id, type, name, location, speed)
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));
    // Untick location, speed
    await userEvent.click(within(dialog).getByLabelText("Vehicle.location"));
    await userEvent.click(within(dialog).getByLabelText("Vehicle.speed"));

    // Tick User
    await userEvent.click(within(dialog).getByLabelText("User"));
    // Untick email
    await userEvent.click(within(dialog).getByLabelText("User.email"));

    // Identity slots id, type must be checked & disabled
    expect(within(dialog).getByLabelText("Vehicle.id")).toBeChecked();
    expect(within(dialog).getByLabelText("Vehicle.id")).toBeDisabled();
    expect(within(dialog).getByLabelText("Vehicle.type")).toBeChecked();
    expect(within(dialog).getByLabelText("Vehicle.type")).toBeDisabled();

    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => {
      // The check sends the bundle as a dry run first; the proposal is the one that writes.
      const importCalls = fetchMock.mock.calls.filter((c) => {
        const req = c[0] as Request;
        const u = typeof c[0] === "string" ? c[0] : req.url;
        return u.includes("/import") && !u.includes("dryRun");
      });
      expect(importCalls.length).toBe(1);
    });

    // The write itself is checked first with the same bundle: the import door needs the
    // check of exactly what it imports (PF-57, T-1460).
    const urls = fetchMock.mock.calls.map((c) =>
      typeof c[0] === "string" ? c[0] : (c[0] as Request).url,
    );
    const write = urls.findIndex((u) => u.includes("/import") && !u.includes("dryRun"));
    const checked = urls
      .slice(0, write)
      .map((u) => u.includes("/import?dryRun=All"))
      .lastIndexOf(true);
    expect(checked).toBeGreaterThanOrEqual(0);
    const bodyOf = (i: number) => (fetchMock.mock.calls[i][0] as Request).clone().text();
    expect(await bodyOf(checked)).toBe(await bodyOf(write));

    const importCall = fetchMock.mock.calls.find((c) => {
      const req = c[0] as Request;
      const u = typeof c[0] === "string" ? c[0] : req.url;
      return u.includes("/import");
    })![0] as Request;

    // The import door mutates, so it carries the session's double-submit token; without it the
    // Portal answers a bare 403 and the dialog only says "not checked yet" (T-0898).
    expect(importCall.headers.get("x-csrf-token")).toBe("tok-1");

    const payload = (await importCall.clone().json()) as { manifests: Record<string, unknown>[] };
    expect(payload.manifests).toBeDefined();
    expect(payload.manifests).toHaveLength(3); // ModelProjection, Endpoint, Read Policy

    const [proj, ep, readPol] = payload.manifests;
    expect(proj.kind).toBe("ModelProjection");
    expect((proj.spec as Record<string, unknown>).classes).toEqual([
      { name: "Vehicle", slots: ["name"] },
      { name: "User", slots: ["name", "age"] },
    ]);

    expect(ep.kind).toBe("Endpoint");
    expect((ep.spec as Record<string, unknown>).projectionRef).toEqual({
      kind: "ModelProjection",
      name: "public-vehicles",
    });

    expect(readPol.kind).toBe("Policy");
    // The organization as the placeholder the loader renders, never its domain (CC-82, T-1447).
    expect((readPol.spec as Record<string, unknown>).assigner).toBe("did:web:{orgDomain}");
    expect((readPol.spec as Record<string, unknown>).operations).toEqual(["retrieveOps"]);
    expect((readPol.spec as Record<string, unknown>).information).toEqual([
      { entities: [{ type: "Vehicle" }], propertyNames: ["name"] },
      { entities: [{ type: "User" }], propertyNames: ["name", "age"] },
    ]);
  });

  it("shows the model picker open, with what is ticked said above it (T-1389)", async () => {
    setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await findFormPage();
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "open-picker");

    // No click on a disclosure: the classes are there to tick.
    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeVisible());
    expect(within(dialog).queryByText(en.endpoints.section.projection)?.closest("summary")).toBeNull();
    expect(within(dialog).getByTestId("projection-summary")).toHaveTextContent("Publishing 0 types with 0 attributes.");
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));
    await waitFor(() =>
      expect(within(dialog).getByTestId("projection-summary")).not.toHaveTextContent("Publishing 0 types"),
    );
  });

  it("refuses proposal if space has model but nothing is ticked", async () => {
    setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await findFormPage();
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "empty-vehicles");

    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());

    // The bundle the check sends is the one that needs a class, so the check is what refuses.
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));

    // Beside the buttons that were pressed, not a screen above them (T-1424) — and the picker
    // itself says the same thing before any check, with the one action that settles it (T-2258),
    // so the sentence is on the screen twice and the footer is the one this case is about.
    expect(within(dialog).getByTestId("footer-error")).toHaveTextContent(en.endpoints.picker.nothingTicked);
    expect(
      within(within(dialog).getByRole("note")).getByRole("button", { name: en.form.useExample }),
    ).toBeInTheDocument();
  });

  it("proposes a projection the space already holds as an update, so the second share is checkable (MF-23, T-1227)", async () => {
    const fetchMock = setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await findFormPage();
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "vehicles-again");

    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));

    // The check reaches the endpoint's own dry run and its verdict, instead of dying on the
    // projection an earlier proposal drew.
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    const bundle = fetchMock.mock.calls
      .map((call) => call[0] as Request)
      .find((request) => request.url.includes("/import"))!;
    expect((await bundle.clone().json()).conflictPolicy).toBe("replace");
  });

  it("ticks the classes the assistant's proposal names, so its check is not refused (T-0895)", async () => {
    window.history.replaceState(null, "", "/projects/banskabystrica/endpoints");
    rememberPrefill("/projects/banskabystrica/endpoints", {
      name: "vehicles-regional",
      contextSpaceRef: "ovzdusie",
      audience: "project-list",
      allowedProjects: ["regional-transport"],
      enabledRepresentations: ["ngsi-ld"],
      hiddenAttributes: ["speed"],
      entityTypes: ["Vehicle"],
    });
    setupTest();
    const dialog = await findFormPage();
    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeChecked());
    expect(within(dialog).getByLabelText("Vehicle.speed")).toBeChecked();
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));
    await waitFor(() => expect(within(dialog).queryByText(en.endpoints.picker.nothingTicked)).not.toBeInTheDocument());
  });

  it("class ticked with no slots is identity only and shows text", async () => {
    setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await findFormPage();

    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));

    // Untick all non-identity slots: name, location, speed
    await userEvent.click(within(dialog).getByLabelText("Vehicle.name"));
    await userEvent.click(within(dialog).getByLabelText("Vehicle.location"));
    await userEvent.click(within(dialog).getByLabelText("Vehicle.speed"));

    expect(within(dialog).getByText(en.endpoints.picker.identityOnly, { exact: false })).toBeInTheDocument();
  });

  it("writable class with idPattern and scope creates write policy", async () => {
    const fetchMock = setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await findFormPage();
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "write-vehicles");

    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));

    // Mark writable. The three write fields are named after the class and the label the person
    // reads, in their own language, so the name matches the screen (WCAG 2.5.3, T-1843).
    await userEvent.click(
      within(dialog).getByLabelText(`Vehicle ${en.endpoints.picker.writable}`),
    );
    await userEvent.type(
      within(dialog).getByLabelText(`Vehicle ${en.endpoints.picker.idPattern}`),
      "urn:ngsi-ld:Vehicle:hel.fi:helsinki:*",
    );
    await userEvent.type(
      within(dialog).getByLabelText(`Vehicle ${en.endpoints.picker.scope}`),
      "/helsinki/bikes",
    );

    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => {
      // The check sends the bundle as a dry run first; the proposal is the one that writes.
      const importCalls = fetchMock.mock.calls.filter((c) => {
        const req = c[0] as Request;
        const u = typeof c[0] === "string" ? c[0] : req.url;
        return u.includes("/import") && !u.includes("dryRun");
      });
      expect(importCalls.length).toBe(1);
    });

    const importCall = fetchMock.mock.calls.find((c) => {
      const req = c[0] as Request;
      const u = typeof c[0] === "string" ? c[0] : req.url;
      return u.includes("/import");
    })![0] as Request;

    // The import door mutates, so it carries the session's double-submit token; without it the
    // Portal answers a bare 403 and the dialog only says "not checked yet" (T-0898).
    expect(importCall.headers.get("x-csrf-token")).toBe("tok-1");

    const payload = (await importCall.clone().json()) as { manifests: Record<string, unknown>[] };
    expect(payload.manifests).toHaveLength(4); // ModelProjection, Endpoint, Read Policy, Write Policy
    const writePol = payload.manifests[3];
    expect(writePol.kind).toBe("Policy");
    expect((writePol.spec as Record<string, unknown>).assigner).toBe("did:web:{orgDomain}");
    expect((writePol.spec as Record<string, unknown>).operations).toEqual(["updateOps"]);
    expect((writePol.spec as Record<string, unknown>).scopeQ).toBe("/helsinki/bikes");
    expect((writePol.spec as Record<string, unknown>).information).toEqual([
      {
        entities: [
          { type: "Vehicle", idPattern: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:*" },
        ],
      },
    ]);
  });

  // T-0763: writes of the page other than its drafts, as "METHOD path?query".
  const writes = (fetchMock: ReturnType<typeof setupTest>) =>
    fetchMock.mock.calls
      .map((call) => call[0] as Request)
      .filter((request) => request.method !== "GET" && !request.url.includes("/drafts"))
      .map((request) => ({ request, line: `${request.method} ${new URL(request.url).pathname}${new URL(request.url).search}` }));

  it("checks and proposes an existing endpoint alone, with the projection it names and no class ticked (T-0763)", async () => {
    const fetchMock = setupTest(EXISTING);
    // The row's actions are behind its one menu now (T-2287).
    await userEvent.click(await screen.findByRole("button", { name: /More actions/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.endpoints.edit }));
    const dialog = await findFormPage();

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const [check] = writes(fetchMock);
    expect(check.line).toBe("POST /api/v1/projects/banskabystrica/endpoints?dryRun=All");
    await expect(check.request.clone().json()).resolves.toMatchObject({
      draft: { kind: "Endpoint", name: "vehicles-live" },
      spec: { projectionRef: { kind: "ModelProjection", name: "vehicles" } },
    });

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(2));
    expect(writes(fetchMock)[1].line).toBe("PUT /api/v1/projects/banskabystrica/endpoints/vehicles-live");
    expect(writes(fetchMock).some((write) => write.line.includes("/import"))).toBe(false);
  });

  it("proposes a rate limit typed in the YAML view outside the classes, as the REST route would (T-0890)", async () => {
    const stored = EXISTING.items[0];
    const limited = { ...stored, spec: { ...stored.spec, rateLimits: { requestsPerMinute: 600, burst: 50 } } };
    const fetchMock = setupTest({ ...EXISTING, items: [limited] });
    // The row's actions are behind its one menu now (T-2287).
    await userEvent.click(await screen.findByRole("button", { name: /More actions/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.endpoints.edit }));
    const dialog = await findFormPage();
    await userEvent.click(within(dialog).getByRole("tab", { name: "YAML" }));
    const editor = await within(dialog).findByRole("textbox", { name: "YAML" });
    // 300 is no class: the form's select offers 60, 600 and 6000, plus what is stored.
    const pasted = { ...limited, status: undefined, spec: { ...limited.spec, rateLimits: { requestsPerMinute: 300, burst: 50 } } };
    fireEvent.change(editor, { target: { value: stringifyYaml(pasted) } });
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(2));
    expect(writes(fetchMock)[1].line).toBe("PUT /api/v1/projects/banskabystrica/endpoints/vehicles-live");
    const body = (await writes(fetchMock)[1].request.clone().json()) as {
      spec: { rateLimits?: { requestsPerMinute?: number } };
    };
    expect(body.spec.rateLimits?.requestsPerMinute).toBe(300);
    expect(within(dialog).queryByText(en.form.invalid)).toBeNull();
  });

  /// T-1059, EP-75: the slug is minted once and the API refuses a change to it. The form has
  /// no field for it, and a slug typed into the YAML view does not reach the proposal either —
  /// the endpoint keeps the one it was published under, whatever the text said.
  it("keeps the minted slug when the YAML view names another one", async () => {
    const stored = EXISTING.items[0];
    const fetchMock = setupTest(EXISTING);
    // The row's actions are behind its one menu now (T-2287).
    await userEvent.click(await screen.findByRole("button", { name: /More actions/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.endpoints.edit }));
    const dialog = await findFormPage();
    await userEvent.click(within(dialog).getByRole("tab", { name: "YAML" }));
    const editor = await within(dialog).findByRole("textbox", { name: "YAML" });
    const pasted = {
      ...stored,
      status: undefined,
      spec: { ...stored.spec, slug: "aaaaaaaaaaaaaaaaaaaaaaaaaa" },
    };
    fireEvent.change(editor, { target: { value: stringifyYaml(pasted) } });
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock).length).toBeGreaterThan(0));
    const proposal = writes(fetchMock)[writes(fetchMock).length - 1];
    const body = (await proposal.request.clone().json()) as { spec: { slug?: string } };
    expect(body.spec.slug).toBe((stored.spec as { slug: string }).slug);
  });

  it("checks a new endpoint's bundle whole and then its endpoint with its draft, so the verdict is the form's (T-0763)", async () => {
    const fetchMock = setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await findFormPage();
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "public-vehicles");
    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(2));
    const [bundle, endpoint] = writes(fetchMock);
    expect(bundle.line).toBe("POST /api/v1/projects/banskabystrica/import?dryRun=All");
    expect(endpoint.line).toBe("POST /api/v1/projects/banskabystrica/endpoints?dryRun=All");
    await expect(endpoint.request.clone().json()).resolves.toMatchObject({
      draft: { kind: "Endpoint", name: "public-vehicles" },
      spec: { projectionRef: { kind: "ModelProjection", name: "public-vehicles" } },
    });
  });
});
