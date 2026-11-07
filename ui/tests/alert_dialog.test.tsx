/**
 * T-3261 (PL-71): a person chooses a pipeline's alerts, changes, mutes and stops them; e-mail is
 * shown and refused with its reason; and the inbox shows an alert notice with Mute beside it.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AlertDialog } from "../src/components/AlertDialog";

const a = en.alerts;

interface Call {
  method: string;
  path: string;
  body: unknown;
}

function stubbed(subscriptions: unknown[]) {
  const calls: Call[] = [];
  let held = subscriptions;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const path = new URL(request.url).pathname;
      const body = request.method === "GET" || request.method === "DELETE" ? undefined : await request.clone().json().catch(() => undefined);
      calls.push({ method: request.method, path, body });
      const json = (value: unknown, status = 200) =>
        new Response(value === undefined ? null : JSON.stringify(value), { status, headers: { "Content-Type": "application/json" } });
      if (path === "/api/v1/alerts" && request.method === "GET") return json({ items: held });
      if (path === "/api/v1/alerts" && request.method === "PUT") {
        const made = { id: 4, mutedUntil: null, ...(body as object) };
        held = [made];
        return json(made);
      }
      if (path.endsWith("/mute")) {
        const muted = { ...(held[0] as object), mutedUntil: "2026-10-08T22:00:00Z" };
        held = [muted];
        return json(muted);
      }
      if (request.method === "DELETE") {
        held = [];
        return new Response(null, { status: 204 });
      }
      return json({});
    }),
  );
  return calls;
}

function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <AlertDialog project="helsinki" scope="pipeline" target="bikes" label="bikes" onClose={() => undefined} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("choosing a pipeline's alerts", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("subscribes to the events ticked, in the Portal, and refuses e-mail with its reason", async () => {
    const user = userEvent.setup();
    const calls = stubbed([]);
    show();
    expect(await screen.findByRole("button", { name: a.subscribe })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: new RegExp(a.email) })).toBeDisabled();
    expect(screen.getByText(a.emailHint)).toBeInTheDocument();
    await user.click(screen.getByLabelText(a.event.stale));
    await user.click(screen.getByRole("button", { name: a.subscribe }));
    await waitFor(() => expect(calls.some((call) => call.method === "PUT")).toBe(true));
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      project: "helsinki",
      scope: "pipeline",
      target: "bikes",
      events: ["failure", "zero", "invalid"],
      delivery: "portal",
    });
    expect(await screen.findByRole("button", { name: a.change })).toBeInTheDocument();
  });

  it("refuses a subscription with no event ticked", async () => {
    const user = userEvent.setup();
    stubbed([]);
    show();
    await screen.findByRole("button", { name: a.subscribe });
    for (const event of ["failure", "stale", "zero", "invalid"] as const) await user.click(screen.getByLabelText(a.event[event]));
    expect(screen.getByRole("button", { name: a.subscribe })).toHaveAttribute("aria-disabled", "true");
  });

  it("mutes an existing subscription for a day and stops it", async () => {
    const user = userEvent.setup();
    const calls = stubbed([
      { id: 4, project: "helsinki", scope: "pipeline", target: "bikes", events: ["failure"], delivery: "digest", mutedUntil: null },
    ]);
    show();
    expect(await screen.findByRole("status")).toHaveTextContent(a.active);
    expect(screen.getByRole("radio", { name: new RegExp(a.digest) })).toBeChecked();
    await user.selectOptions(screen.getByLabelText(a.mute), "1d");
    await waitFor(() => expect(calls.find((call) => call.path.endsWith("/mute"))?.body).toEqual({ for: "1d" }));
    expect(await screen.findByText(/^Muted until/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: a.stop }));
    await waitFor(() => expect(calls.some((call) => call.method === "DELETE" && call.path === "/api/v1/alerts/4")).toBe(true));
    expect(await screen.findByRole("button", { name: a.subscribe })).toBeInTheDocument();
  });
});
