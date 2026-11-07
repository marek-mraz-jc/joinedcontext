import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parse as parseYaml } from "yaml";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { rememberPrefill } from "../src/assistant/state";
import { answeringChecks, checksSoFar } from "./checks";
import { findFormPage } from "./formPage";

// Monaco needs a canvas and a worker, which jsdom has neither of; a textarea keeps its contract.
function MockEditor({ value, onChange }: { value: string; onChange?: (value: string) => void }) {
  return <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />;
}

vi.mock("../src/pages/models/MonacoSourceView", () => ({ default: MockEditor }));

const { App } = await import("../src/App");

const PROJECT = "banskabystrica";
const NAME = "zvolen-ovzdusie";

// A kind with no form of its own, so its Edit opens the manifest as YAML: that path is what this file
// holds. It read a ContextSourceRegistration until that kind got its form (T-2345), then a DataOffer until T-1547 gave it one.
const OFFER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name: NAME, namespace: PROJECT, title: { en: "Zvolen air quality" } },
  spec: { contextSpaceRef: "hub", purpose: "https://w3id.org/dpv#ResearchAndDevelopment" },
  status: { phase: "Live" },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002b", namespace: PROJECT },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
};

function renderList(options: { verbs: string[]; answer?: "change" | "invalid" }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200, type = "application/json") =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": type } }));
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "jana.kovacova", roles: [] });
    }
    if (path.endsWith("/permissions/me")) {
      return json({ grants: [{ rule: { kinds: ["Layer"], verbs: options.verbs } }] });
    }
    if (request.method === "PUT") {
      return options.answer === "invalid"
        ? json(
            {
              type: "https://joinedcontext.com/errors/bad-request",
              title: "Bad Request",
              status: 400,
              detail: "spec.purpose must be a DPV purpose IRI",
            },
            400,
            "application/problem+json",
          )
        : json(CHANGE, 202);
    }
    if (path.endsWith(`/layers/${NAME}`)) {
      return json(OFFER);
    }
    if (path.endsWith("/layers")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [OFFER] });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

const puts = (fetchMock: ReturnType<typeof vi.fn>) =>
  fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request.method === "PUT");

/** The row's actions live behind its one menu now (T-2287): open it and hand back the item. */
async function rowMenuItem(name: string | RegExp) {
  await userEvent.click(await screen.findByRole("button", { name: /More actions/ }));
  return screen.findByRole("menuitem", { name });
}

async function openEditor() {
  await userEvent.click(await rowMenuItem(new RegExp(en.resourceEdit.button)));
  const dialog = await findFormPage();
  const yaml = await within(dialog).findByRole("textbox", { name: "YAML" });
  return { dialog, yaml: yaml as HTMLTextAreaElement };
}

describe("editing a resource from its list", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-value; Secure; path=/";
    window.history.pushState({}, "", `/projects/${PROJECT}/layers`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("keeps Edit disabled with the reason for a person whose role may not propose the kind (UI-44)", async () => {
    renderList({ verbs: ["delete"] });
    expect(await screen.findByText("Zvolen air quality")).toBeInTheDocument();
    // Listed in the menu, disabled, with the reason: a narrow role has to be readable (UI-44).
    const edit = await rowMenuItem(new RegExp(en.resourceEdit.button));
    expect(edit).toHaveAttribute("aria-disabled", "true");
    expect(edit.getAttribute("title")).toMatch(/propose/);
  });

  it("opens the stored manifest without its status and proposes the edit once", async () => {
    const fetchMock = renderList({ verbs: ["propose"] });
    const { dialog, yaml } = await openEditor();
    const shown = parseYaml(yaml.value) as Record<string, unknown>;
    expect(shown.status).toBeUndefined();
    expect(shown.spec).toEqual(OFFER.spec);

    await userEvent.clear(yaml);
    await userEvent.type(yaml, `kind: Layer\nmetadata:\n  name: ${NAME}\nspec:\n  contextSpaceRef: air\n`);
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));

    expect(await screen.findByText(CHANGE.metadata.name)).toBeInTheDocument();
    const sent = puts(fetchMock);
    expect(sent).toHaveLength(1);
    expect(new URL(sent[0].url).pathname).toBe(`/api/v1/projects/${PROJECT}/layers/${NAME}`);
    expect(sent[0].headers.get("x-csrf-token")).toBe("csrf-token-value");
    expect((await sent[0].json()).spec).toEqual({ contextSpaceRef: "air" });
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("/layers/"))).toBe(true);
  });

  it("refuses a renamed manifest before anything is sent", async () => {
    const fetchMock = renderList({ verbs: ["propose"] });
    const { dialog, yaml } = await openEditor();
    await userEvent.clear(yaml);
    await userEvent.type(yaml, "kind: Layer\nmetadata:\n  name: banska-ovzdusie\n");
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(`Keep the name ${NAME}`);
    expect(puts(fetchMock)).toHaveLength(0);
  });

  it("shows why the platform refused the edit", async () => {
    renderList({ verbs: ["propose"], answer: "invalid" });
    const { dialog } = await openEditor();
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("spec.purpose must be a DPV purpose IRI");
  });

  it("opens the editor on the change the assistant made, and sends nothing until proposed (AG-77)", async () => {
    rememberPrefill(`/projects/${PROJECT}/layers?edit=${NAME}`, {
      ...OFFER,
      status: undefined,
      spec: { ...OFFER.spec, contextSpaceRef: "air" },
    });
    window.history.pushState({}, "", `/projects/${PROJECT}/layers?edit=${NAME}&draft=${NAME}`);
    const fetchMock = renderList({ verbs: ["propose"] });
    const dialog = await findFormPage();
    const yaml = (await within(dialog).findByRole("textbox", { name: "YAML" })) as HTMLTextAreaElement;
    expect((parseYaml(yaml.value) as { spec: unknown }).spec).toEqual({ ...OFFER.spec, contextSpaceRef: "air" });
    expect(puts(fetchMock)).toHaveLength(0);

    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    expect(await screen.findByText(CHANGE.metadata.name)).toBeInTheDocument();
    expect((await puts(fetchMock)[0].json()).spec.contextSpaceRef).toBe("air");
  });

  it("opens the editor at once for a page opened to edit one resource", async () => {
    window.history.pushState({}, "", `/projects/${PROJECT}/layers?edit=${NAME}`);
    const fetchMock = renderList({ verbs: ["propose"] });
    const dialog = await findFormPage();
    expect(dialog).toHaveTextContent("Edit Zvolen air quality");
    expect(puts(fetchMock)).toHaveLength(0);
  });
});
