// T-7706: WebhookPanel: when the clipboard refuses, the address is selected for Ctrl+C.
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ConnectionTest } from "../src/pages/datasources/ConnectionTest";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const json = (body: unknown, status = 200) =>
  Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
const INFO = {
  auth: "hmac",
  header: "X-Hub-Signature-256",
  maxBodyBytes: 1048576,
  published: true,
  addresses: [
    { pipeline: "ttn-uplinks", url: "https://portal.city.test/ingest/default/helsinki.ttn-uplinks/ttn" },
    { pipeline: "ttn-archive", url: "https://portal.city.test/ingest/default/helsinki.ttn-archive/ttn" },
  ],
};

function show(info: unknown) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    if (path.endsWith("/webhook")) return json(info);
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ConnectionTest project="helsinki" name="ttn" type="webhook" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("copying a webhook address", () => {
  it("selects the address when the clipboard refuses, and keeps saying Copy", async () => {
    await i18n.changeLanguage("en");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn(async () => Promise.reject(new Error("denied"))) },
    });
    show(INFO);
    const panel = await screen.findByTestId("datasource-webhook");
    const first = (await within(panel).findAllByRole("button", { name: en.datasources.webhook.copy }))[0];
    await userEvent.click(first);
    expect(window.getSelection()?.toString()).toContain("portal.city.test/ingest/default/helsinki.ttn-uplinks/ttn");
    expect(within(panel).queryByText(en.datasources.webhook.copied)).toBeNull();
  });
});
