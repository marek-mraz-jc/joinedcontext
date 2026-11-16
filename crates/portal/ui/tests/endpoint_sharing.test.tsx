import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import validator from "@rjsf/validator-ajv8";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { expectOpen } from "./checks";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import {
  admits,
  dns1123,
  referenceManifest,
  referenceTo,
  sharedReferenceForm,
  sharedSpaceReferenceSchema,
} from "../src/components/endpoints/sharing";
import type { Manifest } from "../src/api/manifest";
import { greenVerdict, isCheck } from "./verdict";
import { findFormPage } from "./formPage";

/**
 * T-0498: sharing across the projects of one organization (EP-14, EP-15, PF-05, UI-01):
 * the owner picks the allowed projects from the project list and sees who an endpoint is
 * shared with; the consumer sees what the other projects opened to it and references it
 * with one click, through the same write API as every other change.
 */

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@hel.fi",
  roles: ["portal-editor"],
};

const API = "joinedcontext.com/v1alpha1";
const BIKES_SLUG = "abcdefghijklmnopqrstuvwxyz";
const EVENTS_SLUG = "zyxwvutsrqponmlkjihgfedcba";
const WEATHER_SLUG = "q2w3e4r5t6y7u2i3o4p5a6s7d2";
const AIR_SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

function endpoint(
  project: string,
  space: string,
  name: string,
  slug: string,
  audience: string,
  allowedProjects?: string[],
) {
  return {
    apiVersion: API,
    kind: "Endpoint",
    metadata: { name, namespace: project, labels: { "joinedcontext.com/space": space } },
    spec: {
      contextSpaceRef: space,
      slug,
      audience,
      enabledRepresentations: ["ngsi-ld", "geojson"],
      ...(allowedProjects ? { allowedProjects } : {}),
    },
    status: { phase: "Live" },
  };
}

function list(items: unknown[]) {
  return { apiVersion: API, kind: "List", items };
}

const CHANGE = {
  apiVersion: API,
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "espoo" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

/** Three projects of the same organization: helsinki publishes, espoo consumes, vantaa is a bystander. */
function responses(): Record<string, unknown> {
  return {
    "/api/v1/projects": list([{ name: "helsinki" }, { name: "espoo" }, { name: "vantaa" }]),
    "/api/v1/projects/helsinki/spaces": list([
      { apiVersion: API, kind: "ContextSpace", metadata: { name: "liikenne", namespace: "helsinki" }, spec: {} },
    ]),
    "/api/v1/projects/helsinki/endpoints": list([
      endpoint("helsinki", "liikenne", "bikes", BIKES_SLUG, "project-list", ["espoo"]),
      endpoint("helsinki", "liikenne", "events", EVENTS_SLUG, "organization"),
      endpoint("helsinki", "liikenne", "weather", WEATHER_SLUG, "project-list", ["vantaa"]),
    ]),
    "/api/v1/projects/vantaa/endpoints": list([
      endpoint("vantaa", "ilma", "air", AIR_SLUG, "public"),
    ]),
    "/api/v1/projects/espoo/endpoints": list([]),
    "/api/v1/projects/espoo/shared": list([]),
  };
}

let byPath: Record<string, unknown>;

function renderAt(path: string) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    // The check is a dry run: it answers a verdict and writes nothing (AG-62, PF-57).
    if (isCheck(request, url)) {
      return greenVerdict(request, { valid: true, lane: "yellow" }).then((body) => json(body));
    }
    if (request.method !== "GET") {
      return json(CHANGE, 202);
    }
    return json(byPath[url.pathname] ?? list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  window.history.pushState({}, "", path);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request instanceof Request && request.method !== "GET" && !String((request as Request).url ?? request).includes("/drafts") && !new URL((request as Request).url).searchParams.has("dryRun"));
}

/** The one Endpoints grid: own, shared and gone-reference rows together (T-6169). */
function sharedSection(): Promise<HTMLElement> {
  return screen.findByRole("table", { name: en.endpoints.title });
}

/** Opens a row's menu and returns its items. */
async function rowMenu(row: HTMLElement): Promise<HTMLElement[]> {
  await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
  return screen.findAllByRole("menuitem");
}

describe("endpoint sharing", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    byPath = responses();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("admits a project the way the gateway does", () => {
    const listed = endpoint("helsinki", "liikenne", "bikes", BIKES_SLUG, "project-list", ["espoo"]) as Manifest;
    const org = endpoint("helsinki", "liikenne", "events", EVENTS_SLUG, "organization") as Manifest;
    const open = endpoint("vantaa", "ilma", "air", AIR_SLUG, "public") as Manifest;
    expect(admits(listed, "helsinki", "espoo")).toBe(true);
    expect(admits(listed, "helsinki", "vantaa")).toBe(false);
    expect(admits(listed, "helsinki", "helsinki")).toBe(true);
    expect(admits(org, "helsinki", "vantaa")).toBe(true);
    expect(admits(open, "vantaa", "espoo")).toBe(true);
  });

  it("picks the allowed projects from the project list, without the project itself", async () => {
    const fetchMock = renderAt("/projects/helsinki/endpoints");

    const row = (await screen.findByText("bikes")).closest("tr") as HTMLElement;
    // The row's actions are behind its one menu now (T-2287).
    await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.endpoints.edit }));
    const dialog = await findFormPage();

    // A checkbox per other project, the one the manifest lists already ticked; no free text.
    const espoo = await within(dialog).findByRole("checkbox", { name: "espoo" });
    expect(espoo).toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: "vantaa" })).not.toBeChecked();
    expect(within(dialog).queryByRole("checkbox", { name: "helsinki" })).not.toBeInTheDocument();
    expect(dialog.querySelector("#root_allowedProjects_0")).toBeNull();

    await userEvent.click(within(dialog).getByRole("checkbox", { name: "vantaa" }));
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/helsinki/endpoints/bikes");
    const body = (await request.clone().json()) as { spec: { allowedProjects?: string[] } };
    expect(body.spec.allowedProjects).toEqual(["espoo", "vantaa"]);
  });

  // T-4214 (UI-27, photo audit T-4158): at 375 px a row is the name, the state and the action.
  // The manifest name and the App line under a title wrapped word by word in the narrow first
  // column, eight lines a row; they wait for a wider screen.
  it("keeps a row to its title on a phone, the manifest name and the App line from sm up", async () => {
    const app = endpoint("helsinki", "liikenne", "app-air-quality", AIR_SLUG, "organization") as {
      metadata: Record<string, unknown>;
    };
    app.metadata.title = { en: "Air quality" };
    app.metadata.annotations = { "joinedcontext.com/generated-by": "portal/app-reconciler" };
    byPath["/api/v1/projects/helsinki/endpoints"] = list([app]);
    renderAt("/projects/helsinki/endpoints");

    const row = (await screen.findByRole("link", { name: "Air quality" })).closest("tr") as HTMLElement;
    // The manifest name waits for a wide screen (2xl, T-6169: the grid's columns fill 1440 px).
    for (const [text, from] of [["app-air-quality", "2xl:inline"], [i18n.t("endpoints.ofApp", { app: "air-quality" }), "sm:inline"]]) {
      const line = within(row).getByText(text);
      expect(line.className).toMatch(/(^| )hidden( |$)/);
      expect(line.className).toContain(from);
    }
    // One line from sm up (T-4989): the title is the row's link, not an underlined one.
    const open = within(row).getByRole("link", { name: "Air quality" });
    expect(open).toHaveAttribute("data-row-link");
    expect(open.className).not.toMatch(/(^|\s)underline(\s|$)/);
  });

  it("shows the owner who each endpoint is shared with", async () => {
    renderAt("/projects/helsinki/endpoints");

    const bikes = (await screen.findByText("bikes")).closest("tr") as HTMLElement;
    const sharedWith = within(bikes).getByRole("list", { name: en.endpoints.sharedWith });
    expect(within(sharedWith).getByText("espoo")).toBeInTheDocument();
    expect(within(bikes).getByText(en.endpoints.audience["project-list"])).toBeInTheDocument();

    const events = screen.getByText("events").closest("tr") as HTMLElement;
    expect(within(events).getByText(en.endpoints.audience.organization)).toBeInTheDocument();
    expect(
      within(events).queryByRole("list", { name: en.endpoints.sharedWith }),
    ).not.toBeInTheDocument();
  });

  it("lists for the consumer only the endpoints whose audience admits it", async () => {
    renderAt("/projects/espoo/endpoints");

    const section = await sharedSection();
    // bikes lists espoo, events is organization-wide, air is public: all three are admitted.
    await within(section).findByText("bikes");
    expect(within(section).getByText("events")).toBeInTheDocument();
    expect(within(section).getByText("air")).toBeInTheDocument();
    // weather lists vantaa only: espoo may not see it.
    expect(within(section).queryByText("weather")).not.toBeInTheDocument();

    const bikes = within(section).getByText("bikes").closest("tr") as HTMLElement;
    // The Source chip names the publishing project; the name opens its Endpoint page there.
    expect(within(bikes).getByText("Shared by helsinki")).toBeInTheDocument();
    expect(within(bikes).getByRole("link", { name: "bikes" })).toHaveAttribute(
      "href",
      "/projects/helsinki/endpoints/bikes",
    );
    // What it serves is one summary; the surface links are on the endpoint's page (T-4989).
    expect(within(bikes).getByText(/^NGSI-LD, MCP/)).toBeInTheDocument();
    expect(within(bikes).queryByRole("link", { name: "ngsi-ld" })).not.toBeInTheDocument();
  });

  it("references a shared endpoint with one click through the write API", async () => {
    const fetchMock = renderAt("/projects/espoo/endpoints");

    const section = await sharedSection();
    const row = (await within(section).findByText("bikes")).closest("tr") as HTMLElement;
    await rowMenu(row);
    await userEvent.click(screen.getByRole("menuitem", { name: en.endpoints.shared.use }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/espoo/shared");
    const body = (await request.clone().json()) as {
      kind: string;
      metadata: { name: string; namespace: string };
      spec: { endpointRef: { project: string; name: string }; alias: string };
    };
    // By name, never by slug: the slug is this environment's, the name travels (EP-77).
    expect(body).toEqual({
      apiVersion: API,
      kind: "SharedSpaceReference",
      metadata: { name: "helsinki-bikes", namespace: "espoo" },
      spec: { endpointRef: { project: "helsinki", name: "bikes" }, alias: "helsinki-liikenne" },
    });
    // The spec is what jc-core's `SharedSpaceReferenceSpec` accepts, field for field.
    expect(validator.isValid(sharedSpaceReferenceSchema, body.spec, sharedSpaceReferenceSchema)).toBe(
      true,
    );
    // The write is a Change in the approvals flow, and the page says so.
    expect(await screen.findByText("chg-77aa11bb")).toBeInTheDocument();
  });

  // UI-44: a viewer of the consuming project sees the button closed, with the verb it lacks, and
  // clicking it writes nothing; before, it was open and the API's 403 was the first they heard.
  it("closes the one-click reference to a role that may not propose one, and writes nothing", async () => {
    byPath["/api/v1/projects/espoo/permissions/me"] = { project: "espoo", bootstrap: false, grants: [] };
    const fetchMock = renderAt("/projects/espoo/endpoints");

    const section = await sharedSection();
    const row = (await within(section).findByText("bikes")).closest("tr") as HTMLElement;
    await rowMenu(row);
    const use = screen.getByRole("menuitem", { name: new RegExp(en.endpoints.shared.use) });
    await waitFor(() => {
      expect(use).toHaveAttribute("aria-disabled", "true");
    });
    expect(use).toHaveTextContent("your role does not permit 'propose' on 'SharedSpaceReference' in this project");
    await userEvent.click(use);
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("shows a reference already declared with its alias instead of the button", async () => {
    byPath["/api/v1/projects/espoo/shared"] = list([
      {
        apiVersion: API,
        kind: "SharedSpaceReference",
        metadata: { name: "helsinki-bikes", namespace: "espoo" },
        spec: { endpointSlug: BIKES_SLUG, alias: "city-bikes" },
        status: { phase: "Live" },
      },
    ]);
    renderAt("/projects/espoo/endpoints");

    const section = await sharedSection();
    const bikes = (await within(section).findByText("bikes")).closest("tr") as HTMLElement;
    await within(bikes).findByText(en.endpoints.shared.referenced);
    const items = (await rowMenu(bikes)).map((item) => item.textContent);
    expect(items).not.toContain(en.endpoints.shared.use);
    expect(items).toContain(en.endpoints.shared.stop);
    await userEvent.keyboard("{Escape}");
    // The other endpoints still offer Use in this project.
    const events = within(section).getByText("events").closest("tr") as HTMLElement;
    expect((await rowMenu(events)).map((item) => item.textContent)).toContain(en.endpoints.shared.use);
  });

  it("offers a shared row only what a reference can do: open, YAML, alias, stop using (T-6169)", async () => {
    byPath["/api/v1/projects/espoo/shared"] = list([
      {
        apiVersion: API,
        kind: "SharedSpaceReference",
        metadata: { name: "helsinki-bikes", namespace: "espoo" },
        spec: { endpointSlug: BIKES_SLUG, alias: "city-bikes" },
        status: { phase: "Live" },
      },
    ]);
    renderAt("/projects/espoo/endpoints");

    // A reference is not a manifest a consumer saves as or works on a copy of.
    const section = await sharedSection();
    const bikes = (await within(section).findByText("bikes")).closest("tr") as HTMLElement;
    await within(bikes).findByText(en.endpoints.shared.referenced);
    // Every row menu opens with Details (owner 2026-10-31, T-6207).
    expect((await rowMenu(bikes)).map((item) => item.textContent)).toEqual([
      en.grid.details,
      en.resourceList.open,
      en.grid.yaml.action,
      en.endpoints.shared.editAlias,
      en.endpoints.shared.stop,
    ]);
  });

  it("keeps a reference whose Endpoint is gone as a row with Stop using (EP-15)", async () => {
    byPath["/api/v1/projects/espoo/shared"] = list([
      {
        apiVersion: API,
        kind: "SharedSpaceReference",
        metadata: { name: "gone-ref", namespace: "espoo" },
        spec: { endpointRef: { project: "vantaa", name: "nothing" }, alias: "old-alias" },
      },
    ]);
    renderAt("/projects/espoo/endpoints");
    const section = await sharedSection();
    const gone = (await within(section).findByText("old-alias")).closest("tr") as HTMLElement;
    expect(within(gone).getByText(en.endpoints.shared.gone)).toBeInTheDocument();
    expect((await rowMenu(gone)).map((item) => item.textContent)).toEqual([
      en.grid.details,
      en.endpoints.shared.editAlias,
      en.endpoints.shared.stop,
    ]);
  });

  // T-2570, EP-15, DM-49: the alias and the mirror schedule change in place; the source stays.
  it("edits a declared reference's alias and schedule and keeps its source", async () => {
    const declared = {
      apiVersion: API,
      kind: "SharedSpaceReference",
      metadata: { name: "helsinki-bikes", namespace: "espoo" },
      spec: {
        endpointRef: { project: "helsinki", name: "bikes" },
        alias: "city-bikes",
        cachedTokenSecretRef: { name: "helsinki-token" },
      },
      status: { phase: "Live" },
    };
    byPath["/api/v1/projects/espoo/shared"] = list([declared]);
    byPath["/api/v1/projects/espoo/shared/helsinki-bikes"] = declared;
    const fetchMock = renderAt("/projects/espoo/endpoints");

    const section = await sharedSection();
    const bikes = (await within(section).findByText("bikes")).closest("tr") as HTMLElement;
    await rowMenu(bikes);
    await userEvent.click(screen.getByRole("menuitem", { name: en.endpoints.shared.editAlias }));
    const alias = await screen.findByLabelText(new RegExp(en.endpoints.shared.edit.alias));
    await userEvent.clear(alias);
    await userEvent.type(alias, "bikes-hki");
    await userEvent.type(screen.getByLabelText(new RegExp(en.endpoints.shared.edit.interval)), "6h");
    await userEvent.click(screen.getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const put = writes(fetchMock)[0];
    expect(put.method).toBe("PUT");
    expect(new URL(put.url).pathname).toBe("/api/v1/projects/espoo/shared/helsinki-bikes");
    const body = (await put.json()) as { spec: Record<string, unknown>; status?: unknown };
    expect(body.spec).toEqual({
      endpointRef: { project: "helsinki", name: "bikes" },
      alias: "bikes-hki",
      schedule: { interval: "6h" },
      cachedTokenSecretRef: { name: "helsinki-token" },
    });
    expect(body.status).toBeUndefined();
  });

  it("drops the schedule back to the daily default when the interval is emptied", () => {
    const form = sharedReferenceForm((key) => key);
    const stored = {
      apiVersion: API,
      kind: "SharedSpaceReference",
      metadata: { name: "r", namespace: "espoo" },
      spec: { endpointSlug: BIKES_SLUG, alias: "a", schedule: { interval: "6h" } },
    };
    expect(form.fromManifest(stored)).toEqual({ alias: "a", interval: "6h" });
    expect(form.toManifest({ alias: "b", interval: " " }, stored)).toEqual({
      apiVersion: API,
      kind: "SharedSpaceReference",
      metadata: { name: "r", namespace: "espoo" },
      spec: { endpointSlug: BIKES_SLUG, alias: "b" },
    });
    expect(validator.isValid(form.schema, { alias: "b", interval: "6 hours" }, form.schema)).toBe(false);
    expect(validator.isValid(form.schema, { alias: "b", interval: "0h" }, form.schema)).toBe(false);
    expect(validator.isValid(form.schema, { alias: "b", interval: "90m" }, form.schema)).toBe(true);
  });

  it("finds a declared reference by name and an older one by slug (EP-77)", () => {
    const byRef = {
      apiVersion: API,
      kind: "SharedSpaceReference",
      metadata: { name: "helsinki-bikes", namespace: "espoo" },
      spec: { endpointRef: { project: "helsinki", name: "bikes" }, alias: "a" },
    } as Manifest;
    const bySlug = { ...byRef, spec: { endpointSlug: BIKES_SLUG, alias: "a" } } as Manifest;
    expect(referenceTo([byRef], "", "helsinki", "bikes")).toBe(byRef);
    expect(referenceTo([byRef], BIKES_SLUG, "helsinki", "events")).toBeUndefined();
    expect(referenceTo([byRef], BIKES_SLUG, "espoo", "bikes")).toBeUndefined();
    expect(referenceTo([bySlug], BIKES_SLUG, "helsinki", "bikes")).toBe(bySlug);
    expect(referenceTo([bySlug], "", "helsinki", "bikes")).toBeUndefined();
    const both = { endpointRef: { project: "helsinki", name: "bikes" }, endpointSlug: BIKES_SLUG, alias: "a" };
    expect(validator.isValid(sharedSpaceReferenceSchema, both, sharedSpaceReferenceSchema)).toBe(false);
    expect(validator.isValid(sharedSpaceReferenceSchema, { alias: "a" }, sharedSpaceReferenceSchema)).toBe(false);
  });

  it("names the reference after the source so it never shadows a local space", () => {
    const bikes = endpoint("helsinki", "liikenne", "bikes", BIKES_SLUG, "organization") as Manifest;
    expect(referenceManifest("espoo", "helsinki", bikes).spec.alias).toBe("helsinki-liikenne");
    expect(dns1123("Helsinki", "City Bikes!")).toBe("helsinki-city-bikes");
    expect(dns1123("a".repeat(40), "b".repeat(40))).toHaveLength(63);
  });

  it("keeps the references in the Endpoints page and off the sidebar (T-0706)", async () => {
    renderAt("/projects/espoo/endpoints");

    const nav = await screen.findByRole("navigation", { name: "Main navigation" });
    expect(within(nav).queryByRole("link", { name: /shared/i })).toBeNull();
    expect(await screen.findByRole("table", { name: en.endpoints.title })).toBeInTheDocument();
  });

  it("the old references URL lands on the Endpoints page (T-0706)", async () => {
    renderAt("/projects/espoo/shared");

    expect(await screen.findByRole("table", { name: en.endpoints.title })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/projects/espoo/endpoints");
  });
});
