/**
 * T-3110, ADR-N-042 §3.6: the API of one type of a space: the calls each Endpoint answers for it,
 * one tried with the person's session, the ServiceAccount way to a program's token, and the
 * webhooks (NGSI-LD subscriptions) that watch it, a new one started as a draft.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/TypeApi.tsx.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { TypeApi, callsOf, viewQueryUrls, webhooksOf } from "../src/pages/spaces/TypeApi";
import { expectNoViolations } from "./checks";
import { renderPage } from "./page_contract";

const V = "joinedcontext.com/v1alpha1";
const endpoint = (name: string, slug: string | undefined, spec: Record<string, unknown> = {}): Manifest =>
  ({ apiVersion: V, kind: "Endpoint", metadata: { name, namespace: "helsinki" }, spec: { contextSpaceRef: "bikes", ...(slug ? { slug } : {}), ...spec } }) as Manifest;
const subscription = (name: string, space: string, type: string, uri: string): Manifest =>
  ({
    apiVersion: V,
    kind: "Subscription",
    metadata: { name, namespace: "helsinki", labels: { "joinedcontext.com/space": space } },
    spec: { entities: [{ type }], notification: { endpoint: { uri } } },
  }) as Manifest;

describe("the calls of a type", () => {
  it("names the list, count, one entity and history of the type, and every representation served", () => {
    const calls = callsOf("abc", "Bike Station", ["ngsi-ld", "geojson", "csv"], undefined);
    expect(calls.map((call) => call.key)).toEqual(["list", "count", "one", "history", "csv"]);
    expect(calls[0].url).toBe(`${window.location.origin}/api/endpoint/abc/ngsi-ld/v1/entities?type=Bike%20Station&limit=10`);
  });

  it("takes the webhooks of this space that watch this type and no other", () => {
    const hooks = webhooksOf(
      [
        subscription("a", "bikes", "BikeHireDockingStation", "https://a.example"),
        subscription("b", "other", "BikeHireDockingStation", "https://b.example"),
        subscription("c", "bikes", "Road", "https://c.example"),
      ],
      "bikes",
      "BikeHireDockingStation",
    );
    expect(hooks.map((hook) => hook.metadata.name)).toEqual(["a"]);
  });
});

describe("the query of the view on screen (T-4884)", () => {
  const at = `${window.location.origin}/api/endpoint/abc`;
  it("gives the same q to NGSI-LD, and to OGC Features and OData as ngsild.q, for each one served", () => {
    const urls = viewQueryUrls("abc", "Event", 'category=="music";startDate>=2026-10-01', ["ngsi-ld", "ogc-features", "odata"]);
    const q = encodeURIComponent('category=="music";startDate>=2026-10-01');
    expect(urls).toEqual([
      { key: "ngsi-ld", url: `${at}/ngsi-ld/v1/entities?type=Event&q=${q}` },
      { key: "ogc-features", url: `${at}/ogc/features/collections/Event/items?ngsild.q=${q}` },
      { key: "odata", url: `${at}/odata/v4/Event?ngsild.q=${q}` },
    ]);
  });

  it("asks the whole type without a filter, and only the representations the Endpoint serves", () => {
    expect(viewQueryUrls("abc", "Event", "  ", ["ngsi-ld", "csv"])).toEqual([
      { key: "ngsi-ld", url: `${at}/ngsi-ld/v1/entities?type=Event` },
    ]);
  });

  it("keeps a q that tries to add a parameter inside its own value", () => {
    const [ngsi] = viewQueryUrls("abc", "Event", "name==\"a\"&limit=1000#x", ["ngsi-ld"]);
    expect(new URL(ngsi.url).searchParams.get("q")).toBe('name=="a"&limit=1000#x');
    expect(new URL(ngsi.url).searchParams.get("limit")).toBeNull();
  });
});

describe("the API tab", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function show(endpoints = [endpoint("public-bikes", "abc"), endpoint("draft-ep", undefined)], q?: string) {
    const puts: { path: string; body: unknown }[] = [];
    renderPage(<TypeApi project="helsinki" space="bikes" type="BikeHireDockingStation" endpoints={endpoints} q={q} />, {
      path: "/projects/helsinki/spaces/bikes",
      answer: async (url, request) => {
        if (url.pathname.startsWith("/api/endpoint/abc/ngsi-ld/v1/entities")) {
          return new Response(JSON.stringify([{ id: "urn:ngsi-ld:BikeHireDockingStation:1", type: "BikeHireDockingStation" }]), {
            status: 200,
            headers: { "Content-Type": "application/ld+json" },
          });
        }
        if (url.pathname.endsWith("/subscriptions") && request.method === "GET") {
          return new Response(
            JSON.stringify({ items: [subscription("bikes-low", "bikes", "BikeHireDockingStation", "https://hooks.example/low")] }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          );
        }
        if (request.method === "PUT" && url.pathname.includes("/drafts/")) {
          puts.push({ path: url.pathname, body: await request.json() });
          return new Response(JSON.stringify({ kind: "Subscription", name: "x", version: 1, manifest: {} }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        return undefined;
      },
    });
    return { puts };
  }

  it("shows the URL of the view's own query with a copy button per representation", async () => {
    show([endpoint("public-bikes", "abc", { enabledRepresentations: ["ngsi-ld", "odata"] })], "availableBikeNumber<3");
    const query = await screen.findByTestId("api-view-query");
    expect(within(query).getByText(/availableBikeNumber<3/)).toBeInTheDocument();
    expect(within(query).getByText(/entities\?type=BikeHireDockingStation&q=availableBikeNumber%3C3$/)).toBeInTheDocument();
    expect(within(query).getByText(/odata\/v4\/BikeHireDockingStation\?ngsild\.q=availableBikeNumber%3C3$/)).toBeInTheDocument();
    expect(within(query).getByRole("button", { name: "Copy the OData URL" })).toBeInTheDocument();
  });

  it.each([
    ["sk", 2, "2 Endpointy tohto"],
    ["sk", 5, "5 Endpointov tohto"],
    ["cs", 3, "3 Endpointy tohoto"],
    ["cs", 7, "7 Endpointů tohoto"],
    ["en", 2, "2 Endpoints of"],
  ])("counts the Endpoints in %s: %i", async (language, count, words) => {
    await i18n.changeLanguage(language);
    const hint = i18n.t("spaces.api.pickHint", { count });
    await i18n.changeLanguage("en");
    expect(hint).toContain(words);
  });

  it("gives every call its own copy button and copies that call's address", async () => {
    show();
    const calls = await screen.findByTestId("api-calls");
    for (const call of ["List", "Count", "One entity", "History"]) {
      expect(within(calls).getByRole("button", { name: `Copy the ${call} URL` })).toBeInTheDocument();
    }
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    await userEvent.click(within(calls).getByRole("button", { name: "Copy the Count URL" }));
    expect(writeText).toHaveBeenCalledWith(expect.stringMatching(/entities\?type=BikeHireDockingStation&count=true&limit=0$/));
  });

  it("shows each Endpoint's calls for the type, tries one with the session, and lists its webhooks", async () => {
    show();
    const api = await screen.findByTestId("view-api");
    // An Endpoint with no slug yet answers nothing, so it is not offered.
    expect(within(api).getAllByRole("heading", { level: 2 }).map((h) => h.textContent)).toEqual([
      "Through public-bikes",
      en.spaces.api.tokens,
      en.spaces.api.webhooks,
    ]);
    expect(within(api).getByText(/entities\?type=BikeHireDockingStation&limit=10$/)).toBeInTheDocument();
    expect(await within(api).findByText("bikes-low")).toBeInTheDocument();
    expect(within(api).getByRole("link", { name: en.spaces.api.tokensLink })).toHaveAttribute(
      "href",
      "/projects/helsinki/settings/service-accounts",
    );
    await expectNoViolations(api);

    await userEvent.click(within(api).getByRole("button", { name: en.spaces.api.try }));
    expect(await within(api).findByRole("status")).toHaveTextContent("The endpoint answered 200");
    expect(within(api).getByTestId("api-tried")).toHaveTextContent("urn:ngsi-ld:BikeHireDockingStation:1");
  });

  it("starts a webhook on the type as a Subscription draft and opens it", async () => {
    const { puts } = show();
    await userEvent.click(await screen.findByRole("button", { name: en.spaces.api.newWebhook }));
    await waitFor(() => expect(puts).toHaveLength(1));
    expect(puts[0].path).toMatch(/^\/api\/v1\/projects\/helsinki\/drafts\/Subscription\/webhook-bikehiredockingstation-[a-z0-9]{1,4}$/);
    const manifest = (puts[0].body as { manifest: { spec: unknown; metadata: { labels: unknown } } }).manifest;
    expect(manifest.spec).toEqual({ entities: [{ type: "BikeHireDockingStation" }], notification: { endpoint: { uri: "", accept: "application/json" } } });
    expect(manifest.metadata.labels).toEqual({ "joinedcontext.com/space": "bikes" });
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/subscriptions"));
    expect(window.location.search).toMatch(/^\?draft=webhook-bikehiredockingstation-/);
  });

  it("shows one Endpoint at a time and the picker changes which", async () => {
    show([endpoint("public-bikes", "abc"), endpoint("second", "def")]);
    const picker = await screen.findByRole("combobox", { name: /Endpoint/ });
    expect(screen.getByRole("heading", { name: "Through public-bikes" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Through second" })).toBeNull();
    await userEvent.selectOptions(picker, "second");
    expect(screen.getByRole("heading", { name: "Through second" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Through public-bikes" })).toBeNull();
  });

  it("says no program can read a space no Endpoint serves", async () => {
    show([]);
    expect(await screen.findByText(en.spaces.api.noEndpoint)).toBeInTheDocument();
  });
});
