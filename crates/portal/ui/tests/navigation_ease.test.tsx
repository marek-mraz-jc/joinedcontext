// covers (T-2137, the module gate in gate_modules.test.ts): src/navigation/CommandPalette.tsx,
// src/navigation/shortcuts.ts, src/navigation/places.ts, src/navigation/PageTools.tsx,
// src/navigation/urlState.ts.
/**
 * T-3238, T-3240, T-3241, T-3242 (UI-88, UI-90, UI-91, UI-92): a person finds any page or item
 * they may read with Ctrl+K and a name, comes back to what they opened or starred, sends a page's
 * link, and works the lists and forms from the keyboard without a letter ever taken from a field.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { CommandPalette, narrow } from "../src/navigation/CommandPalette";
import { PageTools } from "../src/navigation/PageTools";
import { isItem, placeOf, remember, reportRecent, resetPlaces, toggleFavourite } from "../src/navigation/places";
import { act as shortcut, ASSISTANT_EVENT, available } from "../src/navigation/shortcuts";
import { resetDocumentTitle, setPageTitle } from "../src/documentTitle";
import { useUrlParam, useUrlText, useUrlUrn } from "../src/navigation/urlState";
import { HandOff, replaceOwnSearch } from "../src/assistant/HandOff";
import { useState } from "react";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: object = {}, title?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki", ...(title ? { title } : {}) },
  spec,
});

interface Seen {
  puts: unknown[];
  reports: unknown[];
}

function stub(preferences: object, seen: Seen = { puts: [], reports: [] }) {
  let stored: object = preferences;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
      if (url.pathname === "/api/v1/preferences/recent") {
        seen.reports.push(await request.json());
        return json(stored);
      }
      if (url.pathname === "/api/v1/preferences") {
        if (request.method === "PUT") {
          stored = (await request.json()) as object;
          seen.puts.push(stored);
        }
        return json(stored);
      }
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }, { name: "ovzdusie" }] });
      if (url.pathname === "/api/v1/projects/helsinki/pipelines") return json(list([manifest("Pipeline", "air-quality", {}, "Air quality around Helsinki")]));
      if (url.pathname === "/api/v1/projects/helsinki/endpoints") return json(list([manifest("Endpoint", "helsinki-bikes")]));
      if (url.pathname === "/api/v1/projects/helsinki/datamodels") {
        return json(list([manifest("DataModel", "helsinki-city", { classes: ["BikeHireDockingStation", "Železnica"] })]));
      }
      if (url.pathname === "/api/v1/branding") return json({ hiddenSections: [] });
      return json(list([]));
    }),
  );
  return seen;
}

function showPalette() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({ routeTree: createRootRoute({ component: () => <CommandPalette project="helsinki" /> }) });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return router;
}

const options = () => screen.getAllByRole("option").map((option) => option.textContent);

beforeEach(async () => {
  resetPlaces();
  window.history.pushState({}, "", "/projects/helsinki/spaces");
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the command palette (T-3238)", () => {
  it("narrows by a name's start before a match inside it, without accents, at most eight a group", () => {
    const rows = [
      { id: "1", group: "A", label: "Bike stations", go: "/1" },
      { id: "2", group: "A", label: "City bikes", go: "/2" },
      { id: "3", group: "B", label: "Železnica", go: "/3" },
      ...Array.from({ length: 12 }, (_, n) => ({ id: `x${n}`, group: "C", label: `bike ${n}`, go: "/x" })),
    ];
    expect(narrow(rows, "bike").map((r) => r.id).slice(0, 2)).toEqual(["1", "2"]);
    expect(narrow(rows, "BIKE").filter((r) => r.group === "C")).toHaveLength(8);
    expect(narrow(rows, "zelez").map((r) => r.id)).toEqual(["3"]);
    expect(narrow(rows, "")).toHaveLength(11);
    expect(narrow(rows, "nothing like it")).toEqual([]);
  });

  it("opens on Ctrl+K with the recent pages first, and never a page of a project the person may not read", async () => {
    stub({
      recent: [
        { path: "/projects/helsinki/pipelines/air-quality/edit", title: "Edit pipeline · helsinki" },
        { path: "/projects/secret/spaces/x", title: "X · secret" },
      ],
      favourites: [{ path: "/catalogue", title: "Open data" }],
    });
    showPalette();
    const person = userEvent.setup();
    await person.keyboard("{Control>}k{/Control}");
    const dialog = await screen.findByRole("dialog", { name: en.palette.title });
    await waitFor(() => expect(within(dialog).getByRole("group", { name: en.palette.groups.recent })).toBeInTheDocument());
    const recent = within(dialog).getByRole("group", { name: en.palette.groups.recent });
    expect(within(recent).getAllByRole("option").map((o) => o.textContent)).toEqual(["Edit pipeline · helsinki"]);
    expect(within(dialog).getByRole("group", { name: en.palette.groups.favourites })).toHaveTextContent("Open data");
    expect(options()[0]).toBe("Edit pipeline · helsinki");
    expect(within(dialog).queryByText("X · secret")).toBeNull();
  });

  it("finds a pipeline by its title, a type by its name, and goes there on Enter", async () => {
    stub({});
    const router = showPalette();
    const person = userEvent.setup();
    await person.keyboard("{Control>}k{/Control}");
    const search = await screen.findByRole("combobox", { name: en.palette.search });
    await person.type(search, "air qual");
    const pipelines = await screen.findByRole("group", { name: en.nav.pipelines });
    expect(within(pipelines).getByRole("option")).toHaveTextContent("Air quality around Helsinki");
    await person.clear(search);
    await person.type(search, "zeleznica");
    const types = await screen.findByRole("group", { name: en.palette.groups.types });
    expect(within(types).getByRole("option")).toHaveTextContent("Železnica");
    await person.clear(search);
    await person.type(search, "air qual");
    await screen.findByRole("group", { name: en.nav.pipelines });
    // What has the name comes before the actions, so Enter opens it.
    expect(options()[0]).toBe("Air quality around Helsinkiair-quality");
    await person.keyboard("{Enter}");
    expect(screen.queryByRole("dialog", { name: en.palette.title })).toBeNull();
    await waitFor(() => expect(router.state.location.pathname).toBe("/projects/helsinki/pipelines/air-quality"));
  });

  it("opens an entity by its URN in the explorer, and lists the projects the person may read", async () => {
    stub({});
    const router = showPalette();
    const person = userEvent.setup();
    await person.keyboard("{Control>}k{/Control}");
    const search = await screen.findByRole("combobox", { name: en.palette.search });
    await waitFor(() => expect(screen.getByRole("group", { name: en.palette.groups.projects })).toHaveTextContent("ovzdusie"));
    await person.type(search, "urn:ngsi-ld:Station:7");
    expect(options()[0]).toBe("Open the entity urn:ngsi-ld:Station:7urn:ngsi-ld:Station:7");
    // The echo beside the label gives way on a phone, where it cut the label short (T-4165).
    const echo = within(screen.getAllByRole("option")[0]).getAllByText("urn:ngsi-ld:Station:7")[0];
    expect(echo).toHaveClass("max-sm:hidden");
    await person.keyboard("{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toBe("/projects/helsinki/explore"));
    expect(router.state.location.search).toEqual({ entityId: "urn:ngsi-ld:Station:7" });
  });

  it("hands what was typed to the assistant on a second Ctrl+K", async () => {
    stub({});
    showPalette();
    const asked: string[] = [];
    const listen = (event: Event) => asked.push((event as CustomEvent<{ text: string }>).detail.text);
    window.addEventListener(ASSISTANT_EVENT, listen);
    const person = userEvent.setup();
    await person.keyboard("{Control>}k{/Control}");
    await person.type(await screen.findByRole("combobox", { name: en.palette.search }), "free bikes now");
    await person.keyboard("{Control>}k{/Control}");
    expect(asked).toEqual(["free bikes now"]);
    expect(screen.queryByRole("dialog", { name: en.palette.title })).toBeNull();
    window.removeEventListener(ASSISTANT_EVENT, listen);
  });
});

describe("the shortcuts (T-3242)", () => {
  let root: HTMLElement | null = null;
  function page(html: string) {
    root = document.createElement("div");
    root.innerHTML = html;
    document.body.append(root);
  }
  afterEach(() => {
    root?.remove();
    root = null;
  });
  const key = (init: KeyboardEventInit, target: EventTarget = document.body) => {
    const event = new KeyboardEvent("keydown", { bubbles: true, ...init });
    Object.defineProperty(event, "target", { value: target });
    return event;
  };

  it("walks the list's rows with J and K, and never takes a letter from a field", () => {
    page(`<main><input id="f"/><a data-row-link href="/a">a</a><a data-row-link href="/b">b</a></main>`);
    Element.prototype.scrollIntoView = vi.fn();
    expect(shortcut(key({ key: "j" }))).toBe(true);
    expect(document.activeElement?.textContent).toBe("a");
    shortcut(key({ key: "j" }));
    shortcut(key({ key: "j" }));
    expect(document.activeElement?.textContent).toBe("b");
    shortcut(key({ key: "k" }));
    expect(document.activeElement?.textContent).toBe("a");
    expect(shortcut(key({ key: "j" }, document.getElementById("f") as HTMLElement))).toBe(false);
  });

  it("presses the page's New and Test, saves the form with Ctrl+S even from a field, and lists only what works", () => {
    page(`<main><form><input id="name"/></form><button data-shortcut="new">New</button><button data-shortcut="test" disabled>Test</button></main>`);
    const pressed: string[] = [];
    document.querySelector("[data-shortcut=new]")?.addEventListener("click", () => pressed.push("new"));
    const form = document.querySelector("form") as HTMLFormElement;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      pressed.push("save");
    });
    expect(shortcut(key({ key: "n" }))).toBe(true);
    expect(shortcut(key({ key: "t" }))).toBe(false);
    const field = document.getElementById("name") as HTMLInputElement;
    field.focus();
    expect(shortcut(key({ key: "s", ctrlKey: true }, field))).toBe(true);
    expect(shortcut(key({ key: "n" }, field))).toBe(false);
    expect(pressed).toEqual(["new", "save"]);
    expect(available()).toEqual(["palette", "save", "new", "close", "help"]);
  });

  // F-R2UI2-003: the page's form comes first in the document, the portaled dialog after it.
  it("saves the open dialog's form with Ctrl+S, never the page's form behind it", () => {
    page(`<main><form id="page"></form></main><div role="dialog"><button id="in-dialog">x</button><form id="dialog"></form></div>`);
    const submitted: string[] = [];
    for (const form of document.querySelectorAll("form")) {
      form.addEventListener("submit", (event) => {
        event.preventDefault();
        submitted.push(form.id);
      });
    }
    (document.getElementById("in-dialog") as HTMLButtonElement).focus();
    expect(shortcut(key({ key: "s", ctrlKey: true }))).toBe(true);
    expect(submitted).toEqual(["dialog"]);
  });

  it("saves nothing behind an open dialog that holds no form", () => {
    page(`<main><form id="page"></form></main><div role="dialog"><button>x</button></div>`);
    const submitted: string[] = [];
    document.getElementById("page")?.addEventListener("submit", (event) => {
      event.preventDefault();
      submitted.push("page");
    });
    shortcut(key({ key: "s", ctrlKey: true }));
    expect(submitted).toEqual([]);
  });

  it("? lists the page's shortcuts, and does nothing typed into a field", async () => {
    stub({});
    showPalette();
    const person = userEvent.setup();
    await person.keyboard("?");
    const help = await screen.findByRole("dialog", { name: en.shortcuts.title });
    expect(within(help).getByText(en.shortcuts.palette)).toBeInTheDocument();
    expect(within(help).queryByText(en.shortcuts.new)).toBeNull();
    await person.keyboard("{Escape}");
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    await person.keyboard("?");
    expect(screen.queryByRole("dialog", { name: en.shortcuts.title })).toBeNull();
    expect(field).toHaveValue("?");
  });
});

describe("recent and starred pages (T-3240)", () => {
  it("keeps the last ten item pages, newest first, once each, and reports them in one write when the tab is hidden", async () => {
    const seen = stub({ theme: "dark", recent: [] });
    for (let n = 0; n < 12; n += 1) {
      await remember({ path: `/projects/helsinki/pipelines/p${n}/edit`, title: `p${n}` });
    }
    await remember({ path: "/projects/helsinki/pipelines/p5/edit", title: "p5" });
    // A visit is no write: page tests that count a form's writes see none of these.
    expect(seen.puts).toEqual([]);
    expect(seen.reports).toEqual([]);
    Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
    document.dispatchEvent(new Event("visibilitychange"));
    Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
    await waitFor(() => expect(seen.reports).toHaveLength(1));
    const report = seen.reports[0] as { places: { title: string }[] };
    expect(report.places.map((p) => p.title)).toEqual(["p5", "p11", "p10", "p9", "p8", "p7", "p6", "p4", "p3", "p2"]);
    await reportRecent();
    expect(seen.reports).toHaveLength(1);
  });

  it("stars a page and takes the star away", async () => {
    const seen = stub({});
    await toggleFavourite({ path: "/catalogue", title: "Open data" });
    expect((seen.puts.at(-1) as { favourites: unknown[] }).favourites).toEqual([{ path: "/catalogue", title: "Open data" }]);
    await toggleFavourite({ path: "/catalogue", title: "Open data" });
    expect((seen.puts.at(-1) as { favourites?: unknown[] }).favourites ?? []).toEqual([]);
  });

  it("names a place by its path and query without the language, and tells an item from a list", () => {
    expect(placeOf({ pathname: "/projects/helsinki/explore", search: "?lang=sk&type=Event" })).toBe("/projects/helsinki/explore?type=Event");
    expect(placeOf({ pathname: "/catalogue", search: "?lang=en" })).toBe("/catalogue");
    expect(isItem("/projects/helsinki/pipelines/air-quality/edit")).toBe(true);
    expect(isItem("/catalogue/air")).toBe(true);
    expect(isItem("/projects/helsinki/pipelines")).toBe(false);
    expect(isItem("/catalogue")).toBe(false);
  });
});

describe("a page's star and link (T-3240, T-3241)", () => {
  function tools() {
    setPageTitle(["air-quality", "helsinki"]);
    const router = createRouter({ routeTree: createRootRoute({ component: () => <PageTools /> }) });
    render(
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>,
    );
  }

  it("shows nothing before the page has named itself", async () => {
    stub({});
    resetDocumentTitle();
    const router = createRouter({ routeTree: createRootRoute({ component: () => <PageTools /> }) });
    render(
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>,
    );
    await waitFor(() => expect(router.state.status).toBe("idle"));
    expect(screen.queryByRole("button", { name: en.places.copyLink })).toBeNull();
  });

  it("copies the page's address with its query and without the language, and remembers the item page", async () => {
    const seen = stub({});
    window.history.pushState({}, "", "/projects/helsinki/pipelines/air-quality/edit?tab=runs&lang=sk");
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    tools();
    const person = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await person.click(await screen.findByRole("button", { name: en.places.copyLink }));
    expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/projects/helsinki/pipelines/air-quality/edit?tab=runs`);
    expect(screen.getByText(en.places.linkCopied)).toBeInTheDocument();
    await reportRecent();
    expect(seen.reports).toEqual([
      { places: [{ path: "/projects/helsinki/pipelines/air-quality/edit?tab=runs", title: "air-quality · helsinki" }] },
    ]);
  });

  it("says where to copy from when the browser refuses, and shows the star pressed once starred", async () => {
    stub({});
    window.history.pushState({}, "", "/projects/helsinki/pipelines");
    tools();
    const person = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn(async () => Promise.reject(new Error("denied"))) },
      configurable: true,
    });
    await person.click(await screen.findByRole("button", { name: en.places.copyLink }));
    expect(screen.getByText(en.places.linkNotCopied)).toBeInTheDocument();
    const star = screen.getByRole("button", { name: en.places.star });
    expect(star).toHaveAttribute("aria-pressed", "false");
    await act(async () => {
      await person.click(star);
    });
    await waitFor(() => expect(star).toHaveAttribute("aria-pressed", "true"));
  });
});

describe("a page's choice in the address (T-3239)", () => {
  function Tabbed() {
    const [tab, setTab] = useUrlParam<"pages" | "documents">("tab", "pages", ["pages", "documents"]);
    return (
      <div>
        <p>shown: {tab}</p>
        <button type="button" onClick={() => setTab("documents")}>documents</button>
        <button type="button" onClick={() => setTab("pages")}>pages</button>
      </div>
    );
  }
  function show(path: string) {
    window.history.pushState({}, "", path);
    const router = createRouter({ routeTree: createRootRoute({ component: Tabbed }) });
    render(<RouterProvider router={router} />);
    return router;
  }

  it("keeps the tab in the query beside what else is there, and back brings the previous one", async () => {
    const router = show("/projects/helsinki/knowledge/web?lang=sk");
    expect(await screen.findByText("shown: pages")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "documents" }));
    await screen.findByText("shown: documents");
    expect(router.state.location.searchStr).toBe("?lang=sk&tab=documents");
    // The default is no parameter at all.
    await userEvent.click(screen.getByRole("button", { name: "pages" }));
    await screen.findByText("shown: pages");
    expect(router.state.location.searchStr).toBe("?lang=sk");
    act(() => router.history.back());
    expect(await screen.findByText("shown: documents")).toBeInTheDocument();
  });

  it("reads a value the page does not know as the default", async () => {
    show("/x?tab=%3Cscript%3E");
    expect(await screen.findByText("shown: pages")).toBeInTheDocument();
  });
});

describe("an open row and its place in the address (T-5602)", () => {
  const URN = "urn:ngsi-ld:AirQualityObserved:hel:1";
  function Rows() {
    const [row, setRow] = useUrlUrn("row", ["tab", "attr"]);
    const [tab, setTab] = useUrlText("tab");
    const [q, setQ] = useUrlText("q");
    return (
      <div>
        <p>row: {row ?? "none"}, tab: {tab ?? "none"}, q: {q ?? "none"}</p>
        <button type="button" onClick={() => setRow(URN)}>open</button>
        <button type="button" onClick={() => setRow(null)}>close</button>
        <button type="button" onClick={() => setTab("history")}>history</button>
        <button type="button" onClick={() => setQ("Kallio")}>search</button>
        <button type="button" onClick={() => setQ("x".repeat(300))}>long</button>
      </div>
    );
  }
  function show(path: string) {
    window.history.pushState({}, "", path);
    const router = createRouter({ routeTree: createRootRoute({ component: Rows }) });
    render(<RouterProvider router={router} />);
    return router;
  }

  it("opens with a new entry and closes back to the one before it, so Back leaves the page", async () => {
    const router = show("/s?type=AirQualityObserved");
    await screen.findByText(/row: none/);
    const before = window.history.length;
    await userEvent.click(screen.getByRole("button", { name: "open" }));
    await screen.findByText(new RegExp(`row: ${URN}`));
    expect(window.history.length).toBe(before + 1);
    // The tab is written in place: the open entry stays the last one.
    await userEvent.click(screen.getByRole("button", { name: "history" }));
    await screen.findByText(/tab: history/);
    expect(window.history.length).toBe(before + 1);
    await userEvent.click(screen.getByRole("button", { name: "close" }));
    await screen.findByText("row: none, tab: none, q: none");
    expect(router.state.location.searchStr).toBe("?type=AirQualityObserved");
    // Forward reopens the row: the close went back, it pushed nothing.
    act(() => router.history.forward());
    expect(await screen.findByText(new RegExp(`row: ${URN}, tab: history`))).toBeInTheDocument();
  });

  it("closes a row a link opened in place, its tab and attribute with it", async () => {
    const router = show(`/s?type=AirQualityObserved&row=${encodeURIComponent(URN)}&tab=history&attr=pm10`);
    await screen.findByText(new RegExp(`row: ${URN}, tab: history`));
    const before = window.history.length;
    await userEvent.click(screen.getByRole("button", { name: "close" }));
    await screen.findByText(/row: none, tab: none/);
    expect(router.state.location.searchStr).toBe("?type=AirQualityObserved");
    expect(window.history.length).toBe(before);
  });

  it("keeps the search in place and refuses one longer than the address carries", async () => {
    const router = show("/s");
    await screen.findByText(/q: none/);
    const before = window.history.length;
    await userEvent.click(screen.getByRole("button", { name: "search" }));
    await screen.findByText(/q: Kallio/);
    expect(window.history.length).toBe(before);
    await userEvent.click(screen.getByRole("button", { name: "long" }));
    await waitFor(() => expect(router.state.location.searchStr).toBe(`?q=${"x".repeat(200)}`));
    show(`/s?q=${"y".repeat(201)}`);
    expect(await screen.findAllByText(/q: none/)).not.toHaveLength(0);
  });
});

describe("a page writing its own choices in the address (T-3239)", () => {
  it("is not mounted afresh for what it wrote, and still is for a hand-off", async () => {
    let mounts = 0;
    function Counted() {
      const [at] = useState(() => {
        mounts += 1;
        return mounts;
      });
      return <p>mount {at}</p>;
    }
    window.history.pushState({}, "", "/projects/helsinki/explore");
    const router = createRouter({
      routeTree: createRootRoute({
        component: () => (
          <HandOff>
            <Counted />
          </HandOff>
        ),
      }),
    });
    render(<RouterProvider router={router} />);
    expect(await screen.findByText("mount 1")).toBeInTheDocument();
    act(() => replaceOwnSearch(new URLSearchParams("space=ovzdusie&type=Event")));
    await waitFor(() => expect(router.state.location.searchStr).toBe("?space=ovzdusie&type=Event"));
    expect(screen.getByText("mount 1")).toBeInTheDocument();
    act(() => router.history.push("/projects/helsinki/explore?space=helsinki"));
    expect(await screen.findByText("mount 2")).toBeInTheDocument();
  });
});
