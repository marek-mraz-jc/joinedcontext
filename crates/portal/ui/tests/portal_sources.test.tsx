// The Sources and Datasets tabs of a portal (ODP-15, T-6876): adding a source by type as a
// Change, test connection in plain words and its rate limit, run now and its run log, and hiding a
// dataset as a Change; each state as a person meets it.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EMPTY_SOURCE, outcomeKey, sourceFaults, sourceFieldOfRefusal, sourceOf } from "../src/pages/portals/PortalSources";
import { expectAxeClean, jsonResponse, problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk" }] };

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};

const KOSICE = {
  name: "kosice",
  type: "dcat-ap",
  url: "https://data.kosice.sk/catalog.ttl",
  allowedHosts: ["data.kosice.sk"],
  schedule: "23 3 * * *",
  budget: { records: 5000, bytes: 50000000 },
};

function portal(extra: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Portal",
    metadata: { name: "bbsk-mobility", namespace: "org" },
    spec: { organizationRef: "bbsk", path: "mobility", title: { en: "Mobility" }, ...extra },
  };
}

const CHANGE = { metadata: { name: "chg-1", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };

type Answer = (path: string, request: Request) => Response | Promise<Response> | undefined;

async function openTab(tab: "Sources" | "Datasets", stored: Record<string, unknown>, answer: Answer = () => undefined) {
  const rendered = await renderRoute({
    path: "/portals/bbsk-mobility",
    permissions: ADMIN,
    answer: (path, request) => {
      if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
      if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "GET") return jsonResponse(portal(stored));
      return answer(path, request);
    },
  });
  fireEvent.click(await screen.findByRole("tab", { name: tab }));
  return rendered;
}

describe("source helpers", () => {
  it("refuses a bad name, a taken name, no type, http, a bad host, a budget of 0 and a bad schedule", () => {
    const draft = { ...EMPTY_SOURCE, name: "Kosice", type: "dcat-ap" as const, url: "http://data.kosice.sk", allowedHosts: "files kosice/x", records: "0", schedule: "daily" };
    expect(sourceFaults(draft, [])).toEqual({
      name: "portals.sources.fault.name",
      url: "portals.sources.fault.url",
      allowedHosts: "portals.sources.fault.allowedHosts",
      records: "portals.sources.fault.number",
      schedule: "portals.sources.fault.schedule",
    });
    expect(sourceFaults({ ...EMPTY_SOURCE, name: "ke" }, ["ke"])).toMatchObject({ name: "portals.sources.fault.nameTaken", type: "portals.sources.fault.type" });
    expect(sourceFaults({ ...EMPTY_SOURCE, name: "own", type: "endpoints" }, [])).toEqual({ projects: "portals.sources.fault.projects" });
  });

  it("writes the url's own host into allowedHosts and an endpoints source with its projects only", () => {
    const draft = { ...EMPTY_SOURCE, name: "kosice", type: "dcat-ap" as const, url: "https://data.kosice.sk/catalog.ttl", allowedHosts: "files.kosice.sk, data.kosice.sk" };
    expect(sourceOf(draft)).toEqual({ ...KOSICE, allowedHosts: ["files.kosice.sk", "data.kosice.sk"] });
    expect(sourceOf({ ...draft, allowedHosts: "files.kosice.sk" }).allowedHosts).toEqual(["data.kosice.sk", "files.kosice.sk"]);
    expect(sourceOf({ ...EMPTY_SOURCE, name: "own", type: "endpoints", projects: ["mobility"], schedule: "" })).toEqual({ name: "own", type: "endpoints", projects: ["mobility"] });
  });

  it("puts a refusal on the field of the new source and reads an unknown word as failed", () => {
    expect(sourceFieldOfRefusal("spec.sources[1].allowedHosts: 10.0.0.1.nip.io is not in the allow-list", 1)).toBe("allowedHosts");
    expect(sourceFieldOfRefusal("spec.sources[1].budget.bytes: too big", 1)).toBe("records");
    expect(sourceFieldOfRefusal("spec.sources[0].url: x", 1)).toBeUndefined();
    expect(outcomeKey("refused host")).toBe("portals.outcome.refused_host");
    expect(outcomeKey("<script>")).toBe("portals.outcome.failed");
  });
});

describe("the sources tab", () => {
  it("says what a source is and offers to add one when the portal has none", async () => {
    const { container } = await openTab("Sources", {});
    expect(await screen.findByText("No source yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a source" })).toBeInTheDocument();
    await expectAxeClean(container);
  });

  it("adds a dcat-ap source as a checked Change with every hinted field", async () => {
    const user = userEvent.setup();
    const sent: { url: string; body: { spec: { sources: unknown[] } } }[] = [];
    await openTab("Sources", {}, (path, request) => {
      if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT") {
        return request.json().then((body) => {
          sent.push({ url: request.url, body });
          return jsonResponse(request.url.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
        });
      }
      return undefined;
    });
    await user.click(await screen.findByRole("button", { name: "Add a source" }));
    const type = screen.getByLabelText(/^Type/) as HTMLSelectElement;
    expect(Array.from(type.options).map((o) => o.value)).toEqual(["", "endpoints", "dcat-ap", "ckan-action", "nkod-sparql", "arcgis-hub"]);
    fireEvent.change(type, { target: { value: "dcat-ap" } });
    await user.type(screen.getByLabelText(/^Name/), "kosice");
    await user.type(screen.getByLabelText(/^Address/), "https://data.kosice.sk/catalog.ttl");
    for (const hint of [/for example kosice\./, /for example https:\/\/data\.kosice\.sk\/catalog\.ttl/, /for example files\.kosice\.sk/, /for example 5000\./, /50000000/, /23 3 \* \* \*/]) {
      expect(screen.getAllByText(hint).length).toBeGreaterThan(0);
    }
    await user.click(screen.getByRole("button", { name: "Propose the source" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[0].url).toContain("dryRun=All");
    expect(sent[1].body.spec.sources).toEqual([KOSICE]);
  });

  it("stops a plain http address before anything is sent", async () => {
    const user = userEvent.setup();
    const rendered = await openTab("Sources", {});
    await user.click(await screen.findByRole("button", { name: "Add a source" }));
    fireEvent.change(screen.getByLabelText(/^Type/), { target: { value: "ckan-action" } });
    await user.type(screen.getByLabelText(/^Name/), "gov");
    await user.type(screen.getByLabelText(/^Address/), "http://data.gov.sk/api/3/action");
    await user.click(screen.getByRole("button", { name: "Propose the source" }));
    expect(await screen.findByText(/Write an https address/)).toBeInTheDocument();
    expect(rendered.calls().filter((call) => call.startsWith("PUT"))).toEqual([]);
  });

  it("puts the judge's allow-list refusal on the hosts field", async () => {
    const user = userEvent.setup();
    await openTab("Sources", {}, (path, request) =>
      path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT"
        ? problem(422, "spec.sources[0].allowedHosts: files.kosice.sk is not in the deployment's allow-list")
        : undefined,
    );
    await user.click(await screen.findByRole("button", { name: "Add a source" }));
    fireEvent.change(screen.getByLabelText(/^Type/), { target: { value: "dcat-ap" } });
    await user.type(screen.getByLabelText(/^Name/), "kosice");
    await user.type(screen.getByLabelText(/^Address/), "https://data.kosice.sk/catalog.ttl");
    await user.click(screen.getByRole("button", { name: "Propose the source" }));
    const line = await screen.findByText("files.kosice.sk is not in the deployment's allow-list");
    expect(line.closest("[id]")).not.toBeNull();
  });

  it("tests a source and shows each outcome and the rate limit in plain words", async () => {
    const outcomes = ["refused host", "too large"];
    await openTab("Sources", { sources: [KOSICE] }, (path) => {
      if (path === "/api/v1/projects/org/portals/bbsk-mobility/sources/kosice/test") {
        const outcome = outcomes.shift();
        return outcome ? jsonResponse({ outcome }) : problem(429, "6 source tests a minute, 3 at once; try again in 10 s");
      }
      return undefined;
    });
    const test = await screen.findByRole("button", { name: "Test connection of kosice" });
    fireEvent.click(test);
    expect(await screen.findByText(/The address is refused: it must be https/)).toBeInTheDocument();
    fireEvent.click(test);
    expect(await screen.findByText("The answer is larger than the source's byte budget.")).toBeInTheDocument();
    fireEvent.click(test);
    expect(await screen.findByText(/Too many tests in a short time\. 6 source tests a minute/)).toBeInTheDocument();
  });

  it("runs a source now, says when one is still going and shows the run log", async () => {
    let runs = 0;
    const { container } = await openTab("Sources", { sources: [KOSICE] }, (path, request) => {
      if (path === "/api/v1/projects/org/portals/bbsk-mobility/sources/kosice/runs" && request.method === "POST") {
        runs += 1;
        return runs === 1 ? new Response(null, { status: 202 }) : problem(409, "a run of kosice is active");
      }
      if (path === "/api/v1/projects/org/portals/bbsk-mobility/sources/kosice/runs") {
        return jsonResponse({
          items: [
            { id: "r2", state: "failed", startedAt: "2026-11-14T10:00:00Z", created: 0, updated: 0, unchanged: 0, withdrawn: 0, skipped: 0, error: "over budget" },
            { id: "r1", state: "succeeded", startedAt: "2026-11-13T03:23:00Z", created: 12, updated: 3, unchanged: 40, withdrawn: 1, skipped: 2, error: null },
          ],
        });
      }
      return undefined;
    });
    fireEvent.click(await screen.findByRole("button", { name: "Run kosice now" }));
    expect(await screen.findByText(/The run started/)).toBeInTheDocument();
    const log = await screen.findByRole("table", { name: "Last runs of kosice" });
    expect(within(log).getByText(/The run stopped at the source's budget/)).toBeInTheDocument();
    expect(within(log).getByText("12 new, 3 changed, 40 same, 1 withdrawn, 2 left out")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Run kosice now" }));
    expect(await screen.findByText(/still going/)).toBeInTheDocument();
    await expectAxeClean(container);
  });

  it("says when the installation serves no catalogue", async () => {
    await openTab("Sources", { sources: [KOSICE] }, (path) =>
      path.endsWith("/sources/kosice/test") ? problem(503, "this installation serves no open data catalogue") : undefined,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Test connection of kosice" }));
    expect(await screen.findByText(/serves no open data catalogue yet/)).toBeInTheDocument();
  });
});

describe("the datasets tab", () => {
  const DATASETS = {
    total: 2,
    items: [
      { name: "ke-ovzdusie", title: "Ovzdušie Košice", source: "kosice", hidden: false },
      { name: "ke-skoly", title: "Školy", source: "kosice", hidden: true },
    ],
  };

  it("lists each dataset with its source and hides one as a Change", async () => {
    const sent: { spec: { hidden: string[] } }[] = [];
    const { container } = await openTab("Datasets", { sources: [KOSICE], hidden: ["ke-skoly"] }, (path, request) => {
      if (path === "/api/v1/projects/org/portals/bbsk-mobility/datasets") return jsonResponse(DATASETS);
      if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT") {
        return request.json().then((body) => {
          sent.push(body);
          return jsonResponse(request.url.includes("dryRun") ? { valid: true } : CHANGE);
        });
      }
      return undefined;
    });
    const table = await screen.findByRole("table", { name: "Datasets of the portal" });
    expect(within(table).getAllByText("kosice")).toHaveLength(2);
    expect(within(table).getByText("Hidden")).toBeInTheDocument();
    fireEvent.click(within(table).getByRole("button", { name: "Hide Ovzdušie Košice" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1].spec.hidden).toEqual(["ke-skoly", "ke-ovzdusie"]);
    expect(within(table).getByRole("button", { name: "List Školy again" })).toBeInTheDocument();
    await expectAxeClean(container);
  });

  it("says what to do before any source has run", async () => {
    await openTab("Datasets", {}, (path) => (path.endsWith("/datasets") ? jsonResponse({ total: 0, items: [] }) : undefined));
    expect(await screen.findByText("No dataset yet")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Go to Sources" }));
    expect(await screen.findByRole("tab", { name: "Sources", selected: true })).toBeInTheDocument();
  });

  it("answers a portal another Organization owns as not there", async () => {
    await renderRoute({
      path: "/portals/hel-default",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path.startsWith("/api/v1/projects/org/portals/hel-default")) return problem(404, "no portal hel-default");
        return undefined;
      },
    });
    expect(await screen.findByText("No portal hel-default in your Organizations")).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "Sources" })).toBeNull();
  });
});
