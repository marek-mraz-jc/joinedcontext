import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

/**
 * T-5248 (ADR-N-058 sections 2.4 and 2.7): Administration follows `?org=` for the installation's
 * administrator, and an administrator of another Organization opens that Organization's own
 * members, groups and settings alone.
 */
const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const PROJECTS = {
  ...list([
    { name: "helsinki", sample: false, organization: "hel" },
    { name: "ovzdusie", sample: false, organization: "zilina" },
  ]),
  organizations: [
    { name: "hel", title: "City of Helsinki" },
    { name: "zilina", title: "Mesto Žilina" },
  ],
};
const manifest = (kind: string, name: string, spec: object) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "org" },
  spec,
});
const BINDINGS = list([
  manifest("RoleBinding", "hel-admins", { role: "org-admin", subjects: [{ user: "jana@hel.fi" }], scope: { organization: "hel" } }),
  manifest("RoleBinding", "zilina-admins", { role: "org-admin", subjects: [{ user: "eva@zilina.sk" }], scope: { organization: "zilina" } }),
]);
const ORGANIZATIONS = list([
  manifest("Organization", "hel", { domain: "hel.fi", installation: true }),
  manifest("Organization", "zilina", { domain: "zilina.sk" }),
]);
const rule = (kinds: string[], verbs: string[]) => ({ kinds, verbs });
const INSTALLATION_ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "hel-admins", scope: "organization", rule: rule(["RoleBinding", "Organization", "Group"], ["propose", "approve", "delete"]) }],
};
const OWN_ADMIN = {
  project: "org",
  bootstrap: false,
  grants: ["RoleBinding", "Organization", "Group"].map((kind) => ({
    role: "org-admin",
    binding: "zilina-admins",
    scope: "organization",
    organization: "zilina",
    rule: rule([kind], ["propose", "approve"]),
  })),
};

function start(path: string, permissions: object, organizations = ORGANIZATIONS, bindings = BINDINGS) {
  window.history.pushState({}, "", path);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? IDENTITY
        : url.endsWith("/api/v1/projects")
          ? PROJECTS
          : url.includes("/permissions/me")
            ? permissions
            : url.includes("/projects/org/rolebindings")
              ? bindings
              : url.includes("/projects/org/organizations")
                ? organizations
                : list([]);
      return Promise.resolve(
        new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("Administration and the chosen Organization (T-5248)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists the members of the Organization ?org= names for the installation's administrator", async () => {
    start("/organization/members?org=zilina", INSTALLATION_ADMIN);
    expect(await screen.findByText("eva@zilina.sk", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.queryByText("jana@hel.fi")).not.toBeInTheDocument();
  });

  it("lists every Organization's members without ?org=", async () => {
    start("/organization/members", INSTALLATION_ADMIN);
    expect(await screen.findByText("eva@zilina.sk", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.getByText("jana@hel.fi")).toBeInTheDocument();
  });

  it("shows the Organization ?org= names on Settings, else the installation's", async () => {
    start("/organization/settings?org=zilina", INSTALLATION_ADMIN);
    expect((await screen.findAllByText("zilina.sk", {}, { timeout: 5000 })).length).toBeGreaterThan(0);
    expect(screen.queryByText("hel.fi")).not.toBeInTheDocument();
  });

  it("opens an Organization's own administrator its members, groups and settings alone", async () => {
    // The server lists this administrator only Žilina's manifests.
    const own = list([manifest("Organization", "zilina", { domain: "zilina.sk" })]);
    start("/organization/settings", OWN_ADMIN, own, list([BINDINGS.items[1]]));
    expect((await screen.findAllByText("zilina.sk", {}, { timeout: 5000 })).length).toBeGreaterThan(0);
    const [nav] = await screen.findAllByRole("list", { name: "People and access" });
    const links = within(nav).getAllByRole("link").map((link) => link.textContent);
    expect(links).toEqual(["Members", "Groups"]);
    expect(screen.queryByRole("link", { name: "Brokers" })).not.toBeInTheDocument();
  });

  it("tells an Organization's own administrator that a section of the installation is not theirs", async () => {
    start("/organization/brokers", OWN_ADMIN);
    expect(await screen.findByText(/administrators/i, {}, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Brokers" })).not.toBeInTheDocument();
  });
});
