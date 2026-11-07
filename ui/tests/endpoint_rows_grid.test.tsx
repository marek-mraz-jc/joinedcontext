import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import type { Manifest } from "../src/api/manifest";
import { endpointRows } from "../src/pages/endpoints/endpointRows";
import { MASK, manifestYaml } from "../src/components/grid/ManifestYamlDialog";
import { CallsCell, CallsDetail } from "../src/routes/EndpointsPage";

const API = "joinedcontext.com/v1alpha1";

function endpoint(project: string, name: string): Manifest {
  return {
    apiVersion: API,
    kind: "Endpoint",
    metadata: { name, namespace: project },
    spec: { contextSpaceRef: "s", slug: `${name}aaaaaaaaaaaaaaaaaaaaaaaaaa` },
    status: { phase: "Live" },
  } as Manifest;
}

function reference(name: string, project: string, endpointName: string): Manifest {
  return {
    apiVersion: API,
    kind: "SharedSpaceReference",
    metadata: { name, namespace: "espoo" },
    spec: { endpointRef: { project, name: endpointName }, alias: name },
  } as Manifest;
}

describe("the one Endpoints grid (T-6169)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("lists own rows, then shared rows with their reference, then references nothing shares", () => {
    const rows = endpointRows(
      "espoo",
      [endpoint("espoo", "mine")],
      [
        { source: "helsinki", endpoint: endpoint("helsinki", "bikes") },
        { source: "helsinki", endpoint: endpoint("helsinki", "events") },
      ],
      [reference("helsinki-bikes", "helsinki", "bikes"), reference("gone", "vantaa", "air")],
    );
    expect(rows.map((row) => [row.key, row.own, row.source, row.reference?.metadata.name ?? null])).toEqual([
      ["espoo/mine", true, "espoo", null],
      ["helsinki/bikes", false, "helsinki", "helsinki-bikes"],
      ["helsinki/events", false, "helsinki", null],
      ["gone/gone", false, null, "gone"],
    ]);
    expect(rows[3]?.endpoint).toBeNull();
  });

  it("shows a manifest as YAML without its status and with secret-like values masked", () => {
    const text = manifestYaml({
      ...endpoint("espoo", "mine"),
      spec: {
        contextSpaceRef: "s",
        cachedTokenSecretRef: { name: "helsinki-token", key: "token" },
        auth: { password: "hunter2", apiKey: "k-123" },
      },
    } as Manifest);
    expect(text).toContain("kind: Endpoint");
    expect(text).toContain("name: helsinki-token");
    expect(text).not.toContain("status");
    expect(text).not.toContain("hunter2");
    expect(text).not.toContain("k-123");
    expect(text).toContain(`password: "${MASK}"`);
  });

  // F-R2UI2-011: a credential inside an address sits under a key that names no secret.
  it("masks the password inside an address under any key, and leaves a plain address alone", () => {
    const text = manifestYaml({
      ...endpoint("espoo", "mine"),
      spec: { input: { dsn: "postgres://u:p4ss@db.example/air", url: "https://api.example:8443/v1" } },
    } as Manifest);
    expect(text).not.toContain("p4ss");
    expect(text).toContain(`dsn: postgres://${MASK}@db.example/air`);
    expect(text).toContain("url: https://api.example:8443/v1");
  });

  it("says the 7-day calls with the surfaces and the kinds of caller, and a dash while unknown", () => {
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <CallsCell
          calls={{
            name: "mine",
            slug: "x",
            total: 1234,
            daily: [],
            totals: { "ngsi-ld": 1200, mcp: 34, ogc: 0 },
            callers: { person: 1000, agent: 34, anonymous: 200, workload: 0 },
          }}
        />
      </I18nextProvider>,
    );
    const cell = screen.getByText(/^1,234/);
    expect(cell).toHaveAttribute("title", expect.stringMatching(/By surface: NGSI-LD API 1,200 · Assistants \(MCP\) 34/));
    expect(cell.title).toContain("By caller: People 1,000 · Not signed in 200 · Agents and MCP clients 34");
    expect(cell).toHaveTextContent(/NGSI-LD API 1,200, Assistants \(MCP\) 34; By caller: People 1,000, Not signed in 200, Agents and MCP clients 34/);
    expect(cell.title).not.toMatch(/OGC|Apps/);
    rerender(
      <I18nextProvider i18n={i18n}>
        <CallsCell notCounted="shared" calls={undefined} />
      </I18nextProvider>,
    );
    expect(screen.getByText(/^—/)).toHaveAttribute("title", expect.stringMatching(/project that publishes this Endpoint/));
  });

  it("shows the split in the cell panel as two labelled lists, and nothing for a kind nobody used", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <CallsDetail
          calls={{ name: "mine", slug: "x", total: 40, daily: [], totals: { mcp: 40 }, callers: { agent: 30, unknown: 10, person: 0 } }}
        />
      </I18nextProvider>,
    );
    expect(screen.getAllByText("40")).toHaveLength(2);
    expect(screen.getByRole("list", { name: "By surface" })).toHaveTextContent("Assistants (MCP) 40");
    const callers = screen.getByRole("list", { name: "By caller" });
    expect(callers).toHaveTextContent("Agents and MCP clients 30");
    expect(callers).toHaveTextContent("Not told apart yet 10");
    expect(callers).not.toHaveTextContent("People");
  });

  it("says in the panel why a row has no count, and that a zero is no calls", () => {
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <CallsDetail notCounted="failed" calls={undefined} />
      </I18nextProvider>,
    );
    expect(screen.getByText(/could not be read/)).toBeVisible();
    rerender(
      <I18nextProvider i18n={i18n}>
        <CallsDetail calls={{ name: "mine", slug: "x", total: 0, daily: [], totals: {}, callers: {} }} />
      </I18nextProvider>,
    );
    expect(screen.getByText("No answered calls in the last 7 days")).toBeVisible();
    expect(screen.queryByRole("list")).toBeNull();
  });
});
