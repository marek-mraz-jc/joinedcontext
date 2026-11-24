// T-7706: ShareFlow edges: a preview read the endpoint refuses, a type unticked and ticked again, the propose button pressed with nothing chosen.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { ShareFlow } from "../src/pages/endpoints/ShareFlow";
import { InRouter } from "./pageHarness";

const ENDPOINT: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "air", namespace: "helsinki" },
  spec: { contextSpaceRef: "ovzdusie", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "project-list", allowedProjects: [] },
  status: { phase: "Live" },
} as Manifest;

let imports: string[];
let previewReads: number;

function show() {
  imports = [];
  previewReads = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }, { name: "bbsk" }] });
      if (url.pathname.endsWith("/import")) imports.push(url.search);
      if (url.pathname.includes("/ngsi-ld/v1/entities")) {
        previewReads += 1;
        return json({ title: "Forbidden", status: 403 }, 403);
      }
      return json({ items: [] });
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ShareFlow project="helsinki" endpoint={ENDPOINT} space="ovzdusie" types={["AirQualityObserved"]} slots={["pm10"]} policies={[]} />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the share flow's edges", () => {
  it("shows no sample when the endpoint refuses the preview read, and stops reading with no type chosen", async () => {
    const user = userEvent.setup();
    show();
    const select = await screen.findByRole("combobox", { name: en.endpoints.shareFlow.project });
    await waitFor(() => expect(screen.getByRole("option", { name: "bbsk" })).toBeInTheDocument());
    await user.selectOptions(select, "bbsk");
    await waitFor(() => expect(previewReads).toBe(1));

    const type = screen.getByRole("checkbox", { name: "AirQualityObserved" });
    await user.click(type);
    expect(type).not.toBeChecked();
    const propose = screen.getByRole("button", { name: en.endpoints.shareFlow.propose });
    expect(propose).toHaveAttribute("aria-disabled", "true");
    await user.click(propose);
    expect(imports).toEqual([]);

    await user.click(type);
    expect(type).toBeChecked();
  });
});
