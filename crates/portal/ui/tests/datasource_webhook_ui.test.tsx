/**
 * PL-122: a webhook DataSource's page shows where senders post and sends a test event through the
 * server, which signs it; the form offers the type with no tls block (form_help.test.ts holds
 * its hints).
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ConnectionTest } from "../src/pages/datasources/ConnectionTest";
import { WebhookPanel } from "../src/pages/datasources/WebhookPanel";
import { dataSourceSchema, DATA_SOURCE_TYPES } from "../src/schemas/kinds";

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

function show(info: unknown, testEvent: (body: unknown) => Promise<Response> = () => json({ status: 200, answer: "" })) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    if (path.endsWith("/webhook")) return json(info);
    if (path.endsWith("/test-event")) return testEvent(await request.clone().json());
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

describe("webhook DataSource page", () => {
  it("lists each reading pipeline's address and the header, and offers no connection test", async () => {
    show(INFO);
    const panel = await screen.findByTestId("datasource-webhook");
    expect(await within(panel).findByText(INFO.addresses[0].url)).toBeInTheDocument();
    expect(within(panel).getByText(INFO.addresses[1].url)).toBeInTheDocument();
    expect(within(panel).getByText(/Header: X-Hub-Signature-256/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Test connection" })).toBeNull();
    const choice = within(panel).getByLabelText(/Pipeline/);
    expect([...(choice as HTMLSelectElement).options].map((o) => o.value)).toEqual(["ttn-uplinks", "ttn-archive"]);
  });

  it("copies an address with its own button", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    show(INFO);
    const panel = await screen.findByTestId("datasource-webhook");
    await userEvent.click((await within(panel).findAllByRole("button", { name: "Copy address" }))[1]!);
    expect(writeText).toHaveBeenCalledWith(INFO.addresses[1].url);
    expect(await within(panel).findByRole("button", { name: "Address copied" })).toBeInTheDocument();
  });

  it("sends the typed body to the picked pipeline and shows the runner's 200", async () => {
    const sent: unknown[] = [];
    show(INFO, (body) => {
      sent.push(body);
      return json({ status: 200, answer: "" });
    });
    const panel = await screen.findByTestId("datasource-webhook");
    await userEvent.selectOptions(await within(panel).findByLabelText(/Pipeline/), "ttn-archive");
    await userEvent.type(within(panel).getByLabelText(/Body/), '{{"temperature": 21.5}');
    await userEvent.click(within(panel).getByRole("button", { name: "Send test event" }));
    expect(await within(panel).findByText(/Accepted \(200\)/)).toBeInTheDocument();
    expect(sent).toEqual([{ pipeline: "ttn-archive", body: '{"temperature": 21.5}' }]);
  });

  it("shows the runner's refusal and the server's own reason", async () => {
    show(INFO, () => json({ status: 401, answer: "Unauthorized" }));
    const panel = await screen.findByTestId("datasource-webhook");
    await userEvent.type(await within(panel).findByLabelText(/Body/), "x");
    await userEvent.click(within(panel).getByRole("button", { name: "Send test event" }));
    expect(await within(panel).findByText(/answered 401: Unauthorized/)).toBeInTheDocument();
    expect(within(panel).getByText(/check both, then send again/)).toBeInTheDocument();
  });

  it("names a refused field from the server's problem", async () => {
    show(INFO, () =>
      json({ type: "about:blank", title: "Unprocessable", status: 422, detail: "pipeline: no pipeline of project helsinki reads DataSource ttn under that name; pick one of the list", errors: ["pipeline"] }, 422),
    );
    const panel = await screen.findByTestId("datasource-webhook");
    await userEvent.type(await within(panel).findByLabelText(/Body/), "x");
    await userEvent.click(within(panel).getByRole("button", { name: "Send test event" }));
    expect(await within(panel).findByText(/pick one of the list/)).toBeInTheDocument();
  });

  it("says what to do when no pipeline reads the source, or nothing is published", async () => {
    show({ ...INFO, addresses: [] });
    expect(await screen.findByText(/No pipeline reads this source yet, so it has no address/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Send test event" })).toBeNull();
  });

  it("renders alone, as the DataSource dialog embeds it", async () => {
    vi.stubGlobal("fetch", vi.fn(() => json(INFO)));
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <WebhookPanel project="helsinki" name="ttn" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByRole("heading", { name: "Where senders post" })).toBeInTheDocument();
  });

  it("says when the installation publishes no ingest address", async () => {
    show({ ...INFO, published: false, addresses: [{ pipeline: "ttn-uplinks", url: null }] });
    expect(await screen.findByText(/publishes no address for webhooks/)).toBeInTheDocument();
  });
});

describe("webhook DataSource form", () => {
  const t = (key: string) => i18n.t(key);
  const schema = dataSourceSchema(t, "webhook") as {
    properties: Record<string, { properties?: Record<string, { description?: string; oneOf?: unknown[] }> }>;
  };

  it("is offered as a typed source with one credential choice and no tls", () => {
    expect(DATA_SOURCE_TYPES).toContain("webhook");
    expect(Object.keys(schema.properties)).toEqual(["name", "title", "webhook"]);
    const webhook = schema.properties.webhook!.properties!;
    expect(webhook.auth!.oneOf).toHaveLength(2);
  });
});
