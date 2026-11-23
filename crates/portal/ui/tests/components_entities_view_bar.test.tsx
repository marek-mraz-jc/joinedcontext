/**
 * ViewBar branches the first suite leaves open: the settings dialog (open, cancel, close, save),
 * a refused delete, and the reload that clears a conflict.
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

const L = en.spaces.saved;
const VIEWS = "/api/v1/projects/hel/spaces/bikes/views";
const MINE: DataView = {
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
};

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

function respond(body: unknown, status = 200): Response {
  return new Response(status === 204 ? null : JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

describe("ViewBar branches", () => {
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

  function mount(props: { current: ViewConfig; write: (s: Sent) => Response }) {
    global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const path = new URL(request.url).pathname;
      const text = request.method === "GET" || request.method === "DELETE" ? "" : await request.clone().text();
      const one = { method: request.method, path, body: text ? JSON.parse(text) : undefined };
      sent.push(one);
      if (path.endsWith("/auth/me")) return respond({ subject: "sub-jana", username: "jana", roles: [] });
      if (path.endsWith("/permissions/me")) return respond({ project: "hel", grants: [{ role: "reader", rule: { kinds: ["ContextSpace"], verbs: ["read"] } }] });
      if (request.method === "GET" && path === VIEWS) return respond({ items: [MINE] });
      return props.write(one);
    }) as typeof fetch;
    const onSelect = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <ViewBar project="hel" space="bikes" type="BikeHireDockingStation" kind="grid" selected={MINE} onSelect={onSelect} current={props.current} />
        </QueryClientProvider>
      </I18nextProvider>,
    );
    return onSelect;
  }

  it("opens the settings of the view, closes them on Cancel and on the close button, and sends nothing", async () => {
    mount({ current: MINE.config, write: () => respond({}, 500) });
    const open = await screen.findByRole("button", { name: L.settings });
    await waitFor(() => expect(open).toBeEnabled());
    await userEvent.click(open);
    let dialog = await screen.findByRole("dialog", { name: L.settingsTitle });
    expect(within(dialog).getByLabelText(new RegExp(L.title))).toHaveValue("Short of bikes");
    await userEvent.click(within(dialog).getByRole("button", { name: en.app.cancel }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await userEvent.click(screen.getByRole("button", { name: L.settings }));
    dialog = await screen.findByRole("dialog", { name: L.settingsTitle });
    await userEvent.click(within(dialog).getByRole("button", { name: en.app.close }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sent.filter((s) => s.method !== "GET")).toEqual([]);
  });

  it("renames a view from its settings, keeping the settings it saved", async () => {
    const renamed = { ...MINE, title: "Empty docks", version: 2 };
    const onSelect = mount({ current: { q: "other" }, write: () => respond(renamed) });
    await userEvent.click(await screen.findByRole("button", { name: L.settings }));
    const dialog = await screen.findByRole("dialog", { name: L.settingsTitle });
    const title = within(dialog).getByLabelText(new RegExp(L.title));
    await userEvent.clear(title);
    await userEvent.type(title, "Empty docks");
    await userEvent.click(within(dialog).getByRole("button", { name: L.confirm }));
    await waitFor(() => expect(onSelect).toHaveBeenCalledWith(renamed));
    const put = sent.find((s) => s.method === "PUT")!;
    expect(put.body).toMatchObject({ title: "Empty docks", config: { q: "availableBikeNumber<3" }, expectedVersion: 1 });
  });

  it("keeps the view and says why when the delete is refused", async () => {
    const onSelect = mount({ current: MINE.config, write: () => respond({ title: "Forbidden", detail: "only a steward may delete a shared view" }, 403) });
    await userEvent.click(await screen.findByRole("button", { name: L.delete }));
    const confirm = await screen.findByRole("alertdialog");
    await userEvent.click(within(confirm).getByRole("button", { name: L.delete }));
    expect(await screen.findByText(/only a steward may delete a shared view/)).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("reloads the views and clears the conflict", async () => {
    mount({ current: { q: "x==1" }, write: () => respond({ title: "Conflict", detail: "the view is at version 3" }, 409) });
    await userEvent.click(await screen.findByRole("button", { name: L.save }));
    await screen.findByText(/the view is at version 3/);
    const reads = sent.filter((s) => s.method === "GET" && s.path === VIEWS).length;
    await userEvent.click(screen.getByRole("button", { name: L.reload }));
    await waitFor(() => expect(screen.queryByText(/the view is at version 3/)).toBeNull());
    expect(sent.filter((s) => s.method === "GET" && s.path === VIEWS).length).toBeGreaterThan(reads);
  });
});
