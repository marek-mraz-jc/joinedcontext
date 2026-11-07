// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/assistant/AgentAccess.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import en from "../src/locales/en.json";
import { AgentAccess, egressWords } from "../src/pages/assistant/AgentAccess";
import { expectNoAxeViolations, json, problem, renderPart } from "./page_contract";
import { answeringChecks, checksSoFar } from "./checks";

const PROJECT = "helsinki";

const ACCESS = {
  items: [
    {
      name: "app-builder",
      title: "Application builder",
      role: "builder",
      access: { operations: ["jc_catalog_search", "jc_endpoint_propose"] },
      egressHosts: ["registry.npmjs.org"],
      operations: [
        { name: "jc_catalog_search", readOnly: true, profile: true, person: true, reason: null },
        { name: "jc_endpoint_propose", readOnly: false, profile: true, person: false, reason: "no role grants propose on Endpoint" },
        { name: "jc_space_complete", readOnly: false, profile: false, person: true, reason: "the agent profile does not grant jc_space_complete (AG-70)" },
      ],
    },
  ],
};

const PROFILE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "AgentProfile",
  metadata: { name: "app-builder", namespace: "org" },
  spec: { role: "builder", access: ACCESS.items[0].access },
  status: { phase: "Live" },
};

let puts: unknown[] = [];

function renderPage() {
  puts = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const path = new URL(request.url, "http://localhost").pathname;
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (path.endsWith("/auth/me")) return json({ subject: "s", username: "reader", roles: [] });
      if (path.endsWith("/projects")) return json([PROJECT]);
      if (path.endsWith("/assistant/access")) return json(ACCESS);
      if (path.endsWith("/projects/org/agentprofiles/app-builder") && request.method === "GET") return json(PROFILE);
      if (path.endsWith("/projects/org/agentprofiles/app-builder") && request.method === "PUT") {
        puts.push(await request.json());
        return json(
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Change",
            metadata: { name: "pr-7" },
            status: { lane: "yellow", phase: "Pending" },
          },
          202,
        );
      }
      return json({ items: [] });
    }),
  );
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
  vi.stubGlobal("EventSource", class { addEventListener() {} removeEventListener() {} close() {} });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("Agent access on the Assistant page (UI-56)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", `/projects/${PROJECT}/assistant`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows both halves per operation and the reason one refuses", async () => {
    renderPage();
    const table = await screen.findByRole("table", { name: i18n.t("assistantPage.access.caption", { name: "app-builder" }) });
    const rows = within(table).getAllByRole("row");
    const cells = (name: string) =>
      within(rows.find((row) => within(row).queryByText(name)) as HTMLElement)
        .getAllByRole("cell")
        .map((cell) => cell.textContent);
    expect(cells("jc_catalog_search")).toEqual(["jc_catalog_search", "yes", "yes", ""]);
    expect(cells("jc_endpoint_propose")).toEqual(["jc_endpoint_propose", "yes", "no", "no role grants propose on Endpoint"]);
    expect(cells("jc_space_complete")[1]).toBe("no");
    expect(screen.getByText(i18n.t("assistantPage.access.available", { count: 1, total: 3 }))).toBeInTheDocument();
    // T-4160 #31: the registry in words, the host a hover away; the title in the page's language.
    expect(screen.getByText(en.assistantPage.access.packageHosts)).toHaveAttribute("title", "registry.npmjs.org");
  });

  // T-2760: every profile's operations open at once made the Assistant page 10,276 px tall.
  it("folds each profile's operations under a line that counts them", async () => {
    renderPage();
    const table = await screen.findByRole("table", { name: i18n.t("assistantPage.access.caption", { name: "app-builder" }) });
    const folded = table.closest("details") as HTMLDetailsElement;
    expect(folded).not.toBeNull();
    expect(folded.open).toBe(false);
    expect(within(folded).getByText(i18n.t("assistantPage.access.showOperations", { count: 3 }))).toBeInTheDocument();
  });

  it("proposes an edited access block as a change on the profile, without its status", async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: i18n.t("assistantPage.access.edit") }));
    const block = screen.getByLabelText(i18n.t("assistantPage.access.block"));
    await user.clear(block);
    await user.type(block, "operations: [[jc_catalog_search]");
    await user.click(screen.getByRole("button", { name: i18n.t("assistantPage.access.propose") }));

    await waitFor(() => expect(puts).toHaveLength(1));
    const sent = puts[0] as { spec: { access: unknown; role: string }; status?: unknown };
    expect(sent.spec.access).toEqual({ operations: ["jc_catalog_search"] });
    expect(sent.spec.role).toBe("builder");
    expect(sent.status).toBeUndefined();
    expect(await screen.findByText("pr-7")).toBeInTheDocument();
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("PUT /api/v1/projects/org/agentprofiles/app-builder"))).toBe(true);
  });
});

/**
 * The panel on its own (T-2137): the three answers it owes before it can list a profile — in
 * flight, refused, and a project whose agent holds nothing. The order they are decided in is
 * what the panel got wrong once: while the request was on its way both branches fell through
 * and the heading stood over blank space, which reads as "no access" and is not.
 */
describe("the agent access panel, mounted on its own", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  const mounted = (answer: (url: URL) => Response | undefined) =>
    renderPart(<AgentAccess project="helsinki" />, { answer });

  it("says it is reading rather than showing an empty panel", () => {
    mounted(() => undefined as never);
    expect(screen.getByRole("heading", { name: en.assistantPage.access.title })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(en.app.loading);
  });

  it("says what the API said when the read is refused, and offers to ask again", async () => {
    mounted((url) =>
      url.pathname.endsWith("/assistant/access")
        ? problem(403, "You may not read the agent's access in helsinki.")
        : undefined,
    );
    const failed = await screen.findByRole("alert");
    // The server's own sentence, not a generic one: a 403 and a 500 must not read the same.
    expect(failed).toHaveTextContent("You may not read the agent's access in helsinki.");
    expect(failed).toHaveTextContent(en.assistantPage.access.title);
    expect(failed.textContent).not.toContain(en.app.error.generic);
  });

  it("says plainly that the agent holds no profile here", async () => {
    mounted((url) =>
      url.pathname.endsWith("/assistant/access") ? json({ items: [] }) : undefined,
    );
    expect(await screen.findByText(en.assistantPage.access.none)).toBeInTheDocument();
    expect(screen.getByText(en.assistantPage.access.noneHint)).toBeInTheDocument();
  });

  it("has no axe violation in any of the three", async () => {
    const empty = renderPart(<AgentAccess project="helsinki" />, {
      answer: (url) => (url.pathname.endsWith("/assistant/access") ? json({ items: [] }) : undefined),
    });
    await screen.findByText(en.assistantPage.access.none);
    await expectNoAxeViolations(empty.container);
  });
});

describe("an agent profile in the page's words (T-4264, T-4160 #31)", () => {
  it("names the package registries in words and keeps any other host as written", async () => {
    await i18n.changeLanguage("sk");
    const t = (key: string) => i18n.t(key);
    expect(egressWords(["registry.npmjs.org", "static.crates.io"], t)).toBe("balíčky pre stavbu aplikácií");
    expect(egressWords(["registry.npmjs.org", "api.example.org"], t)).toBe("balíčky pre stavbu aplikácií, api.example.org");
    expect(egressWords([], t)).toBe(i18n.t("assistantPage.access.noEgress"));
    expect(i18n.t("assistantPage.access.profileTitles.app-builder")).toBe("Zostavovač aplikácií");
    await i18n.changeLanguage("en");
  });
});

