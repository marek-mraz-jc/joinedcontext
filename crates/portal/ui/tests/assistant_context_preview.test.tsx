/**
 * T-4693 (AG-121, MF-58, MF-60): the deployment editor's selector, exclude list and categories,
 * and the preview beside the form of what the assistant being edited will read.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import en from "../src/locales/en.json";
import { describe, expect, it } from "vitest";
import validator from "../src/components/forms/validator";
import { ContextView } from "../src/pages/knowledge/AssistantContextPreview";
import type { AssistantContext } from "../src/pages/knowledge/knowledge";
import { CATEGORIES, assistantDeploymentSchema, toKindManifest } from "../src/schemas/knowledge";
import { jsonResponse, list, renderRoute } from "./pageHarness";

const t = (key: string) => key;
const PUBLIC = {
  name: "doprava",
  publicId: "bb-doprava",
  channel: "public",
  allowedOrigins: ["https://www.banskabystrica.sk"],
  rateLimit: { requestsPerMinute: 60, perClientPerMinute: 10 },
  budget: { tokensPerDay: 2000000, tokensPerConversation: 40000 },
};
const errorsOf = (form: Record<string, unknown>) =>
  validator
    .validateFormData(form, assistantDeploymentSchema(t))
    .errors.filter((e) => e.name !== "if" && e.name !== "anyOf")
    .map((e) => `${String(e.property).replace(/^\./, "")}:${e.name}`);

describe("the deployment form's selector (MF-58)", () => {
  it("a data-only assistant may pick its Endpoints by selector instead of connectors", () => {
    expect(errorsOf({ ...PUBLIC, answer: { routes: "data" }, select: { themes: ["TRAN"] } })).toEqual([]);
    expect(errorsOf({ ...PUBLIC, answer: { routes: "data" } })).toContain("connectors:required");
  });

  it("a channel nobody signs in to selects public Endpoints alone", () => {
    expect(errorsOf({ ...PUBLIC, select: { accessLevel: ["organization"] } })).toContain("select.accessLevel.0:const");
    expect(errorsOf({ ...PUBLIC, channel: "internal", select: { accessLevel: ["organization"] } })).toEqual([]);
  });

  it("refuses an unknown category, a lower-case type and a repeated exclude", () => {
    const errors = errorsOf({ ...PUBLIC, categories: ["TRAFFIC"], select: { types: ["parking"] }, exclude: ["a", "a"] });
    expect(errors).toEqual(expect.arrayContaining(["categories.0:const", "select.types.0:pattern", "exclude:uniqueItems"]));
  });

  it("offers the thirteen EU data themes", () => {
    expect(CATEGORIES).toHaveLength(13);
  });

  it("proposes select, exclude and categories as typed", () => {
    const manifest = toKindManifest("AssistantDeployment", "bb", {
      ...PUBLIC,
      select: { themes: ["TRAN"] },
      exclude: ["parking"],
      categories: ["TRAN"],
    });
    expect(manifest.spec).toMatchObject({ select: { themes: ["TRAN"] }, exclude: ["parking"], categories: ["TRAN"] });
  });
});

const CONTEXT: AssistantContext = {
  publicId: "bb-doprava",
  channel: "public",
  title: "Transport",
  languages: ["sk", "en"],
  categories: [{ code: "TRAN", label: "Transport" }],
  limits: { perClientPerHour: 60, perClientPerDay: null, messagesPerConversation: null, questionChars: null },
  endpoints: [
    {
      slug: "aaaaaaaaaaaaaaaaaaaaaaaaaa",
      project: "bbsk",
      name: "roads",
      title: "Regional roads",
      audience: "public",
      themes: [{ code: "TRAN", label: "Transport" }],
      types: ["Road"],
      tools: [],
      timeoutSeconds: 20,
    },
  ],
  omitted: ["zzzzzzzzzzzzzzzzzzzzzzzzzz"],
  sources: [],
  problems: ["a public deployment lists the origins it may be placed on"],
};

async function openForm(context: AssistantContext | null, refused = 400) {
  const previews: { url: string; body: unknown }[] = [];
  const rendered = await renderRoute({
    path: "/projects/bb/assistantdeployments/new?lang=en",
    projects: () => jsonResponse(list([{ name: "bb" }])),
    answer: (path, request) => {
      if (request.method === "POST" && path === "/api/v1/projects/bb/knowledge/deployments/context") {
        return request
          .clone()
          .json()
          .then((body: unknown) => {
            previews.push({ url: request.url, body });
            return context
              ? jsonResponse(context)
              : jsonResponse({ title: "Refused", status: refused, detail: "spec: unknown field" }, refused);
          });
      }
      if (/\/knowledgesources$/.test(path)) return jsonResponse(list([]));
      return undefined;
    },
  });
  return { ...rendered, previews };
}

describe("the preview of what the assistant will read (AG-121)", () => {
  it("shows the Endpoints, the categories, what is omitted and what is missing", async () => {
    const { previews } = await openForm(CONTEXT);
    const preview = await screen.findByRole("region", { name: "What the assistant will know" });
    const table = await within(preview).findByRole("table");
    expect(within(table).getByRole("rowheader", { name: "Regional roads" })).toBeInTheDocument();
    expect(within(table).getByRole("cell", { name: "Road" })).toBeInTheDocument();
    expect(within(preview).getAllByText("Transport").length).toBeGreaterThan(0);
    expect(within(preview).getByText(/zzzzzzzzzzzzzzzzzzzzzzzzzz/)).toBeInTheDocument();
    expect(within(preview).getByText(CONTEXT.problems[0])).toBeInTheDocument();
    // A dry run in the page's language, with the manifest being edited.
    const sent = previews[previews.length - 1];
    // An untouched selector is not sent: `select: {}` would read every Endpoint in reach.
    expect(previews.every(({ body }) => (body as { spec: Record<string, unknown> }).spec.select === undefined)).toBe(true);
    expect(new URL(sent.url).searchParams.get("dryRun")).toBe("All");
    expect(new URL(sent.url).searchParams.get("lang")).toBe("en");
    expect(sent.body).toMatchObject({ kind: "AssistantDeployment", metadata: { namespace: "bb" } });
  });

  it("a refused manifest shows the failure, not an empty context", async () => {
    await openForm(null);
    const preview = await screen.findByRole("region", { name: "What the assistant will know" });
    expect(await within(preview).findByRole("alert")).toHaveTextContent("spec: unknown field");
    expect(within(preview).queryByRole("table")).not.toBeInTheDocument();
  });

  it("asks for the preview again on Retry when the Portal was away", async () => {
    const { previews } = await openForm(null, 503);
    const preview = await screen.findByRole("region", { name: "What the assistant will know" });
    await within(preview).findByRole("alert");
    const before = previews.length;
    await userEvent.click(within(preview).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expect(previews.length).toBeGreaterThan(before));
    expect(await within(preview).findByRole("alert")).toHaveTextContent("spec: unknown field");
  });

  it("offers no Retry for a manifest the Portal refused: the same manifest is refused again", async () => {
    await openForm(null);
    const preview = await screen.findByRole("region", { name: "What the assistant will know" });
    await within(preview).findByRole("alert");
    expect(within(preview).queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });
});

describe("the knowledge sources a context reads", () => {
  it("names each source by its kind and the sites it crawls, a kind the Portal does not know by its code", () => {
    render(
      <ContextView
        context={{
          ...CONTEXT,
          omitted: [],
          problems: [],
          sources: [
            { project: "bbsk", name: "bb-web", kind: "website", visibility: "public", sites: ["https://www.banskabystrica.sk", "https://mesto.sk"], categories: [] },
            { project: "bbsk", name: "bb-files", kind: "upload", visibility: "public", sites: [], categories: [] },
            { project: "other", name: "bb-web", kind: "sitemap-x", visibility: "public", sites: [], categories: [] },
          ],
        }}
      />,
    );
    const listed = screen.getByRole("list", { name: "Knowledge sources" });
    const items = within(listed).getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(items[0]).toHaveTextContent("bb-web (Website): https://www.banskabystrica.sk, https://mesto.sk");
    expect(items[1]).toHaveTextContent(/^bb-files \(Uploaded files\)$/);
    expect(items[2]).toHaveTextContent("bb-web (sitemap-x)");
    expect(screen.queryByText("No knowledge source: it answers from live data alone.")).toBeNull();
  });
});

describe("an empty context", () => {
  it("says the assistant reads no Endpoint and no source, with no table and no warning", () => {
    render(
      <ContextView
        context={{
          publicId: "bb-doprava",
          channel: "public",
          title: null,
          languages: [],
          categories: [],
          limits: { perClientPerHour: null, perClientPerDay: null, messagesPerConversation: null, questionChars: null },
          endpoints: [],
          omitted: [],
          sources: [],
          problems: [],
        }}
      />,
    );
    expect(screen.getByText("No Endpoint: it answers from its knowledge sources alone.")).toBeInTheDocument();
    expect(screen.getByText("No knowledge source: it answers from live data alone.")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
