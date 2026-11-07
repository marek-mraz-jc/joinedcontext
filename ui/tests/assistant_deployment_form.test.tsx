/**
 * T-4282 (T-4281 items 8, 9): an AssistantDeployment's sources are picked from every project the
 * person reads, another project's as `{project}/{source}` with that project put in scope; a public
 * channel takes public sources alone; the form is laid out in the capabilities a steward decides.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { getDefaultFormState, type RJSFSchema } from "@rjsf/utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import validator from "../src/components/forms/validator";
import { portalFields } from "../src/components/forms/widgets";
import { KnowledgeSourcesField } from "../src/components/forms/widgets/KnowledgeSourcesField";
import { duplicateDeployment } from "../src/pages/knowledge/knowledge";
import { shippedForms } from "../src/schemas/forms";
import { PROJECT_REF, assistantDeploymentSchema, fromKindManifest, projectsNamed, toKindManifest } from "../src/schemas/knowledge";
import { jsonResponse, list, renderRoute } from "./pageHarness";

const source = (project: string, name: string, visibility: string, title?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "KnowledgeSource",
  metadata: { name, namespace: project, ...(title ? { title: { en: title } } : {}) },
  spec: { source: "website", startUrls: [`https://${project}.example/`], visibility },
});

const SOURCES: Record<string, unknown[]> = {
  helsinki: [source("helsinki", "web", "public"), source("helsinki", "intranet", "internal")],
  espoo: [source("espoo", "espoo-web", "public", "Espoo city pages")],
};

/** A folded group: a `<details>` whose summary starts with its title. */
function foldedGroup(title: string): HTMLDetailsElement | undefined {
  const summary = Array.from(document.querySelectorAll("details > summary")).find((node) => node.textContent?.startsWith(title));
  return (summary?.parentElement as HTMLDetailsElement | null) ?? undefined;
}

async function openForm() {
  const proposals: unknown[] = [];
  const rendered = await renderRoute({
    path: "/projects/helsinki/assistantdeployments/new?lang=en",
    projects: () => jsonResponse(list([{ name: "helsinki" }, { name: "espoo" }])),
    answer: (path, request) => {
      const sources = /^\/api\/v1\/projects\/([^/]+)\/knowledgesources$/.exec(path);
      if (sources) return jsonResponse(list(SOURCES[sources[1]] ?? []));
      if (request.method === "POST" && path === "/api/v1/projects/helsinki/assistantdeployments") {
        return request
          .clone()
          .json()
          .then((body: unknown) => {
            proposals.push(body);
            return jsonResponse({ valid: true, verdict: { ok: true, findings: [], checkedAt: "", inputDigest: "sha256:1" } });
          });
      }
      return undefined;
    },
  });
  const picker = await screen.findByTestId("knowledge-sources");
  await within(picker).findByRole("checkbox", { name: /Espoo city pages/ });
  return { ...rendered, picker, proposals };
}

describe("the deployment form's sources (T-4282)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("is the field a deployment's uiSchema names", () => {
    expect(portalFields.knowledgeSources).toBe(KnowledgeSourcesField);
  });

  it("offers the own project's sources by name and another readable project's under its name", async () => {
    const { picker } = await openForm();
    const own = within(picker).getByRole("group", { name: "Sources of helsinki" });
    expect(within(own).getByRole("checkbox", { name: /^web/ })).toBeEnabled();
    expect(within(own).getByRole("checkbox", { name: /intranet/ })).toBeEnabled();
    const other = within(picker).getByRole("group", { name: "Sources of espoo" });
    expect(within(other).getByRole("checkbox", { name: "Espoo city pages" })).toBeEnabled();
  });

  it("a public channel takes public sources alone: an internal one cannot be ticked", async () => {
    const { picker } = await openForm();
    await userEvent.selectOptions(screen.getByLabelText(/^Channel/), "Public");
    await waitFor(() => expect(within(picker).getByRole("checkbox", { name: /intranet/ })).toBeDisabled());
    expect(within(picker).getByRole("checkbox", { name: /^web/ })).toBeEnabled();
    const why = within(picker).getByText(/reads public sources only/);
    // The disabled box is described by the sentence that says why.
    expect(within(picker).getByRole("checkbox", { name: /intranet/ })).toHaveAttribute("aria-describedby", why.id);
  });

  it("counts what a public channel asks for: its sites, rate limit and budget", async () => {
    await openForm();
    const count = screen.getByTestId("required-count");
    const before = Number(/of (\d+)/.exec(count.textContent ?? "")?.[1]);
    await userEvent.selectOptions(screen.getByLabelText(/^Channel/), "Public");
    await waitFor(() => expect(Number(/of (\d+)/.exec(count.textContent ?? "")?.[1])).toBeGreaterThan(before));
  });

  it("a list of choices shows its help and names it to a screen reader (T-4824)", async () => {
    await openForm();
    const categories = screen.getAllByLabelText(/^Categories/)[0];
    const help = screen.getAllByText(/^The EU data themes it is about/)[0];
    expect(categories.getAttribute("aria-describedby")?.split(" ")).toContain(help.id);
  });

  it("lays the form out in the capabilities, live data apart from knowledge", async () => {
    await openForm();
    for (const group of ["Deployment", "Knowledge"]) {
      expect(screen.getAllByRole("group", { name: group }).length, group).toBeGreaterThan(0);
    }
    // The rest opens on a click: a new internal deployment needs a name, a channel and its sources (T-4824).
    for (const group of ["Live data", "How it answers", "Language", "Widget", "Where it may appear", "Limits and budget"]) {
      const folded = foldedGroup(group);
      expect(folded, group).toBeDefined();
      expect((folded as HTMLDetailsElement).open, group).toBe(false);
      // Named by its title, as an open set is by its legend (T-4924).
      expect(screen.getByRole("group", { name: group }), group).toBe(folded);
    }
    expect(screen.getByLabelText(/^Answers from/)).toHaveDisplayValue("Knowledge and live data");
  });

  // T-4343: the chat's behaviour is set in one group, Widget: the look, the language mode and its
  // toggle, the suggestions, what it answers from and the feedback buttons.
  it("the Widget group holds the language mode, its toggle and the chat's other settings", async () => {
    await openForm();
    const widget = foldedGroup("Widget") as HTMLElement;
    const mode = within(widget).getByLabelText(/^Language the chat opens in/);
    expect(mode).toHaveDisplayValue("The visitor's browser language");
    expect(within(mode as HTMLElement).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "The visitor's browser language",
      "Ask on the first open",
      "Always the first language",
    ]);
    await userEvent.selectOptions(mode, "Ask on the first open");
    expect(mode).toHaveDisplayValue("Ask on the first open");
    expect(within(widget).getByLabelText(/^Show the language switch/)).toBeChecked();
    // The greeting is an anyOf: its field, and the chooser of how it is given, named after it (T-4380).
    for (const field of [/^Answers from/, /^Ask visitors whether an answer helped/, /^Greeting(?!: given as)/, /^Greeting: given as/]) {
      expect(within(widget).getByLabelText(field)).toBeInTheDocument();
    }
  });

  it("ticking another project's source names it {project}/{source} and puts the project in scope", async () => {
    const { picker } = await openForm();
    await userEvent.click(within(picker).getByRole("checkbox", { name: /^web/ }));
    await userEvent.click(within(picker).getByRole("checkbox", { name: /Espoo city pages/ }));
    expect(within(picker).getByRole("checkbox", { name: /Espoo city pages/ })).toBeChecked();
    // What the form proposes is the pure function below; the form holds both references.
    expect(within(picker).getByRole("checkbox", { name: /^web/ })).toBeChecked();
  });

  it("a suggestion per language is added, keyed and taken away, and a site is named in words", async () => {
    await openForm();
    const suggestions = screen.getByRole("group", { name: /^Suggested questions/ });
    await userEvent.click(within(suggestions).getByRole("button", { name: "Add a language" }));
    const key = await within(suggestions).findByLabelText("Language");
    expect(key).toHaveValue("newKey");
    await userEvent.clear(key);
    await userEvent.type(key, "sk");
    await userEvent.tab();
    await waitFor(() => expect(within(suggestions).getByLabelText("Language")).toHaveValue("sk"));
    await userEvent.click(within(suggestions).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(within(suggestions).queryByLabelText("Language")).toBeNull());

    const sites = screen.getByRole("group", { name: /^Allowed sites/ });
    await userEvent.click(within(sites).getByRole("button", { name: "Add" }));
    expect(await within(sites).findByLabelText(/^Allowed sites 1/)).toBeInTheDocument();
    expect(within(sites).queryByText(/sites-1/)).toBeNull();
  });
});

describe("the deployment manifest (T-4282)", () => {
  const t = (key: string) => key;

  it("a new deployment's budget is whole: the defaults the form fills pass jc-core's budget rule (T-4824)", () => {
    const defaults = getDefaultFormState(validator, assistantDeploymentSchema(t) as RJSFSchema, {}) as {
      budget: { tokensPerDay: number; tokensPerConversation: number };
    };
    expect(defaults.budget.tokensPerDay).toBe(2_000_000);
    expect(defaults.budget.tokensPerConversation).toBeGreaterThan(0);
    expect(defaults.budget.tokensPerConversation).toBeLessThanOrEqual(defaults.budget.tokensPerDay);
  });

  it("a new deployment's rate limit is whole: jc-core needs both per-minute limits the form fills it without (T-4869)", () => {
    const defaults = getDefaultFormState(validator, assistantDeploymentSchema(t) as RJSFSchema, {}) as {
      rateLimit: { requestsPerMinute: number; perClientPerMinute: number };
    };
    expect(defaults.rateLimit.requestsPerMinute).toBeGreaterThan(0);
    expect(defaults.rateLimit.perClientPerMinute).toBeGreaterThan(0);
    expect(defaults.rateLimit.perClientPerMinute).toBeLessThanOrEqual(defaults.rateLimit.requestsPerMinute);
  });

  it("widget.language and widget.languageToggle are saved and read back as the form shows them (T-4343)", () => {
    for (const widget of [{ language: "ask", languageToggle: false }, { language: "fixed", languageToggle: true }, { language: "browser" }]) {
      const manifest = toKindManifest("AssistantDeployment", "bbsk", { name: "kraj", publicId: "kraj", sources: ["web"], widget });
      expect((manifest as unknown as { spec: { widget: unknown } }).spec.widget).toEqual(widget);
      expect(fromKindManifest(manifest).widget).toEqual(widget);
    }
    const schema = assistantDeploymentSchema(t) as unknown as { properties: { widget: { properties: { language: { oneOf: { const: string }[]; default: string } } } } };
    expect(schema.properties.widget.properties.language.oneOf.map((o) => o.const)).toEqual(["browser", "ask", "fixed"]);
    expect(schema.properties.widget.properties.language.default).toBe("browser");
  });

  it("a cross-project source or connector puts its project in scope.projects, once, never the own project", () => {
    const manifest = toKindManifest("AssistantDeployment", "bbsk", {
      name: "kraj",
      publicId: "kraj",
      sources: ["web", "banskabystrica/web"],
      connectors: [{ endpoint: "zvolen/air", tools: ["query_entities"] }],
      scope: { projects: ["zvolen"] },
    }) as unknown as { spec: { scope: { projects: string[] } } };
    expect(manifest.spec.scope.projects).toEqual(["zvolen", "banskabystrica"]);
    const own = toKindManifest("AssistantDeployment", "bbsk", { name: "a", publicId: "a", sources: ["web"] }) as unknown as { spec: Record<string, unknown> };
    expect(own.spec.scope).toBeUndefined();
    expect(projectsNamed({ sources: ["bbsk/web"], connectors: [null, { endpoint: 3 }] })).toEqual(["bbsk"]);
  });

  it("a source reference is a name or project/name, nothing longer", () => {
    const ref = new RegExp(PROJECT_REF);
    for (const ok of ["web", "banskabystrica/web"]) expect(ref.test(ok), ok).toBe(true);
    for (const bad of ["a/b/c", "/web", "web/", "Web", "a//b"]) expect(ref.test(bad), bad).toBe(false);
  });

  it("answers from the sources alone need a source, from the connectors alone a connector", () => {
    const errorsOf = (form: Record<string, unknown>) =>
      validator
        .validateFormData({ name: "a", publicId: "a", channel: "internal", ...form }, assistantDeploymentSchema(t))
        .errors.filter((e) => e.name !== "if")
        .map((e) => `${String(e.property).replace(/^\./, "")}:${e.name}`);
    expect(errorsOf({ answer: { routes: "rag" } })).toContain("sources:required");
    expect(errorsOf({ answer: { routes: "rag" }, sources: ["web"] })).toEqual([]);
    expect(errorsOf({ answer: { routes: "data" }, sources: ["web"] })).toContain("connectors:required");
    expect(errorsOf({ answer: { routes: "both" } })).toEqual([]);
    expect(errorsOf({ answer: { routes: "everything" } }).length).toBeGreaterThan(0);
    expect(errorsOf({ feedback: false })).toEqual([]);
  });

  it("a help text with a colour keeps its colour: an unquoted ` #` in YAML is a comment", () => {
    const form = shippedForms.find((one) => one.spec?.for === "AssistantDeployment");
    const help = form?.spec.fields?.["theme.primaryColor"]?.help as Record<string, string> | undefined;
    for (const locale of ["en", "sk", "cs", "de"]) expect(help?.[locale], locale).toMatch(/#rrggbb, .+ #1a5fb4\.$/);
  });

  it("a copy keeps the spec under a free name and publicId, cut to a DNS label", () => {
    const original = { metadata: { name: "obcania", namespace: "bb" }, spec: { publicId: "bb-obcania", sources: ["web"], theme: { greeting: "Ahoj" } } };
    const copy = duplicateDeployment(original, [original, { metadata: { name: "obcania-copy" }, spec: { publicId: "bb-obcania-copy" } }]);
    expect(copy.metadata).toEqual({ name: "obcania-copy-2", namespace: "bb" });
    expect(copy.spec).toEqual({ publicId: "bb-obcania-copy-2", sources: ["web"], theme: { greeting: "Ahoj" } });
    // The copy is its own: changing it leaves the original as it was.
    (copy.spec.theme as { greeting: string }).greeting = "Hi";
    expect(original.spec.theme.greeting).toBe("Ahoj");
    const long = "a".repeat(63);
    const cut = duplicateDeployment({ metadata: { name: long }, spec: {} }, []);
    expect(cut.metadata.name).toHaveLength(63);
    expect(cut.metadata.name.endsWith("-copy")).toBe(true);
    expect(cut.spec.publicId).toBe(cut.metadata.name);
  });
});

describe("the deployment form reads once and lines up (T-4311)", () => {
  it("a group's description and its fields' help say different things", () => {
    const form = shippedForms.find((one) => one.spec?.for === "AssistantDeployment");
    const words = (text: string) => text.toLowerCase().match(/[\p{L}]+/gu) ?? [];
    // Three words in a row both sides share is the same sentence twice ("the sites that…").
    const runs = (text: string) => new Set(words(text).slice(2).map((_, at) => words(text).slice(at, at + 3).join(" ")));
    for (const group of form?.spec.groups ?? []) {
      for (const locale of ["en", "sk", "cs", "de"]) {
        const said = runs((group.description as Record<string, string> | undefined)?.[locale] ?? "");
        for (const field of group.fields) {
          const help = (form?.spec.fields?.[field]?.help as Record<string, string> | undefined)?.[locale] ?? "";
          const shared = [...runs(help)].filter((run) => said.has(run));
          expect(shared, `${JSON.stringify(group.title)} / ${field} / ${locale}`).toEqual([]);
        }
      }
    }
  });

  it("a map entry names its language once, Remove beside it, and the map's help above its entries", async () => {
    await openForm();
    const suggestions = screen.getByRole("group", { name: /^Suggested questions/ });
    const help = within(suggestions).getByText(/^Questions the chat offers before the first one/);
    const add = within(suggestions).getByRole("button", { name: "Add a language" });
    expect(help.compareDocumentPosition(add) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await userEvent.click(add);
    const key = await within(suggestions).findByLabelText("Language");
    await userEvent.clear(key);
    await userEvent.type(key, "sk");
    await userEvent.tab();
    await waitFor(() => expect(within(suggestions).getByLabelText("Language")).toHaveValue("sk"));
    const row = within(suggestions).getByTestId("map-key");
    expect(within(row).getByRole("button", { name: "Remove" })).toBeInTheDocument();
    // The inner list keeps its name for a screen reader, not a second visible "sk".
    const inner = within(suggestions).getByRole("group", { name: "sk" });
    expect(inner.querySelector("legend")).toHaveClass("sr-only");
    await userEvent.click(within(inner).getByRole("button", { name: "Add" }));
    expect(await within(inner).findByLabelText(/^Entry 1/)).toBeInTheDocument();
    expect(within(inner).queryByText(/^sk 1/)).toBeNull();
  });

  it("a list's help sits above its entries, a one-line entry has its Remove on the input's line", async () => {
    await openForm();
    const sites = screen.getByRole("group", { name: /^Allowed sites/ });
    const add = within(sites).getByRole("button", { name: "Add" });
    const help = within(sites).getByText(/^The sites a public chat may be placed on/);
    expect(help.compareDocumentPosition(add) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // Said once: not again under the list.
    expect(screen.getAllByText(/^The sites a public chat may be placed on/)).toHaveLength(1);
    await userEvent.click(add);
    const input = await within(sites).findByLabelText(/^Allowed sites 1/);
    // T-4314: a one-value entry is a line, its name for a screen reader, its Remove beside it.
    expect(input.closest("div.rounded-md")).toBeNull();
    expect(within(sites).getByText(/^Allowed sites 1/, { selector: "label" })).toHaveClass("sr-only");
  });

  it("a list's heading is a label row like a field's: its name alone (T-4314)", async () => {
    await openForm();
    expect(screen.getByRole("group", { name: "Allowed sites" })).toBeInTheDocument();
    // A map is headed as a list too.
    expect(screen.getByRole("group", { name: "Suggested questions" })).toBeInTheDocument();
    expect(screen.getByTestId("knowledge-sources")).toHaveAccessibleName("Sources");
  });

  it("a source's box stays on its name's line when the badge wraps", async () => {
    const { picker } = await openForm();
    // Its help is said once, above the sources.
    const help = within(picker).getByText(/^Tick the sources it answers from/);
    const first = within(picker).getByRole("checkbox", { name: /^web/ });
    expect(help.compareDocumentPosition(first) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getAllByText(/^Tick the sources it answers from/)).toHaveLength(1);
    const box = within(picker).getByRole("checkbox", { name: /intranet/ });
    expect(box.closest("label")).toHaveClass("items-start");
  });
});
