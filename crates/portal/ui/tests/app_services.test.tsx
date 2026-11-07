/**
 * T-3586: the three layers of an App's platform services in the Portal (AP-163, AP-164, AP-165,
 * ADR-N-045). The App page ticks the App's services and shows one the organization or the project
 * keeps off as off with that layer named; Propose sends the App with the new `spec.services`;
 * the project form narrows the list and the organization form leaves the default in force when no
 * box is ticked; an App edit keeps the services and quotas it has no field for.
 */
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import en from "../src/locales/en.json";
import { AppServices, serviceLock } from "../src/pages/apps/AppServices";
import { fromAppEnvelope, toAppEnvelope } from "../src/pages/apps/appForm";
import { toOrganization } from "../src/pages/organization/OrganizationSettings";
import { fromProject, toProject } from "../src/pages/projectSettings/ProjectSettingsPage";
import { expectNoViolations } from "./checks";
import { list, renderPage } from "./page_contract";

const APP = "/api/v1/projects/helsinki/apps/alerts";
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-7", namespace: "helsinki" },
  spec: { lane: "red" },
  status: { phase: "Pending" },
};

const app = (services?: string[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "App",
  metadata: { name: "alerts", namespace: "helsinki", labels: { team: "ops" } },
  spec: {
    kind: "static",
    visibility: "project",
    dataNeeds: [],
    limits: { emailsPerDay: 5 },
    ...(services ? { services } : {}),
  },
});

function renderSection({
  services = ["files"],
  organization,
  project,
  mayPropose = true,
}: { services?: string[]; organization?: string[]; project?: string[]; mayPropose?: boolean } = {}) {
  const sent: { dryRun: boolean; body: { spec: Record<string, unknown> } }[] = [];
  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
  renderPage(<AppServices project="helsinki" name="alerts" />, {
    path: "/projects/helsinki/apps/alerts",
    answer: async (url, request) => {
      if (url.pathname === "/api/v1/projects/helsinki/permissions/me") {
        return reply(200, {
          project: "helsinki",
          bootstrap: false,
          grants: [{ rule: { kinds: ["App"], verbs: mayPropose ? ["read", "propose"] : ["read"] } }],
        });
      }
      if (url.pathname === "/api/v1/projects/org/organizations") {
        return reply(
          200,
          list([{ metadata: { name: "helsinki" }, spec: organization ? { policies: { apps: { services: organization } } } : {} }]),
        );
      }
      if (url.pathname === "/api/v1/projects/org/projects/helsinki") {
        return reply(200, { metadata: { name: "helsinki" }, spec: project ? { apps: { services: project } } : {} });
      }
      if (url.pathname === APP && request.method === "GET") return reply(200, app(services));
      if (url.pathname === APP && request.method === "PUT") {
        const dryRun = url.searchParams.get("dryRun") === "All";
        sent.push({ dryRun, body: (await request.json()) as { spec: Record<string, unknown> } });
        return reply(200, dryRun ? { valid: true, verdict: { ok: true } } : CHANGE);
      }
      return undefined;
    },
  });
  return sent;
}

// The name is the label and the hint together, so a box is found by the label it starts with.
const box = (service: keyof typeof en.appServices.service) =>
  screen.findByRole("checkbox", { name: new RegExp(`^${en.appServices.service[service].label}`) });

describe("serviceLock", () => {
  it("names the layer the Portal names on a refused call", () => {
    expect(serviceLock("identity", [], [])).toBe("everyApp");
    expect(serviceLock("email", undefined, undefined)).toBe("organization");
    expect(serviceLock("files", undefined, undefined)).toBeUndefined();
    expect(serviceLock("email", ["email"], ["files"])).toBe("project");
    // The organization decides first: a project cannot turn on what the organization keeps off.
    expect(serviceLock("ai", ["files"], ["ai"])).toBe("organization");
    expect(serviceLock("email", ["email"], undefined)).toBeUndefined();
  });
});

describe("AppServices", () => {
  it("shows a service a layer keeps off as off with the layer, and proposes the new list", async () => {
    const sent = renderSection({ services: ["files"], organization: ["identity", "data", "files", "email", "jobs"], project: ["files", "email"] });
    const identity = await box("identity");
    expect(identity).toBeChecked();
    expect(identity).toHaveAccessibleDescription(en.appServices.lock.everyApp);
    expect(await box("ai")).toHaveAccessibleDescription(en.appServices.lock.organization);
    const jobs = await box("jobs");
    expect(jobs).not.toBeChecked();
    expect(jobs).toHaveAccessibleDescription(en.appServices.lock.project);
    await userEvent.click(jobs);
    expect(jobs).not.toBeChecked();

    const save = screen.getByRole("button", { name: en.appServices.save });
    expect(save).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(await box("email"));
    expect(save).not.toHaveAttribute("aria-disabled", "true");
    await userEvent.click(save);
    await screen.findByText(/chg-7/);
    const proposed = sent.filter((request) => !request.dryRun).map((request) => request.body.spec);
    expect(proposed).toEqual([expect.objectContaining({ services: ["files", "email"], limits: { emailsPerDay: 5 } })]);
    await expectNoViolations(document.body);
  });

  it("disables Propose with the reason for a person who may not propose the App", async () => {
    renderSection({ mayPropose: false });
    await box("files");
    const save = screen.getByRole("button", { name: en.appServices.save });
    expect(save).toHaveAttribute("aria-disabled", "true");
  });
});

describe("the forms keep the layers", () => {
  it("the project form narrows and an empty one keeps the organization's list", () => {
    const stored = { metadata: { name: "helsinki" }, spec: { organizationRef: { name: "hel" }, apps: { services: ["files"] } } };
    expect(fromProject(stored).apps).toEqual({ services: ["files"], limits: {} });
    const narrowed = toProject(
      { title: "", description: "", quotas: {}, apps: { services: ["email"], limits: { emailsPerDay: 3, filesMiB: undefined } } },
      stored,
    ) as { spec: Record<string, unknown> };
    expect(narrowed.spec.apps).toEqual({ services: ["email"], limits: { emailsPerDay: 3 } });
    const cleared = toProject({ title: "", description: "", quotas: {}, apps: { services: [], limits: {} } }, stored) as {
      spec: Record<string, unknown>;
    };
    expect(cleared.spec).toEqual({ organizationRef: { name: "hel" } });
  });

  it("the organization form leaves the default in force when no box is ticked", () => {
    const stored = { metadata: { name: "hel" }, spec: { domain: "hel.fi" } };
    const empty = toOrganization({ policies: { apps: { public: "refused", services: [] } } }, stored) as {
      spec: { policies: { apps: Record<string, unknown> } };
    };
    expect(empty.spec.policies.apps).toEqual({ public: "refused" });
    const set = toOrganization({ policies: { apps: { services: ["identity", "email"] } } }, stored) as {
      spec: { policies: { apps: Record<string, unknown> } };
    };
    expect(set.spec.policies.apps.services).toEqual(["identity", "email"]);
  });

  it("an App edit keeps its services and the quotas the form has no field for", () => {
    const stored = app(["email"]);
    const edited = toAppEnvelope(
      "helsinki",
      { ...fromAppEnvelope(stored), limits: { requestsPerMinute: 60 } },
      stored,
    ) as { spec: Record<string, unknown> };
    expect(edited.spec.services).toEqual(["email"]);
    expect(edited.spec.limits).toEqual({ emailsPerDay: 5, requestsPerMinute: 60 });
  });
});
