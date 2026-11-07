/**
 * Saved views of a space (API/01 §30, T-3104): pick one, save the grid into it or into a new one,
 * share it, delete it, and hear about a save someone else made meanwhile.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ViewBar } from "../src/components/entities/ViewBar";
import type { DataView, ViewConfig } from "../src/api/dataViews";
import { sameConfig } from "../src/api/dataViews";

const L = en.spaces.saved;
const VIEWS = "/api/v1/projects/hel/spaces/bikes/views";

function viewOf(over: Partial<DataView>): DataView {
  return {
    id: "7f1c2a9e-4b1d-4a57-9a0e-2f6d1c3b8e01",
    type: "BikeHireDockingStation",
    kind: "grid",
    mode: "personal",
    title: "Short of bikes",
    owner: "jana",
    config: { q: "availableBikeNumber<3" },
    version: 1,
    createdAt: "2026-10-06T19:00:00Z",
    updatedAt: "2026-10-06T19:00:00Z",
    ...over,
  };
}

const MINE = viewOf({});
const EVAS = viewOf({ id: "1b2c3d4e-0000-4000-8000-000000000002", title: "Eva's board", owner: "eva", mode: "locked" });
const TRAMS = viewOf({ id: "1b2c3d4e-0000-4000-8000-000000000003", title: "Trams", type: "Tram" });

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

function respond(body: unknown, status = 200): Response {
  return new Response(status === 204 ? null : JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

describe("ViewBar", () => {
  let originalFetch: typeof global.fetch;
  let sent: Sent[];

  beforeEach(async () => {
    originalFetch = global.fetch;
    sent = [];
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    global.fetch = originalFetch;
  });

  function mount(props: { selected: DataView | null; current: ViewConfig; write?: (s: Sent) => Response }) {
    global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const path = new URL(request.url).pathname;
      const text = request.method === "GET" || request.method === "DELETE" ? "" : await request.clone().text();
      const one = { method: request.method, path, body: text ? JSON.parse(text) : undefined };
      sent.push(one);
      if (path.endsWith("/auth/me")) return respond({ subject: "sub-jana", username: "jana", roles: [] });
      if (path.endsWith("/permissions/me")) return respond({ project: "hel", grants: [{ role: "reader", rule: { kinds: ["ContextSpace"], verbs: ["read"] } }] });
      if (request.method === "GET" && path === VIEWS) return respond({ items: [MINE, EVAS, TRAMS] });
      return props.write ? props.write(one) : respond({}, 500);
    }) as typeof fetch;
    const onSelect = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <ViewBar project="hel" space="bikes" type="BikeHireDockingStation" kind="grid" selected={props.selected} onSelect={onSelect} current={props.current} />
        </QueryClientProvider>
      </I18nextProvider>,
    );
    return onSelect;
  }

  it("lists the views of this type and applies the one picked", async () => {
    const onSelect = mount({ selected: null, current: {} });
    const picker = await screen.findByLabelText(L.view);
    await waitFor(() => expect(within(picker).getAllByRole("option")).toHaveLength(3));
    expect(within(picker).getAllByRole("option").map((o) => o.textContent)).toEqual([
      en.spaces.inside.allRows,
      `Short of bikes · ${L.modes.personal}`,
      `Eva's board · ${L.modes.locked}`,
    ]);
    await userEvent.selectOptions(picker, MINE.id);
    expect(onSelect).toHaveBeenCalledWith(MINE);
  });

  it("saves what the grid shows into the view, sending the version it read", async () => {
    const saved = viewOf({ config: { q: "availableBikeNumber<2" }, version: 2 });
    const onSelect = mount({ selected: MINE, current: { q: "availableBikeNumber<2" }, write: () => respond(saved) });
    expect(await screen.findByText(L.dirty)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: L.save }));
    await waitFor(() => expect(onSelect).toHaveBeenCalledWith(saved));
    const put = sent.find((s) => s.method === "PUT")!;
    expect(put.path).toBe(`${VIEWS}/${MINE.id}`);
    expect(put.body).toMatchObject({ mode: "personal", title: "Short of bikes", config: { q: "availableBikeNumber<2" }, expectedVersion: 1 });
  });

  it("offers no save while nothing changed, and none on someone else's locked view", async () => {
    mount({ selected: MINE, current: { q: "availableBikeNumber<3" } });
    expect(await screen.findByRole("button", { name: L.save })).toBeDisabled();
  });

  it("keeps a locked view of someone else read-only and says why", async () => {
    mount({ selected: EVAS, current: { q: "x==1" } });
    const save = await screen.findByRole("button", { name: L.save });
    // Refused with the reason a screen reader hears, not silently greyed out.
    await waitFor(() => expect(save).toHaveAttribute("aria-disabled", "true"));
    expect(save).toHaveAccessibleDescription(L.lockedReason);
    const remove = screen.getByRole("button", { name: L.delete });
    expect(remove).toHaveAttribute("aria-disabled", "true");
    expect(remove).toHaveAccessibleDescription(L.governReason);
  });

  it("saves the grid as a new shared view under the name given", async () => {
    const created = viewOf({ id: "1b2c3d4e-0000-4000-8000-000000000009", title: "Empty docks", mode: "collaborative" });
    const onSelect = mount({ selected: null, current: { q: "availableBikeNumber==0" }, write: () => respond(created, 201) });
    await userEvent.click(await screen.findByRole("button", { name: L.saveNew }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(new RegExp(L.title)), "Empty docks");
    await userEvent.click(within(dialog).getByRole("radio", { name: new RegExp(`^${L.modes.collaborative}\\b(?!,)`) }));
    await userEvent.click(within(dialog).getByRole("button", { name: L.confirm }));
    await waitFor(() => expect(onSelect).toHaveBeenCalledWith(created));
    expect(sent.find((s) => s.method === "POST")?.body).toEqual({
      type: "BikeHireDockingStation",
      kind: "grid",
      mode: "collaborative",
      title: "Empty docks",
      config: { q: "availableBikeNumber==0" },
    });
  });

  it("says another save happened meanwhile instead of overwriting it", async () => {
    mount({
      selected: MINE,
      current: { q: "availableBikeNumber<1" },
      write: () => respond({ title: "Conflict", detail: "the view was changed meanwhile: it is at version 3, reload it before saving" }, 409),
    });
    await userEvent.click(await screen.findByRole("button", { name: L.save }));
    expect(await screen.findByText(/it is at version 3/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: L.reload })).toBeInTheDocument();
  });

  it("deletes its own view after asking, and only the view", async () => {
    const onSelect = mount({ selected: MINE, current: MINE.config, write: () => respond(null, 204) });
    await userEvent.click(await screen.findByRole("button", { name: L.delete }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm).toHaveTextContent(L.deleteHelp);
    await userEvent.click(within(confirm).getByRole("button", { name: L.delete }));
    await waitFor(() => expect(onSelect).toHaveBeenCalledWith(null));
    expect(sent.find((s) => s.method === "DELETE")?.path).toBe(`${VIEWS}/${MINE.id}`);
  });
});

describe("sameConfig", () => {
  it("reads empty and absent settings alike, and hidden attributes in any order", () => {
    expect(sameConfig({ q: "", sort: [], hidden: ["b", "a"] }, { hidden: ["a", "b"] })).toBe(true);
    expect(sameConfig({ q: "a==1" }, { q: "a==2" })).toBe(false);
  });
});
