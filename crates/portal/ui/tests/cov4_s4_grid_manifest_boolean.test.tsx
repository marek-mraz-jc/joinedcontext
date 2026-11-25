// T-7706: ManifestGrid cell panel: a switch (Endpoint mcp, absent means on) stores false when turned off,
// and the member it was never set on is not written when the switch is left where unset reads.
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { ManifestGrid } from "../src/components/grid/ManifestGrid";
import endpointSchema from "./fixtures/kinds/Endpoint.json";
import { json, renderPage } from "./page_contract";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const API = "joinedcontext.com/v1alpha1";
const CHANGE = { apiVersion: API, kind: "Change", metadata: { name: "c-1", namespace: "helsinki" }, status: { state: "Pending" } };

function editor(spec: Record<string, unknown>) {
  const row = { apiVersion: API, kind: "Endpoint", metadata: { name: "one", namespace: "helsinki" }, spec, status: { phase: "Live" } } as unknown as Manifest;
  const sent: { dryRun: boolean; body: { spec: Record<string, unknown> } }[] = [];
  const onChange = vi.fn();
  renderPage(
    <ManifestGrid<Manifest>
      caption="Things"
      storageKey="test-bool"
      searchLabel="Search things"
      rows={[row]}
      rowKey={(each) => each.metadata.name}
      name={{ id: "name", label: "Name", text: (each) => each.metadata.name }}
      columns={[{ id: "field", label: "Field", field: "mcp", text: () => "x" }]}
      edit={{ project: "helsinki", plural: "endpoints", kind: "Endpoint", manifest: (each) => each, onChange }}
      empty={<p>Nothing yet</p>}
    />,
    {
      answer: async (url, request) => {
        const path = "/api/v1/projects/helsinki/endpoints/one";
        if (url.pathname === path && request.method === "GET") {
          return new Response(JSON.stringify(row), { headers: { "Content-Type": "application/json", ETag: '"v1"' } });
        }
        if (url.pathname === "/api/v1/kinds/Endpoint/schema") return json(endpointSchema);
        if (url.pathname.endsWith("/permissions/me")) {
          return json({ project: "helsinki", bootstrap: false, grants: [{ scope: "project:helsinki", rule: { kinds: ["Endpoint"], verbs: ["read", "propose"] } }] });
        }
        if (url.pathname === path && request.method === "PUT") {
          const dryRun = url.searchParams.get("dryRun") === "All";
          sent.push({ dryRun, body: (await request.json()) as { spec: Record<string, unknown> } });
          return dryRun ? json({ valid: true }) : json(CHANGE, 202);
        }
        return undefined;
      },
    },
  );
  return { sent, onChange };
}

async function openPanel(): Promise<HTMLElement> {
  await userEvent.click(await screen.findByRole("button", { name: /More actions.*one/ }));
  await userEvent.click(await screen.findByRole("menuitem", { name: en.grid.details }));
  return screen.findByRole("dialog");
}

describe("the switch of a cell panel (T-7706)", () => {
  it("shows an unset switch as on, and proposes false when it is turned off", async () => {
    const { sent, onChange } = editor({});
    const dialog = await openPanel();
    const toggle = await within(dialog).findByRole("switch");
    expect(toggle).toBeChecked();
    await userEvent.click(toggle);
    expect(toggle).not.toBeChecked();
    await userEvent.click(within(dialog).getByRole("button", { name: en.grid.propose }));
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect(sent[1]!.body.spec.mcp).toBe(false);
  });

  it("proposes true for a switch that was stored off and is turned on", async () => {
    const { sent, onChange } = editor({ mcp: false });
    const dialog = await openPanel();
    const toggle = await within(dialog).findByRole("switch");
    expect(toggle).not.toBeChecked();
    await userEvent.click(toggle);
    await userEvent.click(within(dialog).getByRole("button", { name: en.grid.propose }));
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect(sent[1]!.body.spec.mcp).toBe(true);
  });
});
