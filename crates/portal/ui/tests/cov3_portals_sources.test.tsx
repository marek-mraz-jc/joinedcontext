// Sources and Datasets tabs, remaining branches: every source form field, a refusal naming no field, a double submit,
// the refusals of test and run in words, the datasets tab failing, and its grid searched.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk", sites: [], bases: [] }] };
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
const CHANGE = { metadata: { name: "chg-5", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };
const PORTAL = "/api/v1/projects/org/portals/bbsk-mobility";

type Answer = (path: string, request: Request) => Response | Promise<Response> | undefined;
type Sent = { url: string; body: { spec: { sources?: Record<string, unknown>[] } } };

async function openTab(tab: "Sources" | "Datasets", stored: Record<string, unknown>, answer: Answer = () => undefined, projects?: () => Response) {
  await renderRoute({
    path: "/portals/bbsk-mobility",
    permissions: ADMIN,
    projects,
    answer: (path, request) => {
      if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
      if (path === PORTAL && request.method === "GET") {
        return jsonResponse({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Portal",
          metadata: { name: "bbsk-mobility", namespace: "org" },
          spec: { organizationRef: "bbsk", path: "mobility", title: { en: "Mobility" }, ...stored },
        });
      }
      return answer(path, request);
    },
  });
  fireEvent.click(await screen.findByRole("tab", { name: tab }));
}

function accept(sent: Sent[]): Answer {
  return (path, request) => {
    if (path === PORTAL && request.method === "PUT") {
      return request.json().then((body: Sent["body"]) => {
        sent.push({ url: request.url, body });
        return jsonResponse(request.url.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
      });
    }
    return undefined;
  };
}

describe("the source form, more fields", () => {
  it("proposes an Endpoints source from the projects of the Organization the person reads", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openTab("Sources", {}, accept(sent), () =>
      jsonResponse(
        list([
          { name: "schools", organization: "bbsk" },
          { name: "mobility", organization: "bbsk" },
          { name: "other", organization: "ke" },
        ]),
      ),
    );
    await user.click(await screen.findByRole("button", { name: "Add a source" }));
    fireEvent.change(screen.getByLabelText(/^Type/), { target: { value: "endpoints" } });
    await user.type(screen.getByLabelText(/^Name/), "own");
    const mobility = await screen.findByRole("checkbox", { name: "mobility" });
    expect(screen.queryByRole("checkbox", { name: "other" })).toBeNull();
    await user.click(mobility);
    await user.click(screen.getByRole("checkbox", { name: "schools" }));
    await user.click(screen.getByRole("checkbox", { name: "schools" }));
    await user.clear(screen.getByLabelText(/^Schedule/));
    await user.type(screen.getByLabelText(/^Schedule/), "5 4 * * *");
    await user.click(screen.getByRole("button", { name: "Propose the source" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.sources).toEqual([expect.objectContaining({ name: "own", type: "endpoints", projects: ["mobility"] })]);
  });

  it("types the allowed hosts, the budget and the schedule of a harvested source", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openTab("Sources", {}, accept(sent));
    await user.click(await screen.findByRole("button", { name: "Add a source" }));
    fireEvent.change(screen.getByLabelText(/^Type/), { target: { value: "dcat-ap" } });
    await user.type(screen.getByLabelText(/^Name/), "kosice");
    await user.type(screen.getByLabelText(/^Address/), "https://data.kosice.sk/catalog.ttl");
    expect(screen.getByText(/Harvested from data\.kosice\.sk/)).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^Further allowed hosts/), "files.kosice.sk");
    await user.clear(screen.getByLabelText(/^Most records per run/));
    await user.type(screen.getByLabelText(/^Most records per run/), "1000");
    await user.clear(screen.getByLabelText(/^Most bytes per run/));
    await user.type(screen.getByLabelText(/^Most bytes per run/), "2000000");
    await user.clear(screen.getByLabelText(/^Schedule/));
    await user.type(screen.getByLabelText(/^Schedule/), "5 4 * * *");
    await user.click(screen.getByRole("button", { name: "Propose the source" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.sources).toEqual([
      {
        name: "kosice",
        type: "dcat-ap",
        url: "https://data.kosice.sk/catalog.ttl",
        allowedHosts: ["data.kosice.sk", "files.kosice.sk"],
        schedule: "5 4 * * *",
        budget: { records: 1000, bytes: 2000000 },
      },
    ]);
  });

  it("shows a refusal that names no field above the form", async () => {
    const user = userEvent.setup();
    await openTab("Sources", {}, (path, request) =>
      path === PORTAL && request.method === "PUT" ? problem(422, "the git forge is down") : undefined,
    );
    await user.click(await screen.findByRole("button", { name: "Add a source" }));
    fireEvent.change(screen.getByLabelText(/^Type/), { target: { value: "dcat-ap" } });
    await user.type(screen.getByLabelText(/^Name/), "kosice");
    await user.type(screen.getByLabelText(/^Address/), "https://data.kosice.sk/catalog.ttl");
    await user.clear(screen.getByLabelText(/^Most records per run/));
    await user.type(screen.getByLabelText(/^Most records per run/), "10");
    await user.clear(screen.getByLabelText(/^Most bytes per run/));
    await user.type(screen.getByLabelText(/^Most bytes per run/), "10");
    await user.click(screen.getByRole("button", { name: "Propose the source" }));
    expect(await screen.findByText("The change was not proposed")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("the git forge is down");
  });

  it("sends one proposal when the form is submitted again while it is pending", async () => {
    const user = userEvent.setup();
    let puts = 0;
    await openTab("Sources", {}, (path, request) => {
      if (path === PORTAL && request.method === "PUT") {
        puts += 1;
        return new Promise<Response>(() => undefined);
      }
      return undefined;
    });
    await user.click(await screen.findByRole("button", { name: "Add a source" }));
    fireEvent.change(screen.getByLabelText(/^Type/), { target: { value: "dcat-ap" } });
    await user.type(screen.getByLabelText(/^Name/), "kosice");
    await user.type(screen.getByLabelText(/^Address/), "https://data.kosice.sk/catalog.ttl");
    await user.clear(screen.getByLabelText(/^Most records per run/));
    await user.type(screen.getByLabelText(/^Most records per run/), "10");
    await user.clear(screen.getByLabelText(/^Most bytes per run/));
    await user.type(screen.getByLabelText(/^Most bytes per run/), "10");
    await user.click(screen.getByRole("button", { name: "Propose the source" }));
    await waitFor(() => expect(puts).toBe(1));
    const form = screen.getByLabelText(/^Name/).closest("form") as HTMLFormElement;
    fireEvent.submit(form);
    fireEvent.submit(form);
    expect(puts).toBe(1);
  });
});

describe("a source's test and run refused", () => {
  it("says a source that is gone is gone", async () => {
    await openTab("Sources", { sources: [KOSICE] }, (path) => (path.endsWith("/sources/kosice/test") ? problem(404, "no source kosice") : undefined));
    fireEvent.click(await screen.findByRole("button", { name: "Test connection of kosice" }));
    expect(await screen.findByText("This source is not in the portal any more; reload the page.")).toBeInTheDocument();
  });

  it("says the server's own words for any other refusal of a run", async () => {
    await openTab("Sources", { sources: [KOSICE] }, (path, request) =>
      path.endsWith("/sources/kosice/runs") && request.method === "POST" ? problem(500, "the harvester is restarting") : undefined,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Run kosice now" }));
    expect(await screen.findByText("the harvester is restarting")).toBeInTheDocument();
  });

  it("names the failure when a test never reaches the server", async () => {
    await openTab("Sources", { sources: [KOSICE] }, (path) => (path.endsWith("/sources/kosice/test") ? Promise.reject(new TypeError("Failed to fetch")) : undefined));
    fireEvent.click(await screen.findByRole("button", { name: "Test connection of kosice" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to fetch");
  });
});

describe("the datasets tab, more states", () => {
  const DATASETS = {
    total: 2,
    missingPublisher: [],
    profiles: [],
    items: [
      { name: "ke-ovzdusie", title: "Ovzdusie Kosice", source: "kosice", hidden: false, missing: [{ profile: "dcat-ap-sk-2.0", fields: ["personal_data"] }] },
      { name: "ke-skoly", title: "Skoly", source: "kosice", hidden: false, missing: [] },
    ],
  };

  it("says when the installation serves no catalogue", async () => {
    await openTab("Datasets", {}, (path) => (path.endsWith("/datasets") ? problem(503, "no catalogue") : undefined));
    expect(await screen.findByText("No open data catalogue here yet")).toBeInTheDocument();
  });

  it("reads the datasets again after a failed read", async () => {
    let healthy = false;
    await openTab("Datasets", {}, (path) => {
      if (!path.endsWith("/datasets")) return undefined;
      return healthy ? jsonResponse(DATASETS) : problem(500, "datasets are down");
    });
    const retry = await screen.findByRole("button", { name: "Retry" }, { timeout: 20000 });
    healthy = true;
    fireEvent.click(retry);
    expect(await screen.findByRole("table", { name: "Datasets of the portal" })).toBeInTheDocument();
  });

  it("searches every column of the grid, a word in none of them leaving it empty", async () => {
    const user = userEvent.setup();
    await openTab("Datasets", {}, (path) => (path.endsWith("/datasets") ? jsonResponse(DATASETS) : undefined));
    await screen.findByRole("table", { name: "Datasets of the portal" });
    await user.type(screen.getByRole("searchbox"), "zzz-nothing");
    expect(await screen.findByText("No rows match the search and filters.")).toBeInTheDocument();
  });
});
