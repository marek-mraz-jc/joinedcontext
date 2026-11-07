/** T-0529: the explorer's "Your access" panel from the endpoint's AuthZEN surfaces (UI-33, R51, R17). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import { App } from "../src/App";
import * as AccessPanelModule from "../src/components/entities/AccessPanel";
import { deniedOf, writableOf } from "../src/api/access";

const { CHECKED_ACTIONS } = AccessPanelModule;

const IDENTITY = { subject: "b7c1e0f4", username: "demo.viewer", name: "Demo Viewer", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: {
    version: "1.0.0",
    classes: ["BikeHireDockingStation"],
    linkml: [
      "id: https://hel.fi/models/bikes",
      "name: bikes",
      "classes:",
      "  BikeHireDockingStation:",
      "    slots: [id, availableBikeNumber, name, location]",
      "slots:",
      "  id: {}",
      "  availableBikeNumber: { range: integer }",
      "  name: { range: string }",
      "  location: { range: string }",
      "",
    ].join("\n"),
  },
};
const SPACES = list([
  { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "mobility", namespace: "helsinki" }, spec: { dataModelRef: "bikes" } },
]);
const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-bikes", namespace: "helsinki" },
    spec: { contextSpaceRef: "mobility", slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", audience: "public" },
  },
]);
/** The public grant on the bikes endpoint: reads only, two attributes, never the location. */
const GRANTS = {
  subject: { type: "role", id: "public" },
  resource: { type: "endpoint", id: "scsd2eehkx42n53z2zyd6vshfh7s7irf", space: "helsinki" },
  permissions: [
    {
      resource: { type: "BikeHireDockingStation" },
      actions: ["retrieveEntity", "queryEntity"],
      attributes: ["availableBikeNumber", "name", "location"],
      constraints: { scopeQ: "/helsinki/#" },
    },
  ],
  prohibitions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity"], attributes: ["location"], constraints: {} }],
};

/** The gateway's own decisions per attribute for the same grant (T-5528). */
const ATTRIBUTES = [
  { name: "availableBikeNumber", read: true, write: false, delete: false, reason: "grant" },
  { name: "name", read: true, write: false, delete: false, reason: "grant" },
  { name: "location", read: false, write: false, delete: false, reason: "prohibited" },
];

function renderExplore() {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/access/v1/evaluations")) {
      return request
        .clone()
        .json()
        .then((body: { evaluations: { action: { name: string } }[] }) =>
          json({ evaluations: body.evaluations.map((item) => ({ decision: item.action.name === "queryEntity" })) }),
        );
    }
    if (url.pathname.endsWith("/access/check")) {
      return request
        .clone()
        .json()
        .then((body: { action: { name: string } }) => json({ decision: body.action.name === "queryEntity" }));
    }
    if (url.pathname.endsWith("/access/attributes")) return json({ type: url.searchParams.get("type"), attributes: ATTRIBUTES });
    if (url.pathname.endsWith("/access")) return json(GRANTS);
    if (url.pathname.includes("/ngsi-ld/v1/entities")) return json([]);
    if (url.pathname.endsWith("/spaces")) return json(SPACES);
    if (url.pathname.endsWith("/endpoints")) return json(ENDPOINTS);
    if (url.pathname.endsWith("/datamodels")) return json(list([MODEL]));
    return json(list([]));
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

describe("your access in the explorer", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/explore");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("renders the grant and the dry-run decisions, and disables a denied attribute with the reason", async () => {
    const fetchMock = renderExplore();
    await screen.findByRole("option", { name: "mobility" });
    await userEvent.selectOptions(screen.getByLabelText(en.explore.space), "mobility");
    const panel = await screen.findByRole("region", { name: en.access.panel.title });

    await userEvent.selectOptions(screen.getByLabelText(en.entities.type), "BikeHireDockingStation");
    expect(await within(panel).findByText(/retrieveEntity, queryEntity/)).toBeInTheDocument();
    expect(within(panel).getByText(/scopeQ=\/helsinki\/#/)).toBeInTheDocument();
    // Every action in one batch, with the user's session, answered before the query runs (T-5392).
    expect(await within(panel).findByText("You may read BikeHireDockingStation")).toBeInTheDocument();
    expect(within(panel).getByText("You may not change BikeHireDockingStation")).toBeInTheDocument();
    // T-4264: the operation names wait behind the disclosure, closed.
    expect(within(panel).getByText(/retrieveEntity, queryEntity/).closest("details")).not.toHaveAttribute("open");
    const asked = fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request instanceof Request);
    const batches = asked.filter((request) => request.url.endsWith("/access/v1/evaluations"));
    expect(batches).toHaveLength(1);
    expect(batches[0].credentials).toBe("same-origin");
    expect(await batches[0].clone().json()).toEqual({
      resource: { type: "BikeHireDockingStation" },
      evaluations: [{ action: { name: "queryEntity" } }, { action: { name: "updateAttrs" } }],
    });
    // The panel asks nothing one action at a time; only the explorer's own deleteEntity decision
    // behind its Delete button (T-5530) stays a single check.
    const checks = asked.filter((request) => request.url.endsWith("/access/check"));
    expect(checks).toHaveLength(1);
    expect(await checks[0].clone().json()).toMatchObject({ action: { name: "deleteEntity" } });

    // The prohibited attribute cannot be asked for, and the reason travels with it. Since
    // T-1756 `Checkbox` carries `disabledReason` the way `Button` does: the reason is a
    // *description* beside the box rather than part of its name, and the box is `aria-disabled`
    // so a keyboard can still land on it and be told why (UI-44).
    const location = screen.getByRole("checkbox", { name: "location" });
    expect(location).toHaveAttribute("aria-disabled", "true");
    expect(location).toHaveAccessibleDescription(en.access.panel.attrProhibited);
    expect(screen.getByRole("checkbox", { name: "name" })).toBeEnabled();
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "availableBikeNumber" })).toBeEnabled());
  });

  /// T-5528: the grid's editors and the filters' hidden marks come from the gateway's own
  /// per-attribute answer; the browser computes no grant of its own any more.
  it("takes the hidden attributes and the writable ones from the gateway's answer", () => {
    const t = (key: string) => key;
    const rows = [
      { name: "speed", read: true, write: true, delete: false, reason: "grant" as const },
      { name: "location", read: false, write: false, delete: false, reason: "prohibited" as const },
      { name: "owner", read: false, write: false, delete: false, reason: "not_in_grant" as const },
      { name: "total", read: true, write: true, delete: false, reason: "formula" as const },
    ];
    expect(deniedOf(rows, t)).toEqual({ location: "access.panel.attrProhibited", owner: "access.panel.attrDenied" });
    expect(writableOf(rows)).toEqual(["speed"]);
    expect(deniedOf(undefined, t)).toEqual({});
    expect(writableOf(undefined)).toEqual([]);
  });

  it("leaves no grant computation in the browser", () => {
    expect(Object.keys(AccessPanelModule)).not.toContain("deniedAttributes");
    expect(Object.keys(AccessPanelModule)).not.toContain("editableAttributes");
  });
});

/// T-4264, T-4160 #18: every action the panel checks is a word in each language, never the
/// operation name, which the Slovak page used to show as "Smiete queryEntity".
describe("the checked actions in words", () => {
  it("names each checked action in every catalogue", () => {
    for (const catalogue of [en, sk, cs, de]) {
      for (const action of CHECKED_ACTIONS) {
        const word = (catalogue.access.panel.actions as Record<string, string>)[action];
        expect(word, action).toBeTruthy();
        expect(word).not.toBe(action);
      }
    }
  });
});
