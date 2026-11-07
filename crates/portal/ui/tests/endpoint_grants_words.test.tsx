/**
 * "Who may call it" in words (T-5592), over helsinki-bikes-ops' seeded grants: the walk asks "who
 * may write `status`?" and the section answers it without a Policy opened.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { Covers, PolicyGrants, rulesOf } from "../src/components/endpoints/PolicyGrants";
import {
  DATA_WRITE_OPERATIONS,
  SUBSCRIPTION_WRITE_OPERATIONS,
  grantChanges,
} from "../src/components/endpoints/operationGroups";
import { readsOf, writesOf } from "../src/pages/access/EffectivePermissions";
import { expectNoViolations } from "./checks";

const SUBSCRIPTIONS = ["createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"];

function policy(name: string, title: string, spec: Record<string, unknown>): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: { name, namespace: "helsinki", title: { en: title } },
    spec: { contextSpaceRef: { kind: "ContextSpace", name: "helsinki" }, ...spec },
  } as Manifest;
}

// joinedcontext-deployment components/context-gateway/seed/helsinki/helsinki-policy-bikes-ops-*.yaml
const READ = policy("bikes-ops-read", "The city's people read the bike stations", {
  assignee: { kind: "group", id: "helsinki" },
  operations: ["retrieveOps", ...SUBSCRIPTIONS],
  information: [{ entities: [{ type: "BikeHireDockingStation" }] }],
});
const STEWARD = policy("bikes-ops-steward", "The steward sets a station's status", {
  assignee: { kind: "user", id: "demo.steward@hel.fi" },
  operations: ["retrieveOps", "updateAttrs", ...SUBSCRIPTIONS],
  information: [{ entities: [{ type: "BikeHireDockingStation" }], propertyNames: ["status"] }],
});
const PIPELINES = policy("pipelines-write", "The pipelines write the stations", {
  assignee: { kind: "serviceAccount", id: "pipelines" },
  operations: ["upsertBatch"],
  information: [{ entities: [{ type: "BikeHireDockingStation" }] }],
});
const SERVICE_ACCOUNT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ServiceAccount",
  metadata: { name: "pipelines", namespace: "helsinki", title: { en: "Helsinki pipelines" } },
  spec: {},
};

let asked: string[] = [];

/** permissions/me per project; a project left out answers no grants document, so the API decides. */
type Grants = Record<string, { kinds: string[]; verbs: string[] }[]>;

function show(policies: Manifest[], serviceAccounts: number = 200, grants: Grants = {}) {
  asked = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const path = new URL(input instanceof Request ? input.url : String(input), window.location.origin).pathname;
      asked.push(path);
      const json = (status: number, body: unknown) =>
        Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
      const me = /^\/api\/v1\/projects\/([^/]+)\/permissions\/me$/.exec(path);
      const rules = me ? grants[me[1]] : undefined;
      if (rules) {
        return json(200, { grants: rules.map((rule) => ({ rule })) });
      }
      if (path.endsWith("/helsinki/serviceaccounts")) {
        return serviceAccounts === 200
          ? json(200, { kind: "List", items: [SERVICE_ACCOUNT] })
          : json(serviceAccounts, { type: "about:blank", title: "Forbidden", status: serviceAccounts });
      }
      return json(200, { kind: "List", items: [] });
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const root = createRootRoute({
    component: () => (
      <section aria-label="grants">
        <PolicyGrants project="helsinki" binding={{ kind: "all", policies }} pending={false} />
      </section>
    ),
  });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={createRouter({ routeTree: root })} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** The one grant of a policy, by the policy's title. */
async function grantOf(title: string): Promise<HTMLElement> {
  const link = await screen.findByRole("link", { name: title });
  return link.closest("li") as HTMLElement;
}

describe("who may call helsinki-bikes-ops, in words", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("answers who may write status: only the steward, on that one attribute", async () => {
    show([READ, STEWARD, PIPELINES]);
    const steward = await grantOf("The steward sets a station's status");
    expect(within(steward).getByTitle("user:demo.steward@hel.fi")).toHaveTextContent(
      `demo.steward@hel.fi (${en.choice.principalKind.user})`,
    );
    expect(within(steward).getByText(en.endpoints.page.grantChanges.data)).toBeInTheDocument();
    expect(within(steward).getByText(/Change entities/)).toBeInTheDocument();
    expect(within(steward).getByRole("listitem")).toHaveTextContent("BikeHireDockingStation: only status");

    const read = await grantOf("The city's people read the bike stations");
    expect(within(read).getByRole("listitem")).toHaveTextContent(
      `BikeHireDockingStation: ${en.endpoints.page.grantAllAttributes}`,
    );
  });

  it("says a subscription grant manages subscriptions, never that it changes data", async () => {
    show([READ]);
    const read = await grantOf("The city's people read the bike stations");
    expect(within(read).getByText(en.endpoints.page.grantChanges.subscriptions)).toBeInTheDocument();
    expect(within(read).queryByText(en.endpoints.page.grantChanges.data)).toBeNull();
    expect(within(read).getByText(`${en.choice.operationGroup.retrieveOps}, ${en.access.words.area.subscribe}`)).toBeInTheDocument();
  });

  it("names a group and a ServiceAccount by kind as a word, a ServiceAccount by the title the caller may list", async () => {
    show([READ, PIPELINES]);
    const read = await grantOf("The city's people read the bike stations");
    expect(within(read).getByTitle("group:helsinki")).toHaveTextContent(`helsinki (${en.choice.principalKind.group})`);
    const pipelines = await grantOf("The pipelines write the stations");
    await within(pipelines).findByText("Helsinki pipelines");
    expect(within(pipelines).getByTitle("serviceAccount:pipelines")).toHaveTextContent(
      `Helsinki pipelines (${en.choice.principalKind.serviceAccount})`,
    );
    const section = screen.getByRole("region", { name: "grants" });
    expect(section.textContent).not.toMatch(/\b(role|group|user|serviceAccount):/);
    for (const summary of section.querySelectorAll("summary")) {
      expect(summary.textContent).not.toMatch(/[a-z][A-Z]/);
    }
    await expectNoViolations(section);
  });

  it("keeps the id the Policy holds when the caller may not list ServiceAccounts, and never reads people", async () => {
    show([STEWARD, PIPELINES], 403);
    const pipelines = await grantOf("The pipelines write the stations");
    await vi.waitFor(() => expect(asked).toContain("/api/v1/projects/helsinki/serviceaccounts"));
    expect(within(pipelines).getByTitle("serviceAccount:pipelines")).toHaveTextContent(
      `pipelines (${en.choice.principalKind.serviceAccount})`,
    );
    expect(asked.some((path) => /people|users|groups/.test(path))).toBe(false);
  });

  it("asks for no list the caller's permissions do not read, and names the grantees by id (T-5770)", async () => {
    // A viewer of the space: reads its data, reads no Group of the organization and no
    // ServiceAccount of the project, so either list would 404 (T-3852).
    const viewer = [{ kinds: ["Endpoint", "Policy", "Entity"], verbs: ["read"] }];
    show([READ, PIPELINES], 200, { org: viewer, helsinki: viewer });
    const read = await grantOf("The city's people read the bike stations");
    const pipelines = await grantOf("The pipelines write the stations");
    await vi.waitFor(() => expect(asked).toContain("/api/v1/projects/org/permissions/me"));
    await vi.waitFor(() => expect(asked).toContain("/api/v1/projects/helsinki/permissions/me"));
    expect(within(read).getByTitle("group:helsinki")).toHaveTextContent(`helsinki (${en.choice.principalKind.group})`);
    expect(within(pipelines).getByTitle("serviceAccount:pipelines")).toHaveTextContent(
      `pipelines (${en.choice.principalKind.serviceAccount})`,
    );
    expect(asked).not.toContain("/api/v1/projects/org/groups");
    expect(asked).not.toContain("/api/v1/projects/helsinki/serviceaccounts");
  });

  it("reads each rule of a Policy with rules[], a prohibition as one", async () => {
    const both = policy("mixed", "Mixed", {
      rules: [
        { effect: "permission", assignee: { kind: "role", id: "public" }, operations: ["retrieveOps"] },
        { effect: "prohibition", assignee: { kind: "role", id: "public" }, operations: ["queryEntity"], information: [{ entities: [{ type: "Secret" }] }] },
      ],
    });
    expect(rulesOf(both)).toHaveLength(2);
    show([both]);
    const mixed = await grantOf("Mixed");
    expect(within(mixed).getByText(en.endpoints.page.grantDenies)).toBeInTheDocument();
    expect(within(mixed).getByText(en.endpoints.page.grantMayNot)).toBeInTheDocument();
    expect(within(mixed).getByText(en.endpoints.page.grantEveryType)).toBeInTheDocument();
  });
});

describe("the one write list", () => {
  it("is what Effective permissions and the endpoint page both read", () => {
    expect(writesOf([...DATA_WRITE_OPERATIONS])).toEqual([...DATA_WRITE_OPERATIONS]);
    expect(writesOf([...SUBSCRIPTION_WRITE_OPERATIONS])).toEqual([]);
    expect(readsOf(["retrieveEntity", "updateAttrs"])).toEqual(["retrieveEntity"]);
    // No second list of operation names beside it (T-5592 C7).
    const source = readFileSync(join(__dirname, "../src/pages/access/EffectivePermissions.tsx"), "utf8");
    expect(source).not.toMatch(/"(updateAttrs|createEntity|deleteEntity)"/);
  });

  it("tells a data write from a subscription write and a read", () => {
    expect(grantChanges(["retrieveOps", ...SUBSCRIPTIONS])).toBe("subscriptions");
    expect(grantChanges(["associationOps"])).toBe("subscriptions");
    expect(grantChanges(["retrieveOps", "updateAttrs"])).toBe("data");
    expect(grantChanges(["updateOps"])).toBe("data");
    expect(grantChanges(["retrieveOps"])).toBe("nothing");
  });
});

describe("what a grant covers", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  const covers = (information: Parameters<typeof Covers>[0]["information"]) =>
    render(
      <I18nextProvider i18n={i18n}>
        <Covers information={information} />
      </I18nextProvider>,
    );

  it("says every type when the grant names no information", () => {
    covers([]);
    expect(screen.getByText(en.endpoints.page.grantEveryType)).toBeTruthy();
  });

  it("names the types and the attributes, or all attributes, per entry", () => {
    covers([
      { entities: [{ type: "BikeHireDockingStation" }, { idPattern: "urn:ngsi-ld:Bike:.*" }], propertyNames: ["status"], relationshipNames: ["refDock"] },
      { propertyNames: [] },
    ]);
    const items = screen.getAllByRole("listitem").map((item) => item.textContent);
    expect(items).toEqual([
      "BikeHireDockingStation and urn:ngsi-ld:Bike:.*: only status and refDock",
      `${en.endpoints.page.grantAnyType}: ${en.endpoints.page.grantAllAttributes}`,
    ]);
  });
});
