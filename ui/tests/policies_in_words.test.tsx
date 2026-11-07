/**
 * T-3276: the Policies page reads each stored policy as a sentence under its name.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const POLICY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name: "air-team-reads", namespace: "helsinki" },
  spec: {
    contextSpaceRef: "ovzdusie",
    assigner: "did:web:hel.fi",
    assignee: { kind: "group", id: "air-team" },
    operations: ["retrieveOps"],
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  },
  status: { phase: "Live" },
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  window.history.pushState({}, "", "/projects/helsinki/policies");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "demo.steward", roles: ["portal-admin"] });
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
      if (url.pathname === "/api/v1/projects/helsinki/policies") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [POLICY] });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the Policies page in words (T-3276)", () => {
  it("reads a stored policy as the sentence it grants", async () => {
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(
      await screen.findByText("Members of the group air-team may read on AirQualityObserved in the space ovzdusie."),
    ).toBeInTheDocument();
  });
});
