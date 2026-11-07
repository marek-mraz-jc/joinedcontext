/**
 * The city map over what the public endpoint of `banskabystrica-verejne` answers (T-2782).
 *
 * `fetch` is what is stubbed, so every case goes through the SDK's own endpoint source. MapLibre
 * needs WebGL, which jsdom does not have, so the library is a double recording what it was given.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";

const setData = vi.fn();
const addSource = vi.fn();
/** The map's click on its places, as the App registered it. */
const clicks: Array<(event: unknown) => void> = [];

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, layerOrHandler: string | (() => void), handler?: (event: unknown) => void) {
      if (event === "load" && typeof layerOrHandler === "function") layerOrHandler();
      if (event === "click" && handler) clicks.push(handler);
    }
    addSource = addSource;
    addLayer = vi.fn();
    getSource = () => ({ setData });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const { default: App, MOST } = await import("./App");
const { AIR, answer, EVENTS, SCHOOLS } = await import("./fixtures/verejne");
const { LOCALES } = await import("./locales");
const s = LOCALES.sk;

const SLUG = "kz3c7bq2mxa6wdr4tnyh5pje7f";
const PORTAL = "https://portal.dev.joinedcontext.com/projects/banskabystrica";

/** The places as the panel reads one through the client: keyValues, a value where there is one. */
const FLAT = [...EVENTS, ...SCHOOLS, ...AIR].map((entity) => {
  const row: Record<string, unknown> = { id: entity.id, type: entity.type };
  for (const [key, cell] of Object.entries(entity)) {
    if (key === "id" || key === "type" || key === "location") continue;
    const shaped = cell as { value?: unknown; languageMap?: Record<string, string> };
    row[key] = shaped.languageMap ? shaped.languageMap.sk : shaped.value;
  }
  return row as { id: string; type: string };
});
const TODAY = "2026-10-06";

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
  const client = stubClient({ entities: FLAT }, {
    portal: PORTAL,
    slug: withEndpoint ? SLUG : "",
    orgDomain: "banskabystrica.sk",
    space: withEndpoint ? "banskabystrica-verejne" : "elsewhere",
    transport: "origin",
    appName: "banskabystrica-mapa",
    language: "sk",
  });
  return render(
    <JcProvider client={client}>
      <App today={TODAY} />
    </JcProvider>,
  );
}

/** The App over any `fetch` answer and configuration, for the cases the fixture does not hold. */
function showWith(answerOf: (type: string | null, offset: number) => Promise<Response>, config: Record<string, unknown> = {}) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const url = new URL(path, "http://portal.test");
      return answerOf(url.searchParams.get("type"), Number(url.searchParams.get("offset") ?? 0));
    }),
  );
  const client = stubClient({ entities: FLAT }, {
    slug: SLUG,
    orgDomain: "banskabystrica.sk",
    space: "banskabystrica-verejne",
    transport: "origin",
    appName: "banskabystrica-mapa",
    language: "sk",
    ...config,
  });
  return render(
    <JcProvider client={client}>
      <App today={TODAY} />
    </JcProvider>,
  );
}

afterEach(() => {
  clicks.length = 0;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const results = () => screen.getByRole("region", { name: /miest/ });
const names = () => within(results()).queryAllByRole("button").map((button) => button.querySelector(".name")?.textContent);

describe("the city map", () => {
  it("lists the upcoming events, the schools and the air station, and draws those with a position", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Radvanský jarmok"));
    expect(names()).toEqual([
      "Radvanský jarmok",
      "Výstava fotografií",
      "Banská Bystrica, Štefánikovo nábrežie",
      "Gymnázium Jozefa Gregora Tajovského",
      "Základná škola, Moyzesova 18",
    ]);
    // The past concert is not shown until the person asks for past events too.
    expect(names()).not.toContain("Koncert v parku");
    await userEvent.click(screen.getByRole("checkbox", { name: s.upcomingOnly }));
    expect(names()).toContain("Koncert v parku");
    // The exhibition has no position: listed and said so, not drawn.
    expect(within(results()).getByText(new RegExp(s.notOnMap))).toBeInTheDocument();
    await waitFor(() => {
      const data = setData.mock.calls.at(-1)?.[0] as { features: unknown[] } | undefined;
      expect(data?.features).toHaveLength(5);
    });
  });

  it("narrows by layer and by search, without diacritics", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Radvanský jarmok"));
    await userEvent.click(screen.getByRole("checkbox", { name: new RegExp(`^${s.kind.event}`) }));
    expect(names()).not.toContain("Radvanský jarmok");
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "zakladna skola");
    expect(names()).toEqual(["Základná škola, Moyzesova 18"]);
    await userEvent.clear(screen.getByRole("searchbox", { name: s.search }));
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "nic take");
    expect(within(results()).getByText(s.noResults)).toBeInTheDocument();
  });

  it("opens a place from the list in the SDK's panel, linked to the Portal, and Escape gives the focus back", async () => {
    show();
    const user = userEvent.setup();
    await waitFor(() => expect(names()).toContain("Gymnázium Jozefa Gregora Tajovského"));
    const item = within(results()).getByRole("button", { name: /Gymnázium/ });
    await user.click(item);
    const panel = await screen.findByRole("dialog", { name: "Gymnázium Jozefa Gregora Tajovského" });
    expect(await within(panel).findByText("Tajovského 25, Banská Bystrica")).toBeInTheDocument();
    expect(item).toHaveAttribute("aria-pressed", "true");
    const link = within(panel).getByRole("link", { name: "Otvoriť v Portáli" });
    expect(link).toHaveAttribute("href", expect.stringContaining(`${PORTAL}/explore?space=banskabystrica-verejne&entityId=`));
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    // Following it leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    link.addEventListener("click", (event) => event.preventDefault());
    await user.click(link);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(item).toHaveFocus());
    expect(item).toHaveAttribute("aria-pressed", "false");
  });

  it("names each place's button so a screen reader hears the name and the kind apart", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Gymnázium Jozefa Gregora Tajovského"));
    expect(within(results()).getByRole("button", { name: `Gymnázium Jozefa Gregora Tajovského ${s.kind.school}` })).toBeInTheDocument();
  });

  it("opens every listed place in the panel, and each layer hides and shows its places", async () => {
    show();
    const user = userEvent.setup();
    await waitFor(() => expect(names()).toContain("Gymnázium Jozefa Gregora Tajovského"));
    await user.click(screen.getByRole("checkbox", { name: s.upcomingOnly }));
    for (const button of within(results()).getAllByRole("button")) {
      await user.click(button);
      const name = button.querySelector(".name")?.textContent ?? "";
      expect(await screen.findByRole("dialog", { name })).toBeInTheDocument();
      await user.keyboard("{Escape}");
    }
    for (const kind of ["event", "school", "air"] as const) {
      const layer = screen.getByRole("checkbox", { name: new RegExp(`^${s.kind[kind]} \\(`) });
      const before = names().length;
      await user.click(layer);
      expect(names().length).toBeLessThan(before);
      await user.click(layer);
      expect(names()).toHaveLength(before);
    }
  });

  it("opens a place picked on the map, and a click beside every place picks nothing", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Gymnázium Jozefa Gregora Tajovského"));
    // The map binds its click in an effect that may run after the list is on screen (T-4257).
    await waitFor(() => expect(clicks.length).toBeGreaterThan(0));
    const school = SCHOOLS.find((one) => (one.name as { languageMap: { sk: string } }).languageMap.sk.startsWith("Gymnázium"))!;
    act(() => clicks.at(-1)?.({ features: [{ properties: { id: school.id } }] }));
    expect(await screen.findByRole("dialog", { name: "Gymnázium Jozefa Gregora Tajovského" })).toBeInTheDocument();
    act(() => clicks.at(-1)?.({ features: [] }));
    act(() => clicks.at(-1)?.({ features: [{ properties: { id: "urn:ngsi-ld:Organization:gone" } }] }));
    expect(screen.getByRole("dialog", { name: "Gymnázium Jozefa Gregora Tajovského" })).toBeInTheDocument();
  });

  it("never links a script: an event whose url is javascript: is text in the panel, not a link", async () => {
    show();
    const user = userEvent.setup();
    await waitFor(() => expect(names()).toContain("Radvanský jarmok"));
    await user.click(screen.getByRole("checkbox", { name: s.upcomingOnly }));
    await user.click(within(results()).getByRole("button", { name: /Koncert/ }));
    const panel = await screen.findByRole("dialog", { name: "Koncert v parku" });
    expect(await within(panel).findByText("javascript:alert(1)")).toBeInTheDocument();
    expect(within(panel).queryByRole("link", { name: /javascript/ })).toBeNull();
    expect(within(panel).getAllByRole("link").every((link) => !link.getAttribute("href")?.startsWith("javascript:"))).toBe(true);
    await user.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("says why a kind the endpoint refuses is missing and keeps the others", async () => {
    show("Organization");
    expect(await screen.findByRole("alert")).toHaveTextContent(s.refused(s.kind.school, "the policy does not grant this type"));
    await waitFor(() => expect(names()).toContain("Radvanský jarmok"));
    expect(names()).not.toContain("Základná škola, Moyzesova 18");
  });

  it("toggles a layer while it still loads", async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    showWith(async (type) => {
      await held;
      return json(answer(type));
    });
    const user = userEvent.setup();
    expect(screen.getByRole("status")).toHaveTextContent(s.loading);
    for (const kind of ["event", "school", "air"] as const) {
      const layer = screen.getByRole("checkbox", { name: s.kind[kind] });
      await user.click(layer);
      expect(layer).not.toBeChecked();
      await user.click(layer);
    }
    release();
    await waitFor(() => expect(names()).toContain("Gymnázium Jozefa Gregora Tajovského"));
  });

  it("says a kind is cut at its limit when the endpoint keeps sending full pages", async () => {
    const full = (type: string | null, offset: number) =>
      Array.from({ length: 200 }, (_, index) => ({ id: `urn:ngsi-ld:${type}:banskabystrica.sk:banskabystrica-verejne:${offset + index}`, type }));
    showWith(async (type, offset) => json(type === "AirQualityObserved" ? full(type, offset) : answer(type)));
    expect(await screen.findByText(s.truncated(s.kind.air, MOST))).toBeInTheDocument();
    // A place without a name is listed as such, and opens like any other.
    const [unnamed] = within(results()).getAllByRole("button", { name: new RegExp(`^${s.unnamed} `) });
    await userEvent.click(unnamed);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  }, 30_000);

  it("says why a kind is missing when nothing answered, or something that is not an error", async () => {
    showWith(async (type) => {
      if (type === "Organization") throw new TypeError("Failed to fetch");
      if (type === "Event") return Promise.reject("offline");
      return json(answer(type));
    });
    expect(await screen.findByText(s.refused(s.kind.school, "Failed to fetch"))).toBeInTheDocument();
    expect(await screen.findByText(s.refused(s.kind.event, "offline"))).toBeInTheDocument();
  });

  it("finds its endpoint among several, speaks Slovak without a language, and writes a day it cannot read as given", async () => {
    showWith(
      async (type) => json(type === "Event" ? [{ ...EVENTS[0], startDate: { type: "Property", value: "soon" } }] : answer(type)),
      {
        slug: "",
        space: "elsewhere",
        language: undefined,
        endpoints: [{ slug: SLUG, space: "banskabystrica-verejne", name: "verejne", types: ["Event"] }],
      },
    );
    await userEvent.click(screen.getByRole("checkbox", { name: s.upcomingOnly }));
    await userEvent.click(await within(results()).findByRole("button", { name: /soon/ }));
    const panel = await screen.findByRole("dialog", { name: "Radvanský jarmok" });
    expect(screen.getByRole("heading", { level: 1, name: s.title })).toBeInTheDocument();
    // The shell and its panel speak the App's own fallback, Slovak, not the SDK's English.
    await userEvent.click(within(panel).getByRole("button", { name: "Zavrieť" }));
  });

  it("says it has nothing to read when the app has no endpoint of the public space", () => {
    show(undefined, false);
    expect(screen.getByRole("status")).toHaveTextContent(s.noEndpoint);
  });

  it("names its sources and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(names()).toContain("Radvanský jarmok"));
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
