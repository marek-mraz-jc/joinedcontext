// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/apps/UseTemplate.tsx.
/**
 * T-5617: a template used as it ships, without the model. While the model key is out the gallery
 * opens, says why "Create from this" is unavailable and offers "Use as is", whose dialog asks only
 * the App's name and an Endpoint the person reads, sends both to the template route and shows the
 * Change; a refusal the server names a field for lands under that field.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApiError } from "../src/api/client";
import { AppTemplates } from "../src/pages/apps/AppTemplates";
import { fieldErrors } from "../src/pages/apps/UseTemplate";
import { InRouter } from "./pageHarness";

const MAP = {
  name: "citizen-map",
  title: "Citizen map app",
  purpose: "Help a resident find something near them on a map.",
  audience: "citizens",
  access: "Read-only.",
  dataNeeds: [{ types: ["BikeHireDockingStation"], attrs: ["name"], operations: ["queryEntity"] }],
};

const BIKES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "bikes", namespace: "sharing", title: { en: "City bikes" } },
  spec: { slug: "bikes", contextSpaceRef: "mobility" },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "change-12", namespace: "sharing" },
  spec: { title: "create App bike-finder", lane: "red", operation: "create", files: [] },
  status: { phase: "Open", lane: "red" },
};

type Answer = { status: number; body: unknown };

function show(options: { modelAvailable: boolean; endpoints?: unknown[]; answer?: Answer }) {
  const sent: { url: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
        });
      if (url.pathname === "/api/v1/app-templates") return json({ templates: [MAP], modelAvailable: options.modelAvailable });
      if (url.pathname === "/api/v1/projects/sharing/endpoints") {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: options.endpoints ?? [BIKES] });
      }
      if (request.method === "POST") {
        sent.push({ url: url.pathname, body: await request.json() });
        const answer = options.answer ?? { status: 202, body: CHANGE };
        return json(answer.body, answer.status);
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <AppTemplates project="sharing" mayBuild />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return sent;
}

async function openUse() {
  await userEvent.click(await screen.findByRole("button", { name: en.apps.templates.useAsIs }));
  return screen.findByRole("dialog", { name: "Use Citizen map app as is" });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a template used as it ships (T-5617)", () => {
  it("says why Generate is unavailable while the key is out and keeps Use as is open", async () => {
    show({ modelAvailable: false });
    expect(await screen.findByText(en.apps.templates.modelOut)).toBeInTheDocument();
    expect(screen.getByTestId("app-templates")).toHaveAttribute("open");
    expect(screen.getByRole("button", { name: en.apps.templates.create })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: en.apps.templates.useAsIs })).not.toHaveAttribute("aria-disabled");
  });

  it("leaves Generate as it was while the key works", async () => {
    show({ modelAvailable: true });
    await userEvent.click(await screen.findByText(en.apps.templates.title));
    expect(screen.queryByText(en.apps.templates.modelOut)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.apps.templates.create })).not.toHaveAttribute("aria-disabled");
  });

  it("creates the App from the name and a readable Endpoint and shows the Change", async () => {
    const sent = show({ modelAvailable: false });
    const dialog = await openUse();
    expect(within(dialog).getByText(en.apps.templates.nameHint)).toBeInTheDocument();
    expect(within(dialog).getByText(en.apps.templates.endpointHint)).toBeInTheDocument();
    const submit = within(dialog).getByRole("button", { name: en.apps.templates.useSubmit });
    expect(submit).toHaveAttribute("aria-disabled", "true");
    const name = within(dialog).getByLabelText(new RegExp(en.apps.templates.name));
    await userEvent.clear(name);
    await userEvent.type(name, "bike-finder");
    await userEvent.selectOptions(await within(dialog).findByLabelText(new RegExp(en.apps.generate.endpoint)), "bikes");
    expect(within(dialog).getByRole("option", { name: "City bikes" })).toBeInTheDocument();
    await userEvent.click(submit);
    expect(await within(dialog).findByText(/change-12|#12/)).toBeInTheDocument();
    expect(sent).toEqual([
      { url: "/api/v1/projects/sharing/app-templates/citizen-map/apps", body: { name: "bike-finder", endpointName: "bikes" } },
    ]);
  });

  it("puts a refusal the server names a field for under that field", async () => {
    show({
      modelAvailable: false,
      answer: {
        status: 422,
        body: {
          type: "https://joinedcontext.com/problems/unprocessable",
          title: "Unprocessable Content",
          status: 422,
          detail: "endpointName: 'bikes' is not an Endpoint of 'sharing' you may read; choose one from the list",
          errors: ["endpointName: 'bikes' is not an Endpoint of 'sharing' you may read; choose one from the list"],
        },
      },
    });
    const dialog = await openUse();
    await userEvent.selectOptions(await within(dialog).findByLabelText(new RegExp(en.apps.generate.endpoint)), "bikes");
    await userEvent.click(within(dialog).getByRole("button", { name: en.apps.templates.useSubmit }));
    const select = within(dialog).getByLabelText(new RegExp(en.apps.generate.endpoint));
    expect(await within(dialog).findByText(/is not an Endpoint of 'sharing' you may read/)).toBeInTheDocument();
    expect(select).toHaveAttribute("aria-invalid", "true");
  });

  it("says what to do when the person reads no Endpoint", async () => {
    show({ modelAvailable: false, endpoints: [] });
    const dialog = await openUse();
    expect(await within(dialog).findByText(en.apps.templates.noEndpoints)).toBeInTheDocument();
  });

  it("reads field refusals by their prefix only", () => {
    const error = new ApiError(422, "x", { errors: ["name: taken", "endpointName: hidden", 3] } as never);
    expect(fieldErrors(error, "name")).toEqual(["taken"]);
    expect(fieldErrors(error, "endpointName")).toEqual(["hidden"]);
    expect(fieldErrors(new Error("x"), "name")).toEqual([]);
  });
});
