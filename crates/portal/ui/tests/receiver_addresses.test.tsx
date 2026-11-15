/**
 * A receiver's or a broker's address is shown as host and path (T-4991): the password and the
 * query string an address may carry never reach a table cell or a detail row.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const IDENTITY = { subject: "s1", username: "steward", email: "demo.steward@hel.fi", roles: ["portal-editor"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const ADDRESS = "https://hook-user:hunter2@hook.example.org/notify?token=s3cr3t";
const SUBSCRIPTION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Subscription",
  metadata: { name: "air-alerts", namespace: "helsinki" },
  spec: { contextSpaceRef: "hub", entities: [{ type: "AirQualityObserved" }], notification: { endpoint: { uri: ADDRESS } } },
};
const REGISTRATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSourceRegistration",
  metadata: { name: "remote-broker", namespace: "helsinki" },
  spec: { contextSpaceRef: "hub", endpoint: ADDRESS, information: [{ entities: [{ type: "KeyPerformanceIndicator" }] }] },
};

function renderAt(path: string) {
  vi.stubGlobal("fetch", (input: RequestInfo | URL) => {
    const url = new URL((input as Request).url);
    const json = (body: unknown) => Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }));
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/subscriptions")) return json(list([SUBSCRIPTION]));
    if (url.pathname.endsWith("/csrs")) return json(list([REGISTRATION]));
    if (url.pathname.endsWith("/csrs/remote-broker")) return json(REGISTRATION);
    return json(list([]));
  });
  window.history.pushState({}, "", path);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("addresses with credentials (T-4991)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  for (const path of ["/projects/helsinki/subscriptions", "/projects/helsinki/csrs", "/projects/helsinki/csrs/remote-broker"]) {
    it(`${path} shows the host and path, never the password or the token`, async () => {
      const { container } = renderAt(path);
      expect((await screen.findAllByText("hook.example.org/notify")).length).toBeGreaterThan(0);
      expect(container.innerHTML).not.toContain("hunter2");
      expect(container.innerHTML).not.toContain("s3cr3t");
    });
  }
});
