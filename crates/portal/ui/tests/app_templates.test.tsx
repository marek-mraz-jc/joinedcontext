// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/apps/AppTemplates.tsx.
/**
 * T-3263 (AP-141): the App templates a person starts from, each with what it is for, for whom
 * and the data it needs, and "create from this", which opens the builder with the template's
 * purpose written and named, so only the endpoint to read is left to choose.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AppTemplates, templatePrompt, typesOf } from "../src/pages/apps/AppTemplates";
import { BLUEPRINT } from "../src/pages/apps/AppGenerator";
import { InRouter } from "./pageHarness";

const KPI = {
  name: "kpi-dashboard",
  title: "KPI dashboard",
  purpose: "Show managers how the organization's indicators stand.",
  audience: "managers",
  access: "Read-only; the indicators may be public.",
  dataNeeds: [{ types: ["KeyPerformanceIndicator"], attrs: ["kpiValue"], operations: ["queryEntity"] }],
};

function show(mayBuild: boolean, templates: unknown[] = [KPI]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
      if (url.pathname === "/api/v1/app-templates") return json({ templates });
      if (url.pathname === "/api/v1/blueprints") {
        return json({ items: [{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Blueprint", metadata: { name: BLUEPRINT }, spec: {} }] });
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <AppTemplates project="helsinki" mayBuild={mayBuild} />
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

describe("the App templates (T-3263)", () => {
  it("names the template in the prompt it starts with, and reads its types once each", () => {
    expect(templatePrompt(KPI)).toBe("Show managers how the organization's indicators stand. (template: kpi-dashboard)");
    expect(typesOf({ dataNeeds: [{ types: ["A", "B"] }, { types: ["A"] }, { types: "C" }, {}] })).toEqual(["A", "B"]);
  });

  it("shows what each template is for and needs, and opens the builder with its purpose written", async () => {
    show(true);
    await userEvent.click(await screen.findByText(en.apps.templates.title));
    const card = screen.getByRole("heading", { name: "KPI dashboard" }).closest("li") as HTMLElement;
    expect(card).toHaveTextContent("Show managers how the organization's indicators stand.");
    expect(within(card).getByText("KeyPerformanceIndicator")).toBeInTheDocument();
    // T-3306: its picture and its demo, a page of fixtures that holds no data.
    expect(within(card).getByRole("img", { name: "KPI dashboard as its demo shows it" })).toHaveAttribute(
      "src",
      "/api/v1/app-templates/kpi-dashboard/screenshot/1440",
    );
    expect(within(card).getByRole("link", { name: new RegExp(en.apps.templates.demo) })).toHaveAttribute("href", "/templates/kpi-dashboard/");
    await userEvent.click(within(card).getByRole("button", { name: en.apps.templates.create }));
    const dialog = await screen.findByRole("dialog", { name: "Create from KPI dashboard" });
    expect(within(dialog).getByLabelText(new RegExp(en.apps.generate.prompt))).toHaveValue(templatePrompt(KPI));
  });

  it("offers no build to a person who may not propose an app, and nothing when there is no template", async () => {
    show(false);
    await userEvent.click(await screen.findByText(en.apps.templates.title));
    expect(screen.getByRole("button", { name: en.apps.templates.create })).toHaveAttribute("aria-disabled", "true");
  });
});
