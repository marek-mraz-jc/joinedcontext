import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { SaveAsTemplate, TemplateList, formOfTemplate } from "../src/pages/pipelines/PipelineTemplates";

// T-3976: a project's own templates in the gallery, saved from the editor; using one fills the
// form with the shape and leaves the source and the target to pick.
const perms = vi.hoisted(() => ({ propose: true }));
vi.mock("../src/api/permissions", async (original) => ({
  ...(await original<typeof import("../src/api/permissions")>()),
  usePermissions: () => ({ can: (_kind: string, verb: string) => verb !== "propose" || perms.propose }),
}));

const SPEC = {
  class: "resident",
  sources: [{ query: { type: "AirQualityObserved" } }],
  steps: [{ kind: "bloblang", bloblang: "root.id = this.id" }],
  outputs: [{ type: "AirQualityObserved", mode: "upsert" }],
};

const sent: { method: string; path: string; body: unknown }[] = [];
function stub(items: unknown[]) {
  sent.length = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const path = new URL(request.url).pathname;
      const text = request.method === "GET" ? "" : await request.clone().text();
      sent.push({ method: request.method, path, body: text ? JSON.parse(text) : undefined });
      if (request.method === "POST") {
        const body = JSON.parse(text) as { title: string };
        return new Response(JSON.stringify({ id: "t-new", title: body.title, spec: {}, createdBy: "Jana", createdAt: "2026-10-12T09:00:00Z" }), { status: 201 });
      }
      if (request.method === "DELETE") return new Response(null, { status: 204 });
      return new Response(JSON.stringify({ items }), { status: 200 });
    }),
  );
}

function wrap(node: ReactNode) {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

const TEMPLATE = { id: "3c0f6f0e-8a1b-4c2d-9e4f-5a6b7c8d9e0f", title: "MQTT sensor feed", spec: SPEC, createdBy: "Jana", createdAt: "2026-10-12T09:00:00Z" };

describe("pipeline templates of a project", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    perms.propose = true;
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads a template into a form with no name, its mapping kept and the source and target to pick", () => {
    const form = formOfTemplate(SPEC);
    expect(form.name).toBeUndefined();
    expect(form.class).toBe("resident");
    expect(form.targetEndpoint).toBeUndefined();
    expect(form.source?.dataSourceRef).toBeUndefined();
    expect(form.output).toEqual({ type: "AirQualityObserved", mode: "upsert" });
    expect(JSON.stringify(form)).toContain("root.id = this.id");
  });

  it("lists the project's templates and fills the form from one", async () => {
    stub([TEMPLATE]);
    const onUse = vi.fn();
    wrap(<TemplateList project="helsinki" onUse={onUse} />);
    expect(await screen.findByText("MQTT sensor feed")).toBeInTheDocument();
    expect(sent[0].path).toBe("/api/v1/projects/helsinki/pipelinetemplates");
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.templates.use }));
    expect(onUse).toHaveBeenCalledWith(expect.objectContaining({ class: "resident", output: { type: "AirQualityObserved", mode: "upsert" } }));

    await userEvent.click(screen.getByRole("button", { name: "Delete the template MQTT sensor feed" }));
    await waitFor(() => expect(sent.some((s) => s.method === "DELETE" && s.path.endsWith(`/pipelinetemplates/${TEMPLATE.id}`))).toBe(true));
  });

  it("shows nothing when the project saved none, and a viewer no delete", async () => {
    stub([]);
    wrap(<TemplateList project="helsinki" onUse={vi.fn()} />);
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(screen.queryByTestId("pipeline-templates")).not.toBeInTheDocument();

    perms.propose = false;
    stub([TEMPLATE]);
    wrap(<TemplateList project="espoo" onUse={vi.fn()} />);
    expect(await screen.findByText("MQTT sensor feed")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Delete the template/ })).not.toBeInTheDocument();
  });

  it("saves the editor's manifest under a title, and asks for one first", async () => {
    stub([]);
    const manifest = { apiVersion: "joinedcontext.com/v1alpha2", kind: "Pipeline", metadata: { name: "air" }, spec: SPEC } as Manifest;
    wrap(<SaveAsTemplate project="helsinki" manifest={() => manifest} />);
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.templates.save }));
    const save = screen.getByRole("button", { name: en.pipelines.templates.saveIt });
    expect(save).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(screen.getByLabelText(new RegExp(en.pipelines.templates.titleField)), "  Air feed ");
    await userEvent.click(save);
    expect(await screen.findByRole("status")).toHaveTextContent('Saved as template "Air feed"');
    expect(sent.find((s) => s.method === "POST")?.body).toEqual({ title: "Air feed", manifest });
  });

  it("offers a viewer no save", () => {
    perms.propose = false;
    stub([]);
    wrap(<SaveAsTemplate project="helsinki" manifest={() => ({}) as Manifest} />);
    expect(screen.queryByTestId("save-as-template")).not.toBeInTheDocument();
  });
});
