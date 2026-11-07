/**
 * The endpoint's filter editor and its side-by-side proof (T-2776, EP-85, EP-86).
 *
 * 1. The rows are aligned on the entity id: a row the filter drops is on the right only, marked
 *    "not served", and an attribute the space holds and the endpoint would not serve is struck
 *    through on the right.
 * 2. The summary line counts what is served of what the space holds, and the hidden attributes.
 * 3. An edit reaches the gateway's preview as a draft, after it rests, never as a saved change.
 * 4. Hidden attributes are part of the one editor and are proposed on the endpoint itself.
 * 5. A person the space refuses sees what the endpoint serves today, and is told why.
 * 6. A draft the gateway refuses says why, beside the proof.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { alignRows, hiddenOf, shownValue } from "../src/pages/endpoints/FilterProof";

const PROJECT = "helsinki";
const NAME = "helsinki-events";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
/** The Portal's preview of the Endpoint's draft (API/01 §40, T-3802). */
const PREVIEW = `/api/v1/projects/${PROJECT}/endpoints/${NAME}/preview`;
const SEGMENT = "helsinki";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "aino",
  name: "Aino Virtanen",
  email: "aino@hel.fi",
  roles: ["portal-editor"],
};

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

const PROJECTION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ModelProjection",
  metadata: { name: "events-open", namespace: PROJECT },
  spec: {
    contextSpaceRef: "helsinki",
    dataModelRef: { kind: "DataModel", name: "helsinki", version: "1" },
    classes: [{ name: "Event", slots: ["name", "startDate", "source", "eventStatus"] }],
  },
};

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "helsinki", namespace: PROJECT },
  spec: { urnSegment: SEGMENT },
};

const PERMITTED = {
  project: PROJECT,
  bootstrap: false,
  grants: [
    {
      role: "endpoint-editor",
      binding: "editors",
      rule: { kinds: ["Endpoint", "ModelProjection"], verbs: ["propose", "delete"] },
    },
  ],
};

const concert = {
  id: "urn:ngsi-ld:Event:hel.fi:helsinki:concert",
  type: "Event",
  name: { type: "Property", value: "Concert" },
  startDate: { type: "Property", value: "2026-10-01T18:00:00Z" },
  source: { type: "Property", value: "linkedevents" },
};
const lecture = {
  id: "urn:ngsi-ld:Event:hel.fi:helsinki:lecture",
  type: "Event",
  name: { type: "Property", value: "Lecture" },
  startDate: { type: "Property", value: "2025-01-01T10:00:00Z" },
  source: { type: "Property", value: "linkedevents" },
};
/** The concert as the endpoint serves it: `source` hidden. */
const concertServed = { id: concert.id, type: "Event", name: concert.name, startDate: concert.startDate };

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

interface Sent {
  path: string;
  body: Record<string, unknown>;
}

/** The space's model, inline, for an endpoint that names no projection yet. */
const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "helsinki", namespace: PROJECT },
  spec: {
    contextSpaceRef: "helsinki",
    version: "1.0.0",
    linkml: [
      "id: https://hel.fi/models/helsinki",
      "name: helsinki",
      "default_range: string",
      "classes:",
      "  Event:",
      "    attributes:",
      "      name: {}",
      "      startDate: {range: datetime}",
      "      source: {}",
      "      eventStatus: {range: EventStatus}",
      "enums:",
      "  EventStatus:",
      "    permissible_values:",
      "      scheduled: {}",
      "      cancelled: {}",
      "",
    ].join("\n"),
  },
};

/** The same endpoint before anybody gave it a projection: what helsinki-events is on dev. */
const UNPROJECTED = {
  ...ENDPOINT,
  spec: { ...ENDPOINT.spec, projectionRef: undefined, projection: undefined },
};

function renderPage({
  space = "readable",
  preview = "serves",
  endpoint = ENDPOINT,
}: {
  space?: "readable" | "refused";
  preview?: "serves" | "refuses";
  endpoint?: unknown;
} = {}) {
  const sent: Sent[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input instanceof Request ? input : new Request(String(input));
    const url = new URL(request.url, window.location.origin);
    const path = url.pathname;
    const text = request.method === "GET" ? "" : await request.clone().text();
    sent.push({ path: `${path}${url.search}`, body: text ? (JSON.parse(text) as Record<string, unknown>) : {} });
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json", ...headers },
      });

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json(PERMITTED);
    if (path === `/cs/${SEGMENT}/ngsi-ld/v1/entities`) {
      return space === "readable"
        ? json([concert, lecture], 200, { "NGSILD-Results-Count": "2" })
        : json({ title: "Not Found" }, 404);
    }
    if (path === PREVIEW) {
      if (preview === "refuses") {
        return json({ title: "Bad Request", detail: "projection.filter.q: an empty query string filters nothing" }, 400);
      }
      const asked = JSON.parse(text) as { id?: string[]; limit: number };
      // Only the upcoming concert passes the filter; asked for both ids, it answers one.
      const served = asked.id ? [concertServed].filter((one) => asked.id?.includes(one.id)) : [concertServed];
      return json({ count: 1, entities: served.slice(0, asked.limit) });
    }
    if (path === `/api/endpoint/${SLUG}/ngsi-ld/v1/entities`) {
      return json([concertServed], 200, { "NGSILD-Results-Count": "1" });
    }
    if (path.endsWith(`/endpoints/${NAME}`)) {
      return request.method === "GET"
        ? json(endpoint)
        : url.searchParams.get("dryRun") === "All"
          ? json({ valid: true, verdict: { ok: true, findings: [] } })
          : json({ kind: "Change", metadata: { name: "chg-endpoint" }, spec: {}, status: { phase: "Pending" } });
    }
    if (path.includes("/projections/")) {
      return url.searchParams.get("dryRun") === "All"
        ? json({ valid: true, verdict: { ok: true, findings: [] } })
        : json({ kind: "Change", metadata: { name: "chg-projection" }, spec: {}, status: { phase: "Pending" } });
    }
    if (path.endsWith("/projections")) return json(list(endpoint === ENDPOINT ? [PROJECTION] : []));
    if (path.endsWith("/datamodels")) return json(list([MODEL]));
    if (path.endsWith("/import")) {
      return url.searchParams.get("dryRun") === "All"
        ? json({ created: [], replaced: [], skipped: [], renamed: {}, native_files: 0, lane: "red", needs: [] })
        : json({ kind: "Change", metadata: { name: "chg-bundle" }, spec: {}, status: { phase: "Pending" } }, 202);
    }
    if (path.endsWith("/spaces")) return json(list([SPACE]));
    if (path.endsWith("/endpoints")) return json(list([endpoint]));
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
  return sent;
}

const previews = (sent: Sent[]) => sent.filter((call) => call.path === PREVIEW);

describe("the rows of the proof", () => {
  it("marks a row the filter drops and strikes what the endpoint would not serve", () => {
    const rows = alignRows([concert, lecture], [concertServed]);
    expect(rows.map((row) => row.id)).toEqual([concert.id, lecture.id]);
    expect(rows[0].served).toEqual(concertServed);
    expect(rows[0].struck).toEqual(["source"]);
    // A dropped row strikes nothing: all of it is not served, which the row says as a whole.
    expect(rows[1].served).toBeUndefined();
    expect(rows[1].struck).toEqual([]);
    expect(hiddenOf(rows)).toEqual(["source"]);
  });

  it("is the served page alone when the space cannot be read", () => {
    const rows = alignRows(undefined, [concertServed]);
    expect(rows).toEqual([{ id: concert.id, served: concertServed, struck: [] }]);
  });

  it("reads a value, a relationship and a language map the way a person does", () => {
    expect(shownValue({ type: "Property", value: 42 })).toBe("42");
    expect(shownValue({ type: "Relationship", object: "urn:ngsi-ld:Place:x" })).toBe("urn:ngsi-ld:Place:x");
    expect(shownValue({ type: "LanguageProperty", languageMap: { fi: "Konsertti", en: "Concert" } })).toBe("Konsertti");
    expect(shownValue({ type: "Property", value: "x".repeat(200) })).toHaveLength(80);
    expect(shownValue(undefined)).toBe("");
  });

  it("shows a structured value in words, never as JSON (T-4949)", () => {
    const address = { addressCountry: "FI", addressLocality: "Espoo", postalCode: "02100", streetAddress: "Tapiontori 3" };
    expect(shownValue({ type: "Property", value: address })).toBe("Tapiontori 3, 02100 Espoo");
    expect(shownValue({ type: "GeoProperty", value: { type: "Point", coordinates: [24.657864, 60.203636] } })).toBe("60.20364, 24.65786");
    expect(shownValue({ type: "GeoProperty", value: { type: "Polygon", coordinates: [[[0, 0], [1, 0], [0, 1], [0, 0]]] } })).toBe("Polygon");
    expect(shownValue({ type: "Property", value: { start: "08:00", end: null, days: ["Mo", "Tu"] } })).toBe("start: 08:00 · days: Mo, Tu");
    for (const value of [address, { type: "Point", coordinates: [1, 2] }, { a: { b: 1 } }]) {
      expect(shownValue({ type: "Property", value })).not.toMatch(/[{"]/);
    }
  });
});

describe("the endpoint's filter editor and its proof", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", `/projects/${PROJECT}/endpoints/${NAME}`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows what is served beside the original, aligned, with the summary", async () => {
    renderPage();

    expect(
      await screen.findByText("Serving 1 of 2 Event · hidden attributes: 1"),
    ).toBeInTheDocument();
    const rows = screen.getAllByTestId("proof-row");
    expect(rows).toHaveLength(2);
    // The lecture is on the right only, and says why.
    expect(within(rows[1]).getByText(en.endpoints.proof.notServedMark)).toBeInTheDocument();
    expect(within(rows[1]).getByText(new RegExp(en.endpoints.proof.notServed))).toBeInTheDocument();
    // The concert's source is struck through on the right, and named for a screen reader.
    const struck = within(rows[0]).getByText("source", { selector: "del" });
    expect(struck.closest("dt")).toHaveTextContent(`source (${en.endpoints.proof.hiddenMark})`);
    expect(screen.getByRole("columnheader", { name: en.endpoints.proof.served })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: en.endpoints.proof.original })).toBeInTheDocument();
  });

  it("asks the preview with the draft after an edit rests, and proposes nothing", async () => {
    const sent = renderPage();
    const user = userEvent.setup();

    const q = await screen.findByLabelText(en.endpoints.filter.q);
    await user.type(q, 'startDate>="2026-09-25"');
    await waitFor(
      () => {
        const last = previews(sent).at(-1);
        expect((last?.body.projection as { filter?: { q?: string } } | undefined)?.filter?.q).toBe(
          'startDate>="2026-09-25"',
        );
      },
    );
    const last = previews(sent).at(-1);
    expect(last?.body.type).toBe("Event");
    expect(last?.body.hiddenAttributes).toEqual(["source"]);
    // One key at a time never reached the gateway: only the rested draft did.
    expect(previews(sent).filter((call) => (call.body.projection as { filter?: unknown })?.filter).length).toBeLessThan(
      'startDate>="2026-09-25"'.length,
    );
    expect(sent.some((call) => call.path.includes("/projections/") || call.path.endsWith(`/endpoints/${NAME}`) && Object.keys(call.body).length > 0)).toBe(false);
  });

  it("holds the hidden attributes in the same editor and proposes them on the endpoint", async () => {
    const sent = renderPage();
    const user = userEvent.setup();

    const hideName = await screen.findByRole("checkbox", { name: "name" });
    expect(screen.getByRole("checkbox", { name: "source" })).toBeChecked();
    await user.click(hideName);
    await waitFor(
      () => expect(previews(sent).at(-1)?.body.hiddenAttributes).toEqual(["source", "name"]),
    );

    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));
    // The proposal is the write with a body; the reads of the same address carry none.
    const writes = () =>
      sent.filter((call) => call.path === `/api/v1/projects/${PROJECT}/endpoints/${NAME}` && "spec" in call.body);
    await waitFor(() => expect(writes()).toHaveLength(1));
    const proposed = writes()[0];
    expect((proposed?.body.spec as { projection?: unknown }).projection).toEqual({ hiddenAttributes: ["source", "name"] });
    // The projection did not change, so it is not proposed.
    expect(sent.some((call) => call.path.startsWith(`/api/v1/projects/${PROJECT}/projections/`))).toBe(false);
  });

  it("refuses a draft that serves no type, at the types", async () => {
    renderPage();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("checkbox", { name: "Event" }));
    expect(screen.getByText(en.endpoints.filterEditor.noType)).toBeInTheDocument();
    // Refused with its reason (ui-writing rule 7): aria-disabled keeps it reachable to say why.
    const propose = screen.getByRole("button", { name: en.endpoints.page.filterPropose });
    expect(propose).toHaveAttribute("aria-disabled", "true");
    expect(propose).toHaveAccessibleDescription(en.endpoints.page.filterFaulty);
  });

  it("shows a person the space refuses what the endpoint serves today, and says why", async () => {
    const sent = renderPage({ space: "refused" });

    expect(await screen.findByText("Serving 1 Event")).toBeInTheDocument();
    expect(screen.getByText(en.endpoints.proof.originalUnreadable)).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: en.endpoints.proof.original })).not.toBeInTheDocument();
    // No draft travels for a person the preview would refuse anyway.
    expect(previews(sent)).toHaveLength(0);
  });

  it("says why the gateway refused the draft", async () => {
    renderPage({ preview: "refuses" });

    expect(
      await screen.findByText(
        "The draft filter is refused: projection.filter.q: an empty query string filters nothing",
      ),
    ).toBeInTheDocument();
  });

  it("shows the same proof in the endpoint's form, for the classes and hidden attributes it holds", async () => {
    window.history.pushState({}, "", `/projects/${PROJECT}/endpoints/${NAME}/edit`);
    const sent = renderPage();
    const user = userEvent.setup();

    await user.click(await screen.findByText(en.endpoints.proof.title, { selector: "summary" }));
    expect(
      await screen.findByText("Serving 1 of 2 Event · hidden attributes: 1"),
    ).toBeInTheDocument();
    const last = previews(sent).at(-1);
    expect(last?.body.hiddenAttributes).toEqual(["source"]);
    expect((last?.body.projection as { classes?: { name: string }[] }).classes?.map((klass) => klass.name)).toEqual([
      "Event",
    ]);
  });

  it("gives an endpoint with no projection its first filter from the space's model, as one proposal", async () => {
    const sent = renderPage({ endpoint: UNPROJECTED });
    const user = userEvent.setup();

    expect(
      await screen.findByText(en.endpoints.filterEditor.createsProjection.replace("{name}", NAME)),
    ).toBeInTheDocument();
    // Every class and slot of the model, nothing hidden: what the endpoint serves today.
    expect(screen.getByRole("checkbox", { name: "Event" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "source" })).not.toBeChecked();

    await user.type(screen.getByLabelText(en.endpoints.filter.q), 'startDate>="2026-09-25"');
    await user.click(screen.getByRole("checkbox", { name: "source" }));
    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));

    await waitFor(() => expect(sent.filter((call) => call.path.startsWith(`/api/v1/projects/${PROJECT}/import`))).toHaveLength(2));
    const [dryRun, real] = sent.filter((call) => call.path.startsWith(`/api/v1/projects/${PROJECT}/import`));
    expect(dryRun.path).toContain("dryRun=All");
    const [drawn, named] = real.body.manifests as Array<{ kind: string; metadata: { name: string }; spec: Record<string, unknown> }>;
    expect(drawn.kind).toBe("ModelProjection");
    expect(drawn.metadata.name).toBe(NAME);
    expect(drawn.spec.dataModelRef).toEqual({ kind: "DataModel", name: "helsinki", version: "1" });
    expect(drawn.spec.classes).toEqual([{ name: "Event", slots: ["name", "startDate", "source", "eventStatus"] }]);
    expect(drawn.spec.filter).toEqual({ q: 'startDate>="2026-09-25"' });
    expect(named.kind).toBe("Endpoint");
    expect(named.spec.projectionRef).toEqual({ kind: "ModelProjection", name: NAME });
    expect(named.spec.projection).toEqual({ hiddenAttributes: ["source"] });
    expect(await screen.findByText(/chg-bundle/)).toBeInTheDocument();
  });

  it("offers an enum attribute's permitted values as a list, not a text box (T-2706)", async () => {
    renderPage();
    const user = userEvent.setup();

    await user.selectOptions(await screen.findByLabelText(en.endpoints.condition.attribute), "eventStatus");
    const value = await screen.findByRole("combobox", { name: en.endpoints.condition.value });
    expect(Array.from((value as HTMLSelectElement).options, (option) => option.value)).toEqual([
      "",
      "scheduled",
      "cancelled",
    ]);
    await user.selectOptions(value, "cancelled");
    await user.selectOptions(screen.getByLabelText(en.endpoints.condition.operator), "notEquals");
    await user.click(screen.getByRole("button", { name: en.endpoints.condition.add }));
    expect(screen.getByLabelText(en.endpoints.filter.q)).toHaveValue('eventStatus!="cancelled"');
  });
});
