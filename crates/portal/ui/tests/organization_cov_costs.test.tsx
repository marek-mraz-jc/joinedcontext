/**
 * Administration → Costs: a second click on Export while the first is running asks nothing more,
 * and a refused export says why instead of downloading.
 */
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { Costs } from "../src/pages/organization/Costs";
import { json, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const COSTS = {
  month: "2026-09",
  currency: "EUR",
  measuredAt: "2026-10-22T01:00:00Z",
  prices: { currency: "EUR", perUsd: 0.9, nodes: [], volumes: [], fixed: [] },
  projects: [{ project: "doprava", node: 10, storage: 0, ai: 0, total: 10, share: 1, spaces: [], assistants: [] }],
  platform: { node: 5, storage: 1, total: 6 },
  infrastructure: 16,
  ai: 0,
  total: 16,
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderCosts(exported: () => Response | Promise<Response>) {
  const asked: string[] = [];
  renderPage(<Costs />, {
    path: "/organization/costs",
    answer: (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/organization/costs") return json(COSTS);
      if (url.pathname === "/api/v1/organization/costs/export") {
        asked.push(`${request.method} ${url.pathname}${url.search}`);
        return exported();
      }
      return undefined;
    },
  });
  return asked;
}

describe("Administration → Costs: the CSV export", () => {
  it("asks once when Export is clicked twice before the file is there", async () => {
    URL.createObjectURL = vi.fn(() => "blob:costs");
    URL.revokeObjectURL = vi.fn();
    let release: (response: Response) => void = () => undefined;
    const asked = renderCosts(() => new Promise<Response>((resolve) => (release = resolve)));
    const button = await screen.findByRole("button", { name: en.organization.costs.export });
    fireEvent.click(button);
    const busy = await screen.findByRole("button", { name: en.organization.costs.saving });
    expect(busy).toHaveAttribute("aria-busy", "true");
    fireEvent.click(busy);
    release(new Response("month,project\n", { headers: { "content-type": "text/csv" } }));
    await waitFor(() => expect(URL.createObjectURL).toHaveBeenCalledTimes(1));
    expect(asked).toHaveLength(1);
    expect(asked[0]).toMatch(/^GET \/api\/v1\/organization\/costs\/export\?month=/);
    expect(await screen.findByRole("button", { name: en.organization.costs.export })).toHaveAttribute("aria-busy", "false");
  });

  it("says why the export was refused and downloads nothing", async () => {
    URL.createObjectURL = vi.fn(() => "blob:costs");
    renderCosts(() => problem(403, "The cost export is for administrators."));
    fireEvent.click(await screen.findByRole("button", { name: en.organization.costs.export }));
    expect(await screen.findByText(/The cost export is for administrators/)).toBeInTheDocument();
    expect(URL.createObjectURL).not.toHaveBeenCalled();
  });
});
