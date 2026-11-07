/**
 * T-1752: the EditResourceDialog against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * The dialog had one branch for two states: `current.data ? editor : "Loading…"`. A GET that
 * failed left it saying "Loading…" for ever beside a red line, with nothing to press and no way
 * out but closing it, and the word sat in no live region, so a screen reader was never told the
 * editor was loading at all. The first cases here are the wait, the failure and the retry.
 *
 * `checkForm` is T-1730's and joins this file with it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import {
  expectDenied,
  expectNoRawKeys,
  expectNoViolations,
  expectOpen,
  expectTabOrder,
} from "./checks";

// Monaco needs a canvas and a worker, which jsdom has neither of; a textarea keeps its contract.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

// The change the dialog shows on success links to its approval through the router.
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to, params }: { children: React.ReactNode; to: string; params: Record<string, string> }) => (
    <a
      href={Object.entries(params ?? {}).reduce((path, [key, value]) => path.replace(`$${key}`, value), to)}
    >
      {children}
    </a>
  ),
}));

// The dialog itself, not its lazy shell (T-3316): these cases read it the moment it renders.
const { EditResourceDialogBody: EditResourceDialog } = await import("../src/components/EditResourceDialogBody");

const PROJECT = "banskabystrica";
const NAME = "zvolen-ovzdusie";

const TARGET = { project: PROJECT, kind: "ContextSourceRegistration", plural: "csrs", name: NAME };

const MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSourceRegistration",
  metadata: { name: NAME, namespace: PROJECT },
  spec: { contextSpaceRef: "hub", endpoint: "https://zvolen.example/ngsi-ld" },
  status: { phase: "Live" },
};

function json(body: unknown, status = 200, problem = false) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": problem ? "application/problem+json" : "application/json" },
  });
}

/** `get` answers the manifest read; each call may answer differently, which is what a retry is. */
function renderDialog(get: (attempt: number) => Response | Promise<Response>) {
  let attempts = 0;
  const sent: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init);
      sent.push(request);
      if (request.url.includes("/api/v1/kinds/")) {
        return json({ type: "object" });
      }
      if (request.method === "GET") {
        attempts += 1;
        return await get(attempts);
      }
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Change",
        metadata: { name: "chg-0000002b", namespace: PROJECT },
        status: { lane: "yellow", phase: "PendingApproval" },
      });
    }),
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const rendered = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <EditResourceDialog target={TARGET} open onOpenChange={() => {}} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { ...rendered, sent, gets: () => attempts };
}

const dialog = () => screen.findByRole("dialog");
const propose = async () =>
  within(await dialog()).getByRole("button", { name: en.resourceEdit.propose });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the wait and the failure are two states (UI-15, UI-16)", () => {
  it("the_wait_is_announced_and_not_drawn_as_silent_text", async () => {
    renderDialog(() => new Promise<Response>(() => {}));
    const waiting = within(await dialog()).getByRole("status");
    expect(waiting).toHaveAttribute("aria-busy", "true");
    expect(waiting).toHaveTextContent(`Reading ${NAME}…`);
  });

  it("a_manifest_that_cannot_be_read_says_why_in_the_servers_words_and_offers_no_retry_a_refusal_answers_again", async () => {
    renderDialog(() =>
      json({ title: "Forbidden", status: 403, detail: "Your role may not read this resource." }, 403, true),
    );
    const alert = await within(await dialog()).findByRole("alert");
    expect(alert).toHaveTextContent("Your role may not read this resource.");
    // A refusal answers the same the second time (T-2834); the 503 case below offers Retry.
    expect(within(alert).queryByRole("button", { name: en.app.error.retry })).toBeNull();
    // And it is not still claiming to be loading.
    expect(within(await dialog()).queryByText(`Reading ${NAME}…`)).toBeNull();
  });

  it("the_retry_reads_the_manifest_again_and_the_editor_replaces_the_failure", async () => {
    const { gets } = renderDialog((attempt) =>
      attempt === 1 ? json({ title: "Gone", status: 503, detail: "The store is away." }, 503, true) : json(MANIFEST),
    );
    const alert = await within(await dialog()).findByRole("alert");
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    expect((await screen.findByLabelText("YAML")).textContent ?? "").toContain(NAME);
    await waitFor(() => expect(gets()).toBe(2));
    expect(within(await dialog()).queryByRole("alert")).toBeNull();
  });

  it("the_status_the_portal_computes_is_not_offered_for_editing", async () => {
    renderDialog(() => json(MANIFEST));
    const editor = await screen.findByLabelText("YAML");
    expect((editor as HTMLTextAreaElement).value).toContain(NAME);
    expect((editor as HTMLTextAreaElement).value).not.toContain("status:");
  });
});

describe("the propose button while there is nothing to propose (UI-44)", () => {
  it("is_reachable_and_says_the_manifest_is_still_being_read", async () => {
    renderDialog(() => new Promise<Response>(() => {}));
    expectDenied(await propose(), `Reading ${NAME}…`);
  });

  it("is_reachable_and_says_the_manifest_could_not_be_read", async () => {
    renderDialog(() => json({ title: "Gone", status: 503, detail: "The store is away." }, 503, true));
    await within(await dialog()).findByRole("alert");
    expectDenied(await propose(), `${NAME} could not be read.`);
  });

  it("sends_nothing_while_it_is_refused", async () => {
    const { sent } = renderDialog(() => new Promise<Response>(() => {}));
    await userEvent.click(await propose());
    expect(sent.filter((request) => request.method !== "GET")).toEqual([]);
  });

  it("opens_once_the_manifest_is_there_and_a_rename_is_refused_in_words", async () => {
    renderDialog(() => json(MANIFEST));
    const editor = await screen.findByLabelText("YAML");
    expectOpen(await propose());
    await userEvent.clear(editor);
    await userEvent.type(editor, "metadata:{{}  name: other{enter}");
    await userEvent.click(await propose());
    expect(await within(await dialog()).findByRole("alert")).toHaveTextContent(
      `Keep the name ${NAME}`,
    );
  });
});

describe("the dialog meets the UI contract", () => {
  it("has_no_axe_violation_while_it_waits_while_it_fails_and_with_the_editor_open", async () => {
    const { container, unmount } = renderDialog(() => new Promise<Response>(() => {}));
    await expectNoViolations(await dialog());
    unmount();

    const failed = renderDialog(() => json({ title: "Gone", status: 503, detail: "Away." }, 503, true));
    await within(await dialog()).findByRole("alert");
    await expectNoViolations(await dialog());
    failed.unmount();

    renderDialog(() => json(MANIFEST));
    await screen.findByLabelText("YAML");
    await expectNoViolations(await dialog());
    expect(container).toBeTruthy();
  });

  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    renderDialog(() => json(MANIFEST));
    await screen.findByLabelText("YAML");
    await expectTabOrder(user, await dialog());
  });

  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    renderDialog(() => json(MANIFEST));
    await screen.findByLabelText("YAML");
    expectNoRawKeys(await dialog());
  });
});

describe("an edit tells a person what each field is for (T-3220)", () => {
  it("the_edit_of_a_project_shows_the_help_its_form_ships", async () => {
    const { projectSchema } = await import("../src/schemas/kinds");
    const { fromProject, toProject } = await import("../src/pages/projectSettings/ProjectSettingsPage");
    const project = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Project",
      metadata: { name: "helsinki", namespace: "org", title: "Helsinki city data" },
      spec: { organizationRef: "hel", quotas: { contextSpaces: 4 } },
    };
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(String(input), init);
        const url = new URL(request.url, "http://localhost");
        return url.pathname.endsWith("/forms") ? json({ items: [] }) : json(project);
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <EditResourceDialog
            target={{ project: "helsinki", home: "org", kind: "Project", plural: "projects", name: "helsinki" }}
            open
            onOpenChange={() => {}}
            form={{ schema: projectSchema((key) => i18n.t(key)), fromManifest: fromProject, toManifest: toProject }}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const box = await dialog();
    const title = await within(box).findByLabelText(/^Title/);
    expect(title).toHaveAccessibleDescription(
      "The name people read in the project switcher and at the top of every page of it.",
    );
    expect(within(box).getByText(/How many context spaces this project may run/)).toBeInTheDocument();
  });
});
