/**
 * The region map over what the public endpoint of `bbsk-registre` answers (T-2784).
 *
 * `fetch` is what is stubbed, so every case goes through the SDK's own endpoint source. MapLibre
 * needs WebGL, which jsdom does not have, so the library is a double recording what it was given.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider, projectRow, toRichRow } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import type { EntitySource } from "@joinedcontext/sdk";

const setData = vi.fn();
const addSource = vi.fn();
/** What the map does on a click of a drawn place, as MapLibre would call it. */
let clickPlace: ((event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) | null = null;

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, layerOrHandler: unknown, handler?: (event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) {
      if (event === "load" && typeof layerOrHandler === "function") layerOrHandler();
      if (event === "click" && handler) clickPlace = handler;
    }
    addSource = addSource;
    addLayer = vi.fn();
    getSource = () => ({ setData });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const { default: App, LayerNotes, loadKind, MOST } = await import("./App");
const { answer, HOSPITALS, keyOf, ORGANIZATIONS, SOCIAL } = await import("./fixtures/registre");
const { LOCALES } = await import("./locales");
const s = LOCALES.sk;

const SLUG = "rv5cn2kxq7bmt3wza6hjd4ype2";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const ALL = [...HOSPITALS, ...SOCIAL, ...ORGANIZATIONS];

function show(
  refuse?: string,
  withEndpoint = true,
  endpoints?: { name: string; slug: string; space: string; types: string[] }[],
  answerOf: (type: string | null) => unknown = answer,
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const type = keyOf(new URL(path, "http://portal.test"));
      const special = answerOf(type);
      if (special instanceof Error || typeof special === "string") throw special;
      if (type === refuse) {
        return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "the policy does not grant this type" }), {
          status: 403,
          headers: { "content-type": "application/problem+json" },
        });
      }
      return json(special);
    }),
  );
  // The map reads through `fetch`, the entity panel through the client: the same rows on both.
  const client = stubClient({ entities: ALL.map((entity) => projectRow(toRichRow(entity, "sk"), "sk")) }, {
    slug: withEndpoint ? SLUG : "",
    orgDomain: "bbsk.sk",
    space: withEndpoint ? "bbsk-registre" : "elsewhere",
    transport: "origin",
    appName: "bbsk-mapa",
    language: "sk",
    ...(endpoints ? { endpoints } : {}),
    portal: "https://portal.bbsk.sk/projects/bbsk",
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  // The handler belongs to the map of the test that drew it; the next test waits for its own.
  clickPlace = null;
});

const results = () => screen.getByRole("region", { name: /miest/ });
const names = () => within(results()).queryAllByRole("button").map((button) => button.querySelector(".name")?.textContent);

describe("the region map", () => {
  it("lists the hospitals, social services and organizations, and draws those with a position", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Nemocnica Zvolen"));
    expect(names()).toEqual([
      "Domov sociálnych služieb Tisovec",
      "Fakultná nemocnica s poliklinikou F. D. Roosevelta",
      "Nemocnica Zvolen",
      "Spojená škola Detva",
      "Stredoslovenské múzeum",
      "Terénna opatrovateľská služba",
    ]);
    // Social services publish no position: listed and said so, not drawn.
    expect(within(results()).getAllByText(new RegExp(s.notOnMap)).length).toBe(3);
    await waitFor(() => {
      const data = setData.mock.calls.at(-1)?.[0] as { features: unknown[] } | undefined;
      expect(data?.features).toHaveLength(3);
    });
  });

  it("narrows by layer and by search, without diacritics", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Nemocnica Zvolen"));
    await userEvent.click(screen.getByRole("checkbox", { name: new RegExp(`^${s.kind.hospital}`) }));
    expect(names()).not.toContain("Nemocnica Zvolen");
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "rimavska");
    expect(names()).toEqual(["Domov sociálnych služieb Tisovec"]);
    await userEvent.clear(screen.getByRole("searchbox", { name: s.search }));
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "nic take");
    expect(within(results()).getByText(s.noResults)).toBeInTheDocument();
  });

  it("opens a place from the list in the SDK's panel, links it to the Portal, and Escape gives the focus back", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Domov sociálnych služieb Tisovec"));
    const item = within(results()).getByRole("button", { name: /Tisovec/ });
    await userEvent.click(item);
    expect(item).toHaveAttribute("aria-pressed", "true");
    const panel = await screen.findByRole("dialog", { name: "Domov sociálnych služieb Tisovec" });
    expect(await within(panel).findByText(/Jesenského 869/)).toBeInTheDocument();
    expect(within(panel).getByText("48")).toBeInTheDocument();
    const link = within(panel).getByRole("link", { name: "Otvoriť v Portáli" });
    expect(link).toHaveAttribute("href", expect.stringContaining(`entityId=${encodeURIComponent(SOCIAL[0].id)}`));
    // Following the link leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    fireEvent.click(link);
    expect(followed).toHaveBeenCalledTimes(1);
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(item).toHaveFocus());
  });

  it("shows a link the register publishes as text, so a script URL never runs", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Terénna opatrovateľská služba"));
    await userEvent.click(within(results()).getByRole("button", { name: /Terénna/ }));
    const panel = await screen.findByRole("dialog", { name: "Terénna opatrovateľská služba" });
    expect(await within(panel).findByText("javascript:alert(1)")).toBeInTheDocument();
    expect(within(panel).getAllByRole("link").map((one) => one.getAttribute("href"))).toEqual([expect.stringContaining("https://portal.bbsk.sk/")]);
    await userEvent.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens a place clicked on the map, through the endpoint the Portal lists, and ignores a click on nothing", async () => {
    show(undefined, false, [{ name: "registre", slug: SLUG, space: "bbsk-registre", types: [] }]);
    await waitFor(() => expect(names()).toContain("Nemocnica Zvolen"));
    // The map binds its click in an effect that may run after the list is on screen (T-4257).
    await waitFor(() => expect(clickPlace).not.toBeNull());
    act(() => clickPlace?.({ features: [] }));
    act(() => clickPlace?.({ features: [{ properties: { id: "urn:ngsi-ld:PointOfInterest:gone" } }] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => clickPlace?.({ features: [{ properties: { id: HOSPITALS[1].id } }] }));
    const panel = await screen.findByRole("dialog", { name: "Nemocnica Zvolen" });
    expect(await within(panel).findByText(/Kuzmányho nábrežie 28/)).toBeInTheDocument();
    expect(within(results()).getByRole("button", { name: /Nemocnica Zvolen/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("opens every place of the list, one after another", async () => {
    show();
    await waitFor(() => expect(names()).toHaveLength(6));
    for (const name of names() as string[]) {
      await userEvent.click(within(results()).getByRole("button", { name: new RegExp(`^${name}`) }));
      const panel = await screen.findByRole("dialog", { name });
      await userEvent.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    }
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("turns each layer off and on again", async () => {
    show();
    // Each layer's box is named before its count arrives, and each one is used then as well.
    for (const kind of ["hospital", "social", "organization"] as const) {
      const box = screen.getByRole("checkbox", { name: s.kind[kind] });
      fireEvent.click(box);
      fireEvent.click(box);
      expect(box).toBeChecked();
    }
    await waitFor(() => expect(names()).toContain("Stredoslovenské múzeum"));
    for (const kind of ["hospital", "social", "organization"] as const) {
      // Named with its count once the kind has loaded.
      const box = screen.getByRole("checkbox", { name: new RegExp(`^${s.kind[kind]}`) });
      await userEvent.click(box);
      expect(box).not.toBeChecked();
      await userEvent.click(box);
      expect(box).toBeChecked();
    }
    expect(names()).toHaveLength(6);
  });

  it("says why a kind the endpoint refuses is missing and keeps the others", async () => {
    show("hospital");
    expect(await screen.findByRole("alert")).toHaveTextContent(s.refused(s.kind.hospital, "the policy does not grant this type"));
    await waitFor(() => expect(names()).toContain("Stredoslovenské múzeum"));
    expect(names()).not.toContain("Nemocnica Zvolen");
    // Asking again never grants a right.
    expect(within(screen.getByRole("alert")).queryByRole("button")).toBeNull();
  });

  it("reads a kind again on Retry after a failure on the way", async () => {
    let down = true;
    show(undefined, true, undefined, (type) => (type === "hospital" && down ? new TypeError("offline") : answer(type)));
    const alert = await screen.findByRole("alert");
    await waitFor(() => expect(names()).toContain("Stredoslovenské múzeum"));
    down = false;
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(names()).toContain("Nemocnica Zvolen"));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("never says nothing was found while a kind has failed", async () => {
    show(undefined, true, undefined, () => new TypeError("offline"));
    await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(3));
    expect(screen.queryByText(s.noResults)).toBeNull();
  });

  it("says why a kind failed when the request itself fails, in its own words", async () => {
    show(undefined, true, undefined, (type) => (type === "hospital" ? new TypeError("offline") : type === "socialService" ? "down" : answer(type)));
    await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(2));
    const said = screen.getAllByRole("alert").map((alert) => alert.textContent);
    expect(said[0]).toContain(s.kind.hospital);
    expect(said[0]).toContain("offline");
    expect(said[1]).toContain(s.kind.social);
    expect(names()).toEqual(["Spojená škola Detva", "Stredoslovenské múzeum"]);
  });

  it("holds at most a thousand places of a kind and says the rest are cut off", async () => {
    // Every page answers a full 200: the map asks five pages and stops there.
    const asked: number[] = [];
    const source = {
      query: async (_query: unknown, page: { offset: number; limit: number }) => {
        asked.push(page.offset);
        return { rows: Array.from({ length: page.limit }, (_, at) => toRichRow({ ...HOSPITALS[1], id: `${HOSPITALS[1].id}-${page.offset + at}` })) };
      },
    } as unknown as EntitySource;
    const layer = await loadKind(source, "hospital", "sk");
    expect(asked).toEqual([0, 200, 400, 600, 800]);
    expect(layer).toMatchObject({ status: "ready", truncated: true });
    render(<LayerNotes layers={{ hospital: layer, social: { status: "loading" }, organization: { status: "ready", places: [], truncated: false } }} s={s} onRetry={() => {}} />);
    expect(screen.getByText(s.truncated(s.kind.hospital, MOST))).toBeInTheDocument();
  });

  it("says it has nothing to read when the app has no endpoint of the register space", () => {
    show(undefined, false);
    expect(screen.getByText(s.noEndpoint)).toBeInTheDocument();
  });

  it("names its source and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(names()).toContain("Nemocnica Zvolen"));
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
