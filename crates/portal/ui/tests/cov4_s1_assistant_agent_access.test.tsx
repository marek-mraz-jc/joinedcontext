// T-7706: AgentAccess gaps: an emptied block removes the profile's access, a refused proposal says why, a broken block is named, cancel closes the editor.
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AgentAccess } from "../src/pages/assistant/AgentAccess";
import { json, problem, renderPart } from "./page_contract";
import { InRouter } from "./pageHarness";

const ACCESS = {
  items: [
    {
      name: "app-builder",
      title: "Application builder",
      role: "builder",
      access: { operations: ["jc_catalog_search"] },
      egressHosts: [],
      operations: [{ name: "jc_catalog_search", readOnly: true, profile: true, person: true, reason: null }],
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

let puts: { spec: Record<string, unknown> }[] = [];

function mount(refuse = false) {
  puts = [];
  renderPart(<InRouter><AgentAccess project="helsinki" /></InRouter>, {
    answer: async (url, request) => {
      if (url.pathname.endsWith("/assistant/access")) return json(ACCESS);
      if (url.pathname.endsWith("/projects/org/agentprofiles/app-builder")) {
        if (request.method === "GET") return json(PROFILE);
        if (url.searchParams.get("dryRun") === "All") {
          return refuse
            ? problem(422, "The access block names an operation nobody offers.")
            : json({ valid: true, verdict: { ok: true, findings: [] } });
        }
        puts.push((await request.json()) as { spec: Record<string, unknown> });
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "pr-9" }, status: { lane: "yellow", phase: "Pending" } }, 202);
      }
      return undefined;
    },
  });
}

const A = en.assistantPage.access;

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("editing an agent profile's access block", () => {
  it("proposes the profile without any access when the block is emptied", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(await screen.findByRole("button", { name: A.edit }));
    await user.clear(screen.getByLabelText(A.block));
    await user.click(screen.getByRole("button", { name: A.propose }));
    await waitFor(() => expect(puts).toHaveLength(1));
    expect(puts[0].spec).toEqual({ role: "builder" });
    expect(await screen.findByText("pr-9")).toBeInTheDocument();
  });

  it("says the server's reason when the proposal is refused, and keeps the editor open", async () => {
    const user = userEvent.setup();
    mount(true);
    await user.click(await screen.findByRole("button", { name: A.edit }));
    await user.click(screen.getByRole("button", { name: A.propose }));
    expect(await screen.findByText(/names an operation nobody offers/)).toBeInTheDocument();
    expect(puts).toHaveLength(0);
    expect(screen.getByLabelText(A.block)).toBeInTheDocument();
  });

  it("names a block that is not YAML and sends nothing", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(await screen.findByRole("button", { name: A.edit }));
    const block = screen.getByLabelText(A.block);
    await user.clear(block);
    await user.type(block, "operations: [[unclosed");
    await user.click(screen.getByRole("button", { name: A.propose }));
    expect(await screen.findByText(/Not valid YAML, correct it/)).toBeInTheDocument();
    expect(puts).toHaveLength(0);
  });

  it("closes the editor on cancel without proposing", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(await screen.findByRole("button", { name: A.edit }));
    await user.click(screen.getByRole("button", { name: A.cancel }));
    expect(screen.queryByLabelText(A.block)).toBeNull();
    expect(puts).toHaveLength(0);
  });
});
