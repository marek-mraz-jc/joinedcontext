/**
 * Coverage of the All endpoints list's remaining branches: the row menu's Edit, a retry that
 * reaches the server again, and the CKAN catalogue list (loading, failed, empty, toggled).
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AllEndpointsPage } from "../src/routes/AllEndpointsPage";
import { json, list, problem, renderPage } from "./page_contract";

const MAY = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Endpoint"], verbs: ["read", "propose", "delete"] } }],
};

function endpoint(name: string, project: string, audience: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name, namespace: project, labels: { "joinedcontext.com/space": "air" } },
    spec: { contextSpaceRef: "air", slug: `slug-${name}`, audience, enabledRepresentations: ["ngsi-ld"] },
    status: { phase: "Live" },
  };
}

function ckan(name: string, project: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "CkanInstance",
    metadata: { name, namespace: project },
    spec: { url: "https://data.city.example", secretRef: { name: "ckan-token" } },
  };
}

interface World {
  projects?: Response | "pending";
  ckanFor?: Record<string, Response>;
  endpoints?: () => Response;
}

function renderAll(world: World = {}) {
  const asked: string[] = [];
  const deletes: string[] = [];
  const view = renderPage(<AllEndpointsPage />, {
    answer: (url, request) => {
      asked.push(`${request.method} ${url.pathname}`);
      if (request.method === "DELETE") deletes.push(request.url);
      if (url.pathname.endsWith("/permissions/me")) return json(MAY);
      if (url.pathname.endsWith("/api/v1/endpoints")) {
        return world.endpoints ? world.endpoints() : json(list([endpoint("bikes", "helsinki", "public")]));
      }
      if (url.pathname === "/api/v1/projects") {
        if (world.projects === "pending") return new Promise<Response>(() => undefined);
        return world.projects ?? json(list([{ name: "helsinki" }, { name: "kosice" }]));
      }
      const m = /^\/api\/v1\/projects\/([^/]+)\/ckaninstances$/.exec(url.pathname);
      if (m && world.ckanFor?.[m[1]]) return world.ckanFor[m[1]];
      return undefined;
    },
  });
  return { ...view, asked, deletes };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("all endpoints, remaining branches", () => {
  it("opens the endpoint's edit form from the row menu", async () => {
    renderAll();
    const table = await screen.findByRole("table", { name: en.allEndpoints.title });
    const user = userEvent.setup();
    await user.click(await within(table).findByRole("button", { name: "More actions for helsinki/bikes" }));
    const edit = await screen.findByRole("menuitem", { name: "Edit" });
    await waitFor(() => expect(edit).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(edit);
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/endpoints/bikes/edit"));
  });

  it("closes the delete dialog with Cancel without deleting anything", async () => {
    const { deletes } = renderAll();
    const table = await screen.findByRole("table", { name: en.allEndpoints.title });
    const user = userEvent.setup();
    await user.click(await within(table).findByRole("button", { name: "More actions for helsinki/bikes" }));
    await user.click(await screen.findByRole("menuitem", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: en.form.cancel }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // The dialog's own look-ahead is a dry run; nothing else is ever sent.
    expect(deletes.filter((url) => !new URL(url).searchParams.has("dryRun"))).toEqual([]);
  });

  it("asks the server again when Retry is pressed after a failure", async () => {
    let calls = 0;
    const { asked } = renderAll({
      endpoints: () => {
        calls += 1;
        return calls === 1 ? problem(503, "The endpoint index is not answering.") : json(list([endpoint("bikes", "helsinki", "public")]));
      },
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("The endpoint index is not answering.");
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("table", { name: en.allEndpoints.title })).toBeInTheDocument();
    expect(asked.filter((line) => line.endsWith("/api/v1/endpoints")).length).toBeGreaterThanOrEqual(2);
  });

  it("asks for no catalogue until the list is shown, then hides it again", async () => {
    const { asked } = renderAll({ ckanFor: { helsinki: json(list([ckan("hel-open", "helsinki")])) } });
    await screen.findByRole("table", { name: en.allEndpoints.title });
    expect(asked.some((line) => line.includes("ckaninstances"))).toBe(false);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: en.allEndpoints.ckan.show }));
    expect(await screen.findByText("helsinki / hel-open")).toBeInTheDocument();
    expect(asked).toContain("GET /api/v1/projects/kosice/ckaninstances");
    await user.click(screen.getByRole("button", { name: en.allEndpoints.ckan.hide }));
    expect(screen.queryByText("helsinki / hel-open")).toBeNull();
    expect(screen.getByRole("button", { name: en.allEndpoints.ckan.show })).toHaveAttribute("aria-expanded", "false");
  });

  it("opens and closes one catalogue's access panel with its details", async () => {
    renderAll({ ckanFor: { helsinki: json(list([ckan("hel-open", "helsinki")])) } });
    await screen.findByRole("table", { name: en.allEndpoints.title });
    await userEvent.click(screen.getByRole("button", { name: en.allEndpoints.ckan.show }));
    const summary = await screen.findByText("helsinki / hel-open");
    const details = summary.closest("details") as HTMLDetailsElement;
    await userEvent.click(summary);
    expect(await screen.findByRole("heading", { name: "Access to hel-open" })).toBeInTheDocument();
    await userEvent.click(summary);
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Access to hel-open" })).toBeNull());
    expect(details.open).toBe(false);
  });

  it("says no catalogue exists when no project has one", async () => {
    renderAll();
    await screen.findByRole("table", { name: en.allEndpoints.title });
    await userEvent.click(screen.getByRole("button", { name: en.allEndpoints.ckan.show }));
    expect(await screen.findByText(en.allEndpoints.ckan.empty)).toBeInTheDocument();
  });

  it("says the catalogues are loading while the projects are not known yet", async () => {
    renderAll({ projects: "pending" });
    await screen.findByRole("table", { name: en.allEndpoints.title });
    await userEvent.click(screen.getByRole("button", { name: en.allEndpoints.ckan.show }));
    expect(await screen.findByText(en.app.loading)).toBeInTheDocument();
  });

  it("shows why the catalogue list failed, with a Retry that asks again", async () => {
    const { asked } = renderAll({ projects: problem(503, "No project list now.") });
    await screen.findByRole("table", { name: en.allEndpoints.title });
    await userEvent.click(screen.getByRole("button", { name: en.allEndpoints.ckan.show }));
    const alert = await screen.findByText("No project list now.");
    expect(alert).toBeInTheDocument();
    await userEvent.click(screen.getAllByRole("button", { name: en.app.error.retry }).at(-1) as HTMLElement);
    await waitFor(() => expect(asked.filter((line) => line === "GET /api/v1/projects").length).toBeGreaterThanOrEqual(2));
  });

  it("shows why one project's catalogues failed", async () => {
    renderAll({ ckanFor: { kosice: problem(422, "Kosice catalogues are unreadable.") } });
    await screen.findByRole("table", { name: en.allEndpoints.title });
    await userEvent.click(screen.getByRole("button", { name: en.allEndpoints.ckan.show }));
    expect(await screen.findByText("Kosice catalogues are unreadable.")).toBeInTheDocument();
  });
});
