// T-7706: EndpointPage and FilterProof gaps: retry of the failed page and of the policies, hidden attribute unticked, filter emptied, proof type switch, paging, retry, a count-less answer, box and condition faults.
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { boxFault } from "../src/pages/endpoints/EndpointPage";
import { shownValue } from "../src/pages/endpoints/FilterProof";
import { odataCall, odataValue } from "./odata";

const PROJECT = "helsinki";
const NAME = "helsinki-events";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const SEGMENT = "helsinki";

const IDENTITY = { subject: "b7c1e0f4", username: "aino", name: "Aino Virtanen", email: "aino@hel.fi", roles: ["portal-editor"] };
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: NAME, namespace: PROJECT, title: "Events", labels: { "joinedcontext.com/space": "helsinki" } },
  spec: {
    contextSpaceRef: "helsinki",
    slug: SLUG,
    audience: "public",
    enabledRepresentations: ["ngsi-ld"],
    projection: { hiddenAttributes: ["source"] },
    projectionRef: { kind: "ModelProjection", name: "events-open" },
  },
  status: { phase: "Live" },
};
const projectionOf = (classes: { name: string; slots: string[] }[], filter?: Record<string, string>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ModelProjection",
  metadata: { name: "events-open", namespace: PROJECT },
  spec: { contextSpaceRef: "helsinki", dataModelRef: { kind: "DataModel", name: "helsinki", version: "1" }, classes, filter },
});
const EVENT = { name: "Event", slots: ["name", "startDate", "source"] };
const PLACE = { name: "Place", slots: ["name"] };
const SPACE = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: PROJECT }, spec: { urnSegment: SEGMENT } };
const PERMITTED = {
  project: PROJECT,
  bootstrap: false,
  grants: [{ role: "endpoint-editor", binding: "editors", rule: { kinds: ["Endpoint", "ModelProjection"], verbs: ["propose", "delete"] } }],
};

const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "helsinki", namespace: PROJECT },
  spec: {
    contextSpaceRef: "helsinki",
    version: "1.0.0",
    linkml: ["id: https://hel.fi/models/helsinki", "name: helsinki", "default_range: string", "classes:", "  Event:", "    attributes:", "      name: {}", "      startDate: {range: datetime}", ""].join("\n"),
  },
};

const entity = (type: string, n: number) => ({
  id: `urn:ngsi-ld:${type}:hel.fi:helsinki:${n}`,
  type,
  name: { type: "Property", value: `${type} ${n}` },
});
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

interface Options {
  classes?: { name: string; slots: string[] }[];
  filter?: Record<string, string>;
  /** The count the space's canonical surface answers; absent: no count header. */
  total?: number | null;
  spaceReadable?: boolean;
  /** The saved endpoint's answer when the space is not readable. */
  saved?: "ok" | "text-500";
  policiesFail?: boolean;
  endpointFails?: boolean;
  spacesFail?: boolean;
  noProjectionRef?: boolean;
  savedNoCount?: boolean;
}

interface Seen {
  url: URL;
  method: string;
  body: string;
}

function renderPage(options: Options = {}) {
  const { classes = [EVENT], filter, total = 2, spaceReadable = true, saved = "ok", policiesFail = false } = options;
  let endpointFailures = options.endpointFails ? 1 : 0;
  let policyFailures = policiesFail ? 1 : 0;
  let spaceFailures = options.spacesFail ? 1 : 0;
  const seen: Seen[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input instanceof Request ? input : new Request(String(input));
    const url = new URL(request.url, window.location.origin);
    const path = url.pathname;
    const body = request.method === "GET" ? "" : await request.clone().text();
    seen.push({ url, method: request.method, body });
    const json = (value: unknown, status = 200, headers: Record<string, string> = {}) =>
      new Response(JSON.stringify(value), { status, headers: { "Content-Type": "application/json", ...headers } });
    const called = odataCall(url);

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/admission")) return json({ admitted: true });
    if (path.endsWith("/permissions/me")) return json(PERMITTED);
    if (path === `/cs/${SEGMENT}/ngsi-ld/v1/entities`) {
      if (!spaceReadable) return json({ title: "Not Found" }, 404);
      const type = url.searchParams.get("type") ?? "Event";
      const offset = Number(url.searchParams.get("offset") ?? "0");
      const headers: Record<string, string> = total === null ? {} : { "NGSILD-Results-Count": String(total) };
      return json([entity(type, offset + 1), entity(type, offset + 2)], 200, headers);
    }
    if (path.endsWith("/schema/index.json")) return json({ models: [{ name: "helsinki", version: 3 }] });
    if (path.endsWith("/schema/v3/json-schema")) return json({ $defs: { Event: { properties: { name: {}, startDate: {} } } } });
    if (path.endsWith("/access/attributes")) {
      const type = url.searchParams.get("type") ?? "";
      return json({ type, attributes: ["name", "startDate"].map((name) => ({ name, read: true, write: false, delete: false, reason: "not_in_grant" })) });
    }
    if (path.endsWith("/datamodels")) return json(list([MODEL]));
    if (path === `/api/endpoint/${SLUG}/access`) {
      return json({ permissions: [{ resource: { type: "Event" }, actions: ["queryEntity"], attributes: "*" }, { resource: { type: "Place" }, actions: ["queryEntity"], attributes: "*" }] });
    }
    if (called?.name === "jc_endpoint_preview") {
      const asked = called.params as { type?: string; id?: string[]; limit: number };
      const served = (asked.id ?? []).map((id) => ({ id, type: asked.type ?? "Event" }));
      return odataValue({ count: served.length, entities: served.slice(0, asked.limit) });
    }
    if (path === `/api/endpoint/${SLUG}/ngsi-ld/v1/entities`) {
      if (options.savedNoCount) return json([entity("Event", 1)]);
      if (saved === "text-500") return new Response("upstream gone", { status: 500, statusText: "" });
      return json([entity("Event", 1)], 200, { "NGSILD-Results-Count": "1" });
    }
    if (path.endsWith(`/endpoints/${NAME}`)) {
      if (request.method !== "GET") {
        return url.searchParams.get("dryRun") === "All"
          ? json({ valid: true, verdict: { ok: true, findings: [] } })
          : json({ kind: "Change", metadata: { name: "chg-endpoint" }, spec: {}, status: { phase: "Pending" } });
      }
      if (endpointFailures > 0) {
        endpointFailures -= 1;
        return json({ title: "Unavailable", status: 503, detail: "the store is restarting" }, 503);
      }
      return json(options.noProjectionRef ? { ...ENDPOINT, spec: { ...ENDPOINT.spec, projectionRef: undefined } } : ENDPOINT);
    }
    if (path.includes("/projections/")) {
      return url.searchParams.get("dryRun") === "All"
        ? json({ valid: true, verdict: { ok: true, findings: [] } })
        : json({ kind: "Change", metadata: { name: "chg-projection" }, spec: {}, status: { phase: "Pending" } });
    }
    if (path.endsWith("/projections")) return json(list([projectionOf(classes, filter)]));
    if (path.endsWith("/policies")) {
      if (policyFailures > 0) {
        policyFailures -= 1;
        return json({ title: "Unavailable", status: 503, detail: "policies are restarting" }, 503);
      }
      return json(list([]));
    }
    if (path.endsWith("/spaces")) {
      if (spaceFailures > 0) {
        spaceFailures -= 1;
        return json({ title: "Unavailable", status: 503, detail: "spaces are restarting" }, 503);
      }
      return json(list([SPACE]));
    }
    if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
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
  return seen;
}

const writes = (seen: Seen[], tail: string) =>
  seen.filter((call) => call.url.pathname.endsWith(tail) && call.method !== "GET" && call.url.searchParams.get("dryRun") !== "All");

beforeEach(async () => {
  await i18n.changeLanguage("en");
  window.history.pushState({}, "", `/projects/${PROJECT}/endpoints/${NAME}`);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("pure helpers of the endpoint page and the proof", () => {
  it("names the fault of a box: numbers, range and order", () => {
    expect(boxFault({ west: "a", south: "1", east: "2", north: "3" })).toBe("numbers");
    expect(boxFault({ west: "", south: "1", east: "2", north: "3" })).toBe("numbers");
    expect(boxFault({ west: "-190", south: "1", east: "2", north: "3" })).toBe("range");
    expect(boxFault({ west: "0", south: "-91", east: "2", north: "3" })).toBe("range");
    expect(boxFault({ west: "5", south: "1", east: "2", north: "3" })).toBe("order");
    expect(boxFault({ west: "1", south: "1", east: "2", north: "3" })).toBeUndefined();
  });

  it("joins the members of a list value in words", () => {
    expect(shownValue({ type: "Property", value: ["Mo", "Tu"] })).toBe("Mo, Tu");
    expect(shownValue({ type: "Property", value: null })).toBe("");
  });
});

describe("the endpoint page", () => {
  it("retries a page that failed to load and then shows the endpoint", async () => {
    const user = userEvent.setup();
    renderPage({ endpointFails: true });
    const retry = await screen.findByRole("button", { name: en.app.error.retry });
    await user.click(retry);
    expect(await screen.findByRole("heading", { name: "Events" })).toBeInTheDocument();
  });

  it("retries the policies of the access section after they failed", async () => {
    const user = userEvent.setup();
    const seen = renderPage({ policiesFail: true });
    await screen.findByRole("heading", { name: "Events" });
    const policiesRead = () => seen.filter((call) => call.url.pathname.endsWith("/policies")).length;
    await waitFor(() => expect(policiesRead()).toBeGreaterThan(0));
    const before = policiesRead();
    const retry = await screen.findByText(/policies are restarting/);
    await user.click(within(retry.closest("[role=alert]") as HTMLElement).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expect(policiesRead()).toBeGreaterThan(before));
  });

  it("proposes a hidden attribute shown again, as a change of the endpoint alone", async () => {
    const user = userEvent.setup();
    const seen = renderPage();
    const source = await screen.findByRole("checkbox", { name: "source" });
    expect(source).toBeChecked();
    await user.click(source);
    expect(source).not.toBeChecked();
    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));
    await waitFor(() => expect(writes(seen, `/endpoints/${NAME}`)).toHaveLength(1));
    const body = JSON.parse(writes(seen, `/endpoints/${NAME}`)[0].body) as { spec: { projection?: { hiddenAttributes?: string[] } } };
    expect(body.spec.projection?.hiddenAttributes ?? []).toEqual([]);
  });

  it("proposes a projection without a filter once the last condition is cleared", async () => {
    const user = userEvent.setup();
    const seen = renderPage({ filter: { q: 'name=="x"' } });
    await user.clear(await screen.findByLabelText(en.endpoints.filter.q));
    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));
    await waitFor(() => expect(writes(seen, "/projections/events-open")).toHaveLength(1));
    const body = JSON.parse(writes(seen, "/projections/events-open")[0].body) as { spec: Record<string, unknown> };
    expect(body.spec).not.toHaveProperty("filter");
  });

  it("refuses a condition when the projection names no attribute to compare", async () => {
    const user = userEvent.setup();
    renderPage({ classes: [{ name: "Event", slots: [] }] });
    await user.click(await screen.findByRole("button", { name: en.endpoints.condition.add }));
    expect(await screen.findByText(en.endpoints.condition.needsAttribute)).toBeInTheDocument();
  });

  it("sets a geometry attribute from the select of the area box", async () => {
    const user = userEvent.setup();
    renderPage();
    const property = await screen.findByLabelText(en.endpoints.area.property);
    const options = [...(property as HTMLSelectElement).options].map((option) => option.value);
    await user.selectOptions(property, options[0]);
    expect(property).toHaveValue(options[0]);
  });
});

describe("the endpoint page, data view and counts", () => {
  it("writes the data view's type into the address, so a link opens what was seen", async () => {
    renderPage();
    await waitFor(() => expect(window.location.search).toContain("dataType=Event"));
  });

  it("names the projection from the model when another projection already exists", async () => {
    const seen = renderPage({ noProjectionRef: true });
    await screen.findByRole("heading", { name: "Events" });
    expect(await screen.findByText(en.endpoints.filterEditor.createsProjection.replace("{name}", NAME))).toBeInTheDocument();
    expect(seen.some((call) => call.url.pathname.endsWith("/projections"))).toBe(true);
  });

  it("says the endpoint answered no results count", async () => {
    renderPage({ savedNoCount: true });
    expect(await screen.findByText(/the endpoint answered no results count/)).toBeInTheDocument();
  });

  it("retries the spaces of the access section after they failed", async () => {
    const user = userEvent.setup();
    const seen = renderPage({ spacesFail: true });
    await screen.findByRole("heading", { name: "Events" });
    const spacesRead = () => seen.filter((call) => call.url.pathname.endsWith("/spaces")).length;
    const alert = await screen.findByText(/spaces are restarting/);
    const before = spacesRead();
    await user.click(within(alert.closest("[role=alert]") as HTMLElement).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expect(spacesRead()).toBeGreaterThan(before));
  });
});

describe("the filter proof", () => {
  it("switches between the types the endpoint serves and pages through the rows", async () => {
    const user = userEvent.setup();
    const seen = renderPage({ classes: [EVENT, PLACE], total: 30 });
    const proof = await screen.findByRole("region", { name: en.endpoints.proof.title });
    const next = await within(proof).findByRole("button", { name: en.endpoints.proof.next });
    await waitFor(() => expect(next).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(next);
    await waitFor(() =>
      expect(seen.some((call) => call.url.pathname === `/cs/${SEGMENT}/ngsi-ld/v1/entities` && call.url.searchParams.get("offset") === "12")).toBe(true),
    );
    await user.click(within(proof).getByRole("button", { name: en.endpoints.proof.previous }));
    await waitFor(() => expect(within(proof).getByRole("button", { name: en.endpoints.proof.previous })).toHaveAttribute("aria-disabled", "true"));
    await user.selectOptions(within(proof).getByLabelText(en.endpoints.proof.type), "Place");
    await waitFor(() =>
      expect(seen.some((call) => call.url.pathname === `/cs/${SEGMENT}/ngsi-ld/v1/entities` && call.url.searchParams.get("type") === "Place")).toBe(true),
    );
  });

  it("says a failing saved read with the status text, and reads again on retry", async () => {
    const user = userEvent.setup();
    const seen = renderPage({ spaceReadable: false, saved: "text-500" });
    const proof = await screen.findByRole("region", { name: en.endpoints.proof.title });
    const retry = await within(proof).findByRole("button", { name: en.app.error.retry });
    const reads = () => seen.filter((call) => call.url.pathname === `/api/endpoint/${SLUG}/ngsi-ld/v1/entities`).length;
    const before = reads();
    await user.click(retry);
    await waitFor(() => expect(reads()).toBeGreaterThan(before));
    cleanup();
  });

  it("counts the rows it got when the space answers no results count", async () => {
    renderPage({ total: null });
    expect(await screen.findByText(/^Serving \d+ of 2 Event/)).toBeInTheDocument();
  });
});
