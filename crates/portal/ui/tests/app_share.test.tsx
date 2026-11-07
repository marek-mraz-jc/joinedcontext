// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/apps/ShareDialog.tsx: the
// Share dialog of an App (AP-173, AP-175, T-4913), driven through the Apps page as a person uses it.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { ShareDialog } from "../src/pages/apps/ShareDialog";
import type { Manifest } from "../src/api/manifest";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["domain-editor"],
};

function app(spec: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: { name: "mapa-ovzdusia", namespace: "banskabystrica", title: { en: "Air quality map" } },
    status: { phase: "Live", build: { digest: "sha256:ab", commit: "9f1c2ab" } },
    spec: { kind: "static", visibility: "project", lifecycle: "published", dataNeeds: [], ...spec },
  };
}

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000a1b2", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

const AUDIENCE = { project: 7, organization: 42, roles: [{ name: "inspector", people: 3 }], publicAllowed: true };

function renderApps(apps: unknown[], audience: unknown = AUDIENCE, write: { body: unknown; status: number } = { body: CHANGE, status: 202 }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
        }),
      );
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/branding")) return json({ organisation: "Mestský úrad Banská Bystrica", appsOrigin: "https://dev.example.sk" });
    if (path.endsWith("/apps") && request.method === "GET") return json({ apiVersion: "v1", kind: "List", items: apps });
    if (path.endsWith("/audience")) return json(audience);
    if (path.endsWith("/build")) return json({ title: "Not Found", status: 404 }, 404);
    if (path.endsWith("/app-checks")) return json({ checks: [] });
    if (path.endsWith("/agent-runs")) return json({ items: [] });
    if (request.method !== "GET") {
      return url.searchParams.get("dryRun") === "All"
        ? json({ valid: true, verdict: { ok: true, findings: [] } })
        : json(write.body, write.status);
    }
    return json({ apiVersion: "v1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
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

const writes = (fetchMock: ReturnType<typeof vi.fn>) =>
  fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request.method !== "GET");

async function openShare(user: ReturnType<typeof userEvent.setup>): Promise<HTMLElement> {
  const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
  await user.click(within(card).getByRole("button", { name: en.rowActions.more.replace("{name}", "Air quality map") }));
  await user.click(await screen.findByRole("menuitem", { name: new RegExp(`^${en.apps.share.action}`) }));
  return screen.findByRole("dialog", { name: en.apps.share.title.replace("{name}", "Air quality map") });
}

describe("the Share dialog (AP-173)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/apps");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("says who opens it now and whom each choice reaches, in people, by the organization's name", async () => {
    const user = userEvent.setup();
    renderApps([app({ roles: [{ name: "inspector", title: { en: "Inspector" } }] })]);
    const dialog = await openShare(user);
    expect(dialog).toHaveTextContent("Who opens it now: members of this project");
    expect(await within(dialog).findByText("Everyone in Mestský úrad Banská Bystrica: 42 people")).toBeInTheDocument();
    expect(within(dialog).getByText("Members of banskabystrica: 7 people")).toBeInTheDocument();
    expect(within(dialog).getByText("Holders of Inspector: 3 people")).toBeInTheDocument();
    expect(within(dialog).getByRole("radio", { name: en.apps.share.choice.project.label })).toBeChecked();
    // The hint carries an example (UI DONE), and a published app is never made private.
    expect(within(dialog).getByText(en.apps.share.hint)).toBeInTheDocument();
    expect(within(dialog).getByRole("radio", { name: en.apps.share.choice.private.label })).toBeDisabled();
    expect(within(dialog).getByText(en.apps.share.privatePublished)).toBeInTheDocument();
    // Proposing the current audience is no change.
    expect(within(dialog).getByRole("button", { name: new RegExp(`^${en.apps.share.propose}`) })).toHaveAttribute("aria-disabled", "true");
  });

  it("proposes the organization as a Change of spec.visibility and shows it waiting for approval", async () => {
    const user = userEvent.setup();
    const fetchMock = renderApps([app()]);
    const dialog = await openShare(user);
    await user.click(within(dialog).getByRole("radio", { name: en.apps.share.choice.organization.label }));
    await user.click(within(dialog).getByRole("button", { name: en.apps.share.propose }));
    await waitFor(() => {
      expect(writes(fetchMock)).toHaveLength(2);
    });
    const [check, real] = writes(fetchMock);
    expect(new URL(check.url).searchParams.get("dryRun")).toBe("All");
    expect(real.method).toBe("PUT");
    expect(new URL(real.url).pathname).toBe("/api/v1/projects/banskabystrica/apps/mapa-ovzdusia");
    const body = JSON.parse(await real.text()) as { spec: Record<string, unknown>; status?: unknown };
    expect(body.spec.visibility).toBe("organization");
    expect(body.spec.lifecycle).toBe("published");
    expect(body.status).toBeUndefined();
    expect(await within(dialog).findByText(en.apps.share.proposed)).toBeInTheDocument();
    expect(within(dialog).queryByRole("radio")).toBeNull();
  });

  it("offers no role choice without roles and no public choice the organization refuses, each saying why", async () => {
    const user = userEvent.setup();
    renderApps([app()], { ...AUDIENCE, roles: [], publicAllowed: false });
    const dialog = await openShare(user);
    expect(within(dialog).getByRole("radio", { name: en.apps.share.choice.roles.label })).toBeDisabled();
    expect(within(dialog).getByText(en.apps.share.rolesNone)).toBeInTheDocument();
    await waitFor(() => {
      expect(within(dialog).getByRole("radio", { name: en.apps.share.choice.public.label })).toBeDisabled();
    });
    expect(within(dialog).getByText(en.apps.share.publicRefused)).toBeInTheDocument();
  });

  it("names the server's refusal, and copies the app's own address", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    renderApps([app()], AUDIENCE, {
      body: { title: "Forbidden", status: 403, detail: "approving a public App needs publisher" },
      status: 403,
    });
    const dialog = await openShare(user);
    expect(within(dialog).getByText("https://mapa-ovzdusia.apps.dev.example.sk/")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: en.apps.share.copy }));
    expect(writeText).toHaveBeenCalledWith("https://mapa-ovzdusia.apps.dev.example.sk/");
    expect(await within(dialog).findByText(en.apps.share.copied)).toBeInTheDocument();

    await user.click(within(dialog).getByRole("radio", { name: en.apps.share.choice.public.label }));
    await user.click(within(dialog).getByRole("button", { name: en.apps.share.propose }));
    expect(await within(dialog).findByText("approving a public App needs publisher")).toBeInTheDocument();
  });

  it("a retired app offers no Share, with the reason", async () => {
    const user = userEvent.setup();
    renderApps([app({ lifecycle: "retired" })]);
    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    await user.click(within(card).getByRole("button", { name: en.rowActions.more.replace("{name}", "Air quality map") }));
    const item = await screen.findByRole("menuitem", { name: new RegExp(`^${en.apps.share.action}`) });
    expect(item).toHaveAttribute("aria-disabled", "true");
    expect(item).toHaveTextContent(en.apps.share.retired);
  });
});

describe("the Share dialog on its own", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("Cancel closes it without a write, and a failed count still lets the person choose", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    const fetchMock = vi.fn((_input: RequestInfo | URL) =>
      Promise.resolve(new Response(JSON.stringify({ title: "Unavailable", status: 503 }), { status: 503, headers: { "Content-Type": "application/problem+json" } })),
    );
    vi.stubGlobal("fetch", fetchMock);
    const onClose = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ShareDialog project="banskabystrica" app={app() as unknown as Manifest} onClose={onClose} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(en.apps.share.countFailed)).toBeInTheDocument();
    expect(within(dialog).getByRole("radio", { name: en.apps.share.choice.organization.label })).toBeEnabled();
    await user.click(within(dialog).getByRole("button", { name: en.apps.share.cancel }));
    expect(onClose).toHaveBeenCalled();
    expect(fetchMock.mock.calls.every((call) => (call[0] as Request).method === "GET")).toBe(true);
  });
});
