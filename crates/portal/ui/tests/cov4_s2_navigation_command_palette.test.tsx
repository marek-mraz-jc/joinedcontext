// T-7706: CommandPalette: page shortcuts while closed, arrow/Home/End keys, mouse hover and click on a row,
// the assistant row's function target, and a data model whose class list holds a non-name.
import { render, screen, waitFor, within, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { doorsAsRest } from "./odata";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { CommandPalette } from "../src/navigation/CommandPalette";
import { resetPlaces } from "../src/navigation/places";
import { ASSISTANT_EVENT } from "../src/navigation/shortcuts";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: object = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki" },
  spec,
});

function stub() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url, "http://localhost");
      const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
      if (url.pathname === "/api/v1/preferences" || url.pathname === "/api/v1/preferences/recent") return json({});
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
      if (url.pathname === "/api/v1/projects/helsinki/datamodels") {
        return json(list([manifest("DataModel", "city", { classes: ["Station", 5, "Bike"] })]));
      }
      if (url.pathname === "/api/v1/branding") return json({ hiddenSections: [] });
      return json(list([]));
    }),
  );
  doorsAsRest();
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

beforeEach(async () => {
  resetPlaces();
  window.history.pushState({}, "", "/projects/helsinki/spaces");
  await i18n.changeLanguage("en");
  stub();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("CommandPalette coverage (T-7706)", () => {
  it("clicks the page's marked New button on the letter n while the palette is closed", async () => {
    const clicked = vi.fn();
    const button = document.createElement("button");
    button.dataset.shortcut = "new";
    button.addEventListener("click", clicked);
    document.body.append(button);
    showPalette();
    await userEvent.keyboard("n");
    expect(clicked).toHaveBeenCalledTimes(1);
    button.remove();
  });

  it("moves the highlight with the arrows, Home and End, and follows the mouse", async () => {
    showPalette();
    const person = userEvent.setup();
    await person.keyboard("{Control>}k{/Control}");
    const search = await screen.findByRole("combobox", { name: en.palette.search });
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(2));
    const selected = () => screen.getAllByRole("option").findIndex((o) => o.getAttribute("aria-selected") === "true");
    expect(selected()).toBe(0);
    await person.keyboard("{ArrowDown}{ArrowDown}");
    expect(selected()).toBe(2);
    await person.keyboard("{ArrowUp}");
    expect(selected()).toBe(1);
    await person.keyboard("{End}");
    expect(selected()).toBe(screen.getAllByRole("option").length - 1);
    await person.keyboard("{Home}");
    expect(selected()).toBe(0);
    fireEvent.mouseMove(screen.getAllByRole("option")[3]);
    expect(selected()).toBe(3);
    expect(search).toHaveAttribute("aria-activedescendant");
  });

  it("opens a row by clicking it", async () => {
    const router = showPalette();
    const person = userEvent.setup();
    await person.keyboard("{Control>}k{/Control}");
    await person.type(await screen.findByRole("combobox", { name: en.palette.search }), "station");
    const types = await screen.findByRole("group", { name: en.palette.groups.types });
    expect(within(types).getAllByRole("option")).toHaveLength(1);
    await person.click(within(types).getByRole("option", { name: /Station/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/projects/helsinki/explore"));
    expect(router.state.location.search).toEqual({ type: "Station" });
    expect(screen.queryByRole("dialog", { name: en.palette.title })).toBeNull();
  });

  it("sends what was typed to the assistant when its row is chosen", async () => {
    showPalette();
    const asked: string[] = [];
    const listen = (event: Event) => asked.push((event as CustomEvent<{ text: string }>).detail.text);
    window.addEventListener(ASSISTANT_EVENT, listen);
    const person = userEvent.setup();
    await person.keyboard("{Control>}k{/Control}");
    await person.type(await screen.findByRole("combobox", { name: en.palette.search }), "free bikes now");
    await person.click(await screen.findByRole("option", { name: new RegExp(en.palette.actions.ask) }));
    expect(asked).toEqual(["free bikes now"]);
    window.removeEventListener(ASSISTANT_EVENT, listen);
  });

  it("does nothing on Enter when no row matches", async () => {
    showPalette();
    const person = userEvent.setup();
    await person.keyboard("{Control>}k{/Control}");
    const search = await screen.findByRole("combobox", { name: en.palette.search });
    fireEvent.change(search, { target: { value: "" } });
    await person.keyboard("{Enter}");
    expect(window.location.pathname).toBe("/projects/helsinki/spaces");
  });
});
