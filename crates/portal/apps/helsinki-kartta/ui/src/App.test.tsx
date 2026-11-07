/**
 * The Helsinki service map over what the public endpoint of `helsinki` answers (T-2788).
 *
 * `fetch` is what is stubbed, so every case goes through the SDK's own endpoint source. MapLibre
 * needs WebGL, which jsdom does not have, so the library is a double recording what it was given.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";

const setData = vi.fn();
const addSource = vi.fn();
/** What a click on a place of the map hands the App, as MapLibre would. */
let clickPlace: ((event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) | null = null;

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, layerOrHandler: unknown, handler?: (event: never) => void) {
      if (event === "load") (layerOrHandler as () => void)();
      if (event === "click" && handler) clickPlace = handler as typeof clickPlace;
    }
    addSource = addSource;
    addLayer = vi.fn();
    getSource = () => ({ setData });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const App = (await import("./App")).default;
const { SERVICES, WATER, answer } = await import("./fixtures/helsinki");
const { LOCALES } = await import("./locales");
const s = LOCALES.fi;

const SLUG = "hx5kv2nr7qcm3tbz6wjd4yae2p";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function show(refuse?: string, withEndpoint = true) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const type = new URL(path, "http://portal.test").searchParams.get("type");
      if (type === refuse) {
        return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "the policy does not grant this type" }), {
          status: 403,
          headers: { "content-type": "application/problem+json" },
        });
      }
      return json(answer(type));
    }),
  );
  // The entity panel reads the entity it shows through the App's client, as key-values.
  const client = stubClient({ entities: [...SERVICES, ...WATER].map(keyValues) }, {
    slug: withEndpoint ? SLUG : "",
    orgDomain: "hel.fi",
    space: withEndpoint ? "helsinki" : "elsewhere",
    transport: "origin",
    appName: "helsinki-kartta",
    language: "fi",
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

/** A normalized entity as the endpoint answers it with `options=keyValues`. */
function keyValues(entity: Record<string, unknown>): Row {
  const out: Record<string, unknown> = {};
  for (const [name, attr] of Object.entries(entity)) {
    const a = attr as { type?: string; value?: unknown; object?: unknown; languageMap?: Record<string, string> };
    out[name] = typeof attr !== "object" || attr === null ? attr : a.type === "Relationship" ? a.object : a.type === "LanguageProperty" ? (a.languageMap?.fi ?? null) : a.value;
  }
  return out as Row;
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  // The handler belongs to the map of the test that drew it; the next test waits for its own.
  clickPlace = null;
});

const results = () => screen.getByRole("region", { name: /paikka/ });
const names = () => within(results()).queryAllByRole("button").map((button) => button.querySelector(".name")?.textContent);

describe("the Helsinki service map", () => {
  it("lists the services and the water sensors, and draws those with a position", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Keskustakirjasto Oodi"));
    expect(names()).toEqual([
      "Hietaniemen uimaranta",
      "Hietaniemi, vesi",
      "Irrallinen anturi",
      "Kallion ala-asteen koulu",
      "Kallion terveysasema",
      "Keskustakirjasto Oodi",
      "Yrjönkadun uimahalli",
    ]);
    // A service of a category the map does not show is not shown under a wrong one.
    expect(names()).not.toContain("Muu palvelu");
    await waitFor(() => {
      const data = setData.mock.calls.at(-1)?.[0] as { features: unknown[] } | undefined;
      expect(data?.features).toHaveLength(6);
    });
  });

  it("counts each layer and narrows by layer and by search, without diacritics", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Keskustakirjasto Oodi"));
    expect(screen.getByRole("checkbox", { name: s.kind.water })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: s.kind.water })).toHaveAccessibleDescription("(2)");
    await userEvent.click(screen.getByRole("checkbox", { name: s.kind.water }));
    expect(names()).not.toContain("Hietaniemi, vesi");
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "yrjonkadun");
    expect(names()).toEqual(["Yrjönkadun uimahalli"]);
    await userEvent.clear(screen.getByRole("searchbox", { name: s.search }));
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "ei mitaan");
    expect(within(results()).getByText(s.noResults)).toBeInTheDocument();
  });

  // SDK-40: a place opens in the shell's entity panel, from the list or from the map; a public App
  // writes nothing, so the panel offers no Edit, and closing it gives the focus back.
  it("opens a water sensor from the list in the entity panel with its temperature, and closing returns", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Hietaniemi, vesi"));
    const item = within(results()).getByRole("button", { name: "Hietaniemi, vesi" });
    expect(item).toHaveAccessibleDescription(expect.stringContaining(s.kind.water));
    await userEvent.click(item);
    expect(item).toHaveAttribute("aria-pressed", "true");
    const panel = await screen.findByRole("dialog", { name: "Hietaniemi, vesi" });
    expect(await within(panel).findByText(/^16[.,]4/)).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "Muokkaa" })).toBeNull();
    await userEvent.click(within(panel).getByRole("button", { name: "Sulje" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(item).toHaveFocus());
  });

  it("opens a place clicked on the map, and ignores a click on nothing it knows", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Keskustakirjasto Oodi"));
    // The map binds its click in an effect that may run after the list is on screen (T-4257).
    await waitFor(() => expect(clickPlace).not.toBeNull());
    act(() => clickPlace?.({ features: [{ properties: { id: "urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:nowhere" } }] }));
    act(() => clickPlace?.({ features: [] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => clickPlace?.({ features: [{ properties: { id: SERVICES[0].id } }] }));
    expect(await screen.findByRole("dialog", { name: "Keskustakirjasto Oodi" })).toBeInTheDocument();
    expect(within(results()).getByRole("button", { name: "Keskustakirjasto Oodi" })).toHaveAttribute("aria-pressed", "true");
  });

  it("opens every place of the list in the panel, and every layer turns off and on", async () => {
    show();
    // Five services and two sensors; the service of a category the map does not show is not one.
    await waitFor(() => expect(names()).toHaveLength(7));
    const user = userEvent.setup();
    for (const name of names()) {
      await user.click(within(results()).getByRole("button", { name: name! }));
      const panel = await screen.findByRole("dialog", { name: name! });
      await user.click(within(panel).getByRole("button", { name: "Sulje" }));
    }
    for (const kind of Object.values(s.kind)) {
      await user.click(screen.getByRole("checkbox", { name: kind }));
      await user.click(screen.getByRole("checkbox", { name: kind }));
    }
    expect(names()).toHaveLength(7);
  });

  it("toggles the layers while they still load", async () => {
    let answerNow: () => void = () => undefined;
    const held = new Promise<void>((resolve) => (answerNow = resolve));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        await held;
        return json(answer(new URL(path, "http://portal.test").searchParams.get("type")));
      }),
    );
    render(
      <JcProvider client={stubClient(undefined, { slug: SLUG, orgDomain: "hel.fi", space: "helsinki", transport: "origin", appName: "helsinki-kartta", language: "fi" })}>
        <App />
      </JcProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent(s.loading);
    const user = userEvent.setup();
    for (const kind of Object.values(s.kind)) await user.click(screen.getByRole("checkbox", { name: kind }));
    answerNow();
    await waitFor(() => expect(screen.queryByText(s.loading)).toBeNull());
    expect(names()).toEqual([]);
  });

  it("says a failure that is no endpoint answer in its own words", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      }),
    );
    render(
      <JcProvider client={stubClient(undefined, { slug: SLUG, orgDomain: "hel.fi", space: "helsinki", transport: "origin", appName: "helsinki-kartta", language: "fi" })}>
        <App />
      </JcProvider>,
    );
    const alerts = await screen.findAllByRole("alert");
    expect(alerts.map((a) => a.textContent).join(" ")).toMatch(/Failed to fetch/);
  });

  it("says why a type the endpoint refuses is missing and keeps the other", async () => {
    show("WaterQualityObserved");
    expect(await screen.findByRole("alert")).toHaveTextContent(s.refused(s.type.WaterQualityObserved, "the policy does not grant this type"));
    await waitFor(() => expect(names()).toContain("Keskustakirjasto Oodi"));
  });

  it("says it has nothing to read when the app has no endpoint of the city's space", () => {
    show(undefined, false);
    expect(screen.getByRole("status")).toHaveTextContent(s.noEndpoint);
  });

  // WCAG 2.5.3 label in name: a voice user says what a place's button shows, so its name is its visible text.
  it("names each place's button by the words it shows", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Keskustakirjasto Oodi"));
    for (const button of within(results()).getAllByRole("button")) {
      const shown = button.cloneNode(true) as HTMLElement;
      shown.querySelectorAll("[aria-hidden='true']").forEach((hidden) => hidden.remove());
      expect(within(results()).getByRole("button", { name: shown.textContent!.trim() })).toBe(button);
    }
  });

  it("names its sources and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(names()).toContain("Keskustakirjasto Oodi"));
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    // label-content-name-mismatch (WCAG 2.5.3) is opt-in before axe 4.14: every visible word of a place's button is in its name.
    const found = await axe.run(container, { rules: { region: { enabled: false }, "label-content-name-mismatch": { enabled: true } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
