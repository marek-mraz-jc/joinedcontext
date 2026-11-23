/**
 * AddFieldDialog branches the first suite does not reach: the own-field form per type, the cancel
 * button, each refusal of a plain and of a formula field (the model, the pipeline, the missing
 * Endpoint or account) and a lost connection.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AddFieldDialog } from "../src/components/entities/AddFieldDialog";
import { parseModel } from "../src/pages/models/linkml";

const STATIONS = `id: https://hel.fi/models/stations
name: stations
default_prefix: hel
prefixes:
  hel: https://hel.fi/models/
classes:
  BikeHireDockingStation:
    slots: [name]
  Street:
    slots: [label]
slots:
  name:
    range: string
  label:
    range: string
`;
const TYPE = "BikeHireDockingStation";

function respond(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

const ENDPOINTS = [{ apiVersion: "x", kind: "Endpoint", metadata: { name: "bikes-all" }, spec: { slug: "b" } }];
const CHANGE = { metadata: { name: "chg-1" }, status: { phase: "Proposed" } };

describe("AddFieldDialog branches", () => {
  let originalFetch: typeof global.fetch;
  let sent: { method: string; path: string; body: string }[];

  beforeEach(async () => {
    originalFetch = global.fetch;
    sent = [];
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    global.fetch = originalFetch;
  });

  function mount(answer: (path: string, method: string) => Response | Promise<Response>, extra: { endpoints?: unknown[]; space?: string; type?: string; onProposed?: () => void } = {}) {
    global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const url = new URL(request.url);
      sent.push({ method: request.method, path: url.pathname + url.search, body: request.method === "GET" ? "" : await request.clone().text() });
      if (url.pathname.includes("/tools/")) return respond({ title: "Bad Gateway" }, 502);
      return answer(url.pathname, request.method);
    }) as typeof fetch;
    const onProposed = vi.fn(extra.onProposed);
    const onOpenChange = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <AddFieldDialog
            project="hel"
            modelName="stations"
            source={STATIONS}
            type={extra.type ?? TYPE}
            open
            onOpenChange={onOpenChange}
            onProposed={onProposed}
            space={extra.space}
            endpoints={extra.endpoints as never[] | undefined}
            orgDomain="hel.fi"
          />
        </QueryClientProvider>
      </I18nextProvider>,
    );
    return { onProposed, onOpenChange };
  }

  const nameField = () => within(screen.getByRole("dialog")).getByLabelText(new RegExp(en.spaces.fields.name));
  const typeField = () => within(screen.getByRole("dialog")).getByLabelText(en.spaces.fields.type);
  const propose = () => userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: en.spaces.fields.propose }));

  it("closes on Cancel and tells the parent", async () => {
    const { onOpenChange } = mount(() => respond({}, 500));
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: en.app.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("proposes a choice field with the values typed one per line, and a required one", async () => {
    const { onProposed } = mount(() => respond(CHANGE, 202));
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await userEvent.type(nameField(), "state");
    await userEvent.selectOptions(typeField(), "select");
    await userEvent.type(within(screen.getByRole("dialog")).getByLabelText(new RegExp(en.spaces.fields.values)), "open{enter}  closed  {enter}{enter}");
    await userEvent.click(within(screen.getByRole("dialog")).getByLabelText(en.spaces.fields.required));
    await propose();

    await waitFor(() => expect(onProposed).toHaveBeenCalledTimes(1));
    const put = sent.find((r) => r.method === "PUT")!;
    expect(put.path).toBe("/api/v1/projects/hel/datamodels/stations/source");
    const after = parseModel(put.body);
    const slot = after.slots.find((s) => s.name === "state")!;
    expect(slot.required).toBe(true);
    expect(after.enums.find((e) => e.name === slot.range)?.permissible_values.map((v) => v.name)).toEqual(["open", "closed"]);
  });

  it("proposes a relationship with its target, its way back and the many flag", async () => {
    const { onProposed } = mount(() => respond(CHANGE, 202));
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    const dialog = screen.getByRole("dialog");
    await userEvent.type(nameField(), "street");
    await userEvent.selectOptions(typeField(), "relationship");
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: /^Points to/ }), "Street");
    await userEvent.type(within(dialog).getByLabelText(new RegExp(en.spaces.fields.inverse)), "stations");
    await userEvent.click(within(dialog).getByLabelText(en.spaces.fields.many));
    await propose();

    await waitFor(() => expect(onProposed).toHaveBeenCalledTimes(1));
    const after = parseModel(sent.find((r) => r.method === "PUT")!.body);
    expect(after.slots.find((s) => s.name === "street")).toMatchObject({ range: "Street", kind: "Relationship", multivalued: true });
    expect(after.slots.find((s) => s.name === "stations")?.multivalued).toBe(true);
  });

  it("shows the reason when the model refuses the slot, and sends nothing", async () => {
    // The type is no class of the model, so the editor's operation is refused before a request.
    const { onProposed } = mount(() => respond({}, 500), { type: "Ghost" });
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await userEvent.type(nameField(), "capacity");
    await propose();
    expect(await screen.findByText("unknown class 'Ghost'")).toBeInTheDocument();
    expect(sent.some((r) => r.method === "PUT")).toBe(false);
    expect(onProposed).not.toHaveBeenCalled();
  });

  it("shows the Portal's message when the connection is lost on a plain field", async () => {
    mount(() => {
      throw new TypeError("network down");
    });
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await userEvent.type(nameField(), "capacity");
    await propose();
    expect(await screen.findByText("network down")).toBeInTheDocument();
  });

  it("shows the error of the parent when it cannot take the proposed Change", async () => {
    mount(() => respond(CHANGE, 202), {
      onProposed: () => {
        throw new Error("the change list is closed");
      },
    });
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await userEvent.type(nameField(), "capacity");
    await propose();
    expect(await screen.findByText("the change list is closed")).toBeInTheDocument();
  });

  async function fillFormula(name: string, expression: string) {
    await userEvent.type(nameField(), name);
    await userEvent.selectOptions(typeField(), "formula");
    await userEvent.type(within(screen.getByRole("dialog")).getByLabelText(new RegExp(en.spaces.fields.formula.expression)), expression.replace(/[{[]/g, "$&$&"));
    await propose();
  }

  it("says a formula needs an Endpoint of the space when there is none, and sends nothing", async () => {
    mount(() => respond({}, 500), { space: "bikes", endpoints: [] });
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await fillFormula("title", "{name} + ' station'");
    expect(await screen.findByText(en.spaces.fields.formula.noEndpoint)).toBeInTheDocument();
    expect(sent.filter((r) => !r.path.includes("/tools/"))).toEqual([]);
  });

  it("shows the server's message when the pipelines account cannot be read", async () => {
    mount((path) => (path.endsWith("/serviceaccounts/pipelines") ? respond({ title: "Server Error", detail: "store down" }, 500) : respond({}, 500)), {
      space: "bikes",
      endpoints: ENDPOINTS,
    });
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await fillFormula("title", "{name} + ' station'");
    expect(await screen.findByText("store down")).toBeInTheDocument();
    expect(sent.some((r) => r.method === "PUT")).toBe(false);
  });

  it("shows the reason when the model refuses the formula's slot", async () => {
    const { onProposed } = mount(() => respond({ metadata: { name: "pipelines" } }), { space: "bikes", endpoints: ENDPOINTS, type: "Ghost" });
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await fillFormula("title", "1 + 2");
    expect(await screen.findByText("unknown class 'Ghost'")).toBeInTheDocument();
    expect(sent.some((r) => r.method === "PUT")).toBe(false);
    expect(onProposed).not.toHaveBeenCalled();
  });

  it("shows the Portal's refusal of the formula's model Change, and proposes no pipeline", async () => {
    const { onProposed } = mount(
      (path, method) =>
        method === "PUT" ? respond({ title: "Conflict", detail: "the model changed since it was read" }, 409) : respond({ metadata: { name: "pipelines" } }),
      { space: "bikes", endpoints: ENDPOINTS },
    );
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await fillFormula("title", "{name} + ' station'");
    expect(await screen.findByText("the model changed since it was read")).toBeInTheDocument();
    expect(sent.some((r) => r.path.startsWith("/api/v1/projects/hel/import"))).toBe(false);
    expect(onProposed).not.toHaveBeenCalled();
  });

  it("falls back to the status line when the refusal names no detail", async () => {
    mount(
      (path, method) => (method === "PUT" ? respond({}, 403) : respond({ metadata: { name: "pipelines" } })),
      { space: "bikes", endpoints: ENDPOINTS },
    );
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await fillFormula("title", "{name} + ' station'");
    expect(await screen.findByRole("alert")).toHaveTextContent(/403|Forbidden|refused/i);
  });

  it("keeps the model Change and says why the pipeline was refused", async () => {
    const { onProposed } = mount(
      (path, method) => {
        if (path.endsWith("/serviceaccounts/pipelines")) return respond({ metadata: { name: "pipelines" } });
        if (method === "PUT") return respond({ metadata: { name: "chg-model" }, status: { phase: "PendingApproval" } }, 202);
        if (path === "/api/v1/projects('hel')") return respond({ "@odata.etag": 'W/"hel"', name: "hel" });
        return respond({ title: "Unprocessable", detail: "pipeline refused by the check" }, 422);
      },
      { space: "bikes", endpoints: ENDPOINTS },
    );
    await screen.findByText(en.spaces.fields.sdmUnavailable);
    await fillFormula("title", "{name} + ' station'");
    await waitFor(() => expect(onProposed).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("alert")).toHaveTextContent(/pipeline refused by the check/);
  });
});
