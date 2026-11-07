/**
 * T-3792, API/01 §41: Project settings → Access explorer. An organization administrator picks a
 * principal and sees each Endpoint it reaches, what it may do there, the hub connector and a
 * person's connected clients, with Revoke behind a confirmation. Nobody else sees the tab.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { AccessExplorer, GeoPreview } from "../src/pages/access/AccessExplorer";

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const ROLE = (name: string, namespace: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Role",
  metadata: { name, namespace },
  spec: { rules: [] },
});
const ACCOUNT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ServiceAccount",
  metadata: { name: "etl", namespace: "helsinki" },
  spec: {},
};
const JANA = { id: "jana-id", email: "jana@hel.fi", firstName: "Jana", lastName: "K", enabled: true, emailVerified: true };
const ADMIN_GRANT = {
  role: "org-admin",
  binding: "admins",
  scope: "organization",
  rule: { kinds: ["RoleBinding"], verbs: ["approve", "delete"] },
};

function row(name: string, geoQ?: unknown) {
  return {
    name,
    slug: `${name}-slug`,
    space: "air",
    representations: ["ngsi-ld", "mcp"],
    policies: [`${name}-read`],
    permissions: [
      {
        resource: { type: "AirQualityObserved" },
        actions: ["queryEntity", "retrieveEntity"],
        attributes: ["pm10"],
        constraints: geoQ ? { geoQ } : {},
      },
    ],
    prohibitions: [],
  };
}

const FIVE = {
  subject: { serviceAccount: "helsinki-etl", groups: [], roles: [] },
  endpoints: ["a", "b", "c", "d", "e"].map((n, i) =>
    row(n, i === 0 ? { geometry: "Polygon", coordinates: [[[24.9, 60.1], [25.0, 60.1], [25.0, 60.2], [24.9, 60.1]]], georel: "within" } : undefined),
  ),
  mcpServers: ["city-air"],
  consents: [],
};

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

function stub(administrator: boolean, explored: unknown): Sent[] {
  const sent: Sent[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const text = request.method === "POST" ? await request.text() : "";
      sent.push({ method: request.method, path: url.pathname, body: text ? JSON.parse(text) : undefined });
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "anna", roles: [] });
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: "org", bootstrap: false, grants: administrator ? [ADMIN_GRANT] : [] });
      }
      if (url.pathname === "/api/v1/projects/helsinki/serviceaccounts") return json(LIST([ACCOUNT]));
      if (url.pathname === "/api/v1/organization/people") return json({ items: [JANA], total: 1 });
      if (url.pathname === "/api/v1/projects/helsinki/roles") return json(LIST([ROLE("reader", "helsinki")]));
      if (url.pathname === "/api/v1/projects/org/roles") return json(LIST([ROLE("steward", "org")]));
      if (url.pathname.endsWith("/access/explore")) return json(explored);
      if (request.method === "DELETE" && url.pathname.includes("/consents/")) return new Response(null, { status: 204 });
      return json(LIST([]));
    }),
  );
  return sent;
}

function renderAt(path: string) {
  window.history.pushState({}, "", path);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Access explorer (T-3792)", () => {
  it("shows the five Endpoints a service account reaches, from a deep link", async () => {
    const sent = stub(true, FIVE);
    renderAt("/projects/helsinki/settings/explorer?kind=serviceAccount&name=etl");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Show access" }));
    expect(await screen.findByText("5 Endpoints reached")).toBeInTheDocument();
    const asked = sent.find((s) => s.path.endsWith("/access/explore"));
    expect(asked?.body).toEqual({ subject: { kind: "serviceAccount", name: "etl" } });
    expect(screen.getAllByRole("listitem").filter((li) => li.querySelector("details"))).toHaveLength(5);
    expect(screen.getByText("city-air")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect all (copy hub URL)" })).toBeInTheDocument();
  });

  it("picks a role of the project or the organization, never a typed name (T-4947)", async () => {
    const sent = stub(true, { subject: { groups: [], roles: ["reader"] }, endpoints: [], mcpServers: [], consents: [] });
    renderAt("/projects/helsinki/settings/explorer?kind=role");
    const user = userEvent.setup();
    const show = await screen.findByRole("button", { name: "Show access" });
    expect(show).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("combobox", { name: /role/i }));
    const offered = within(await screen.findByRole("listbox")).getAllByRole("option").map((o) => o.textContent ?? "");
    expect(offered.some((text) => text.includes("reader"))).toBe(true);
    expect(offered.some((text) => text.includes("steward"))).toBe(true);
    await user.click(screen.getByRole("option", { name: /reader/ }));
    await user.click(show);
    await waitFor(() =>
      expect(sent.find((s) => s.path.endsWith("/access/explore"))?.body).toEqual({ subject: { kind: "role", name: "reader" } }),
    );
  });

  it("says a principal reaches nothing", async () => {
    stub(true, { subject: { groups: [], roles: [] }, endpoints: [], mcpServers: [], consents: [] });
    renderAt("/projects/helsinki/settings/explorer?kind=public");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Show access" }));
    expect(await screen.findByText("Reaches nothing")).toBeInTheDocument();
    expect(screen.queryByText("One MCP connector for all of it")).not.toBeInTheDocument();
  });

  it("draws a geoQ restriction and copies a surface URL", async () => {
    stub(true, FIVE);
    renderAt("/projects/helsinki/settings/explorer?kind=serviceAccount&name=etl");
    // After setup, which installs a clipboard of its own.
    const user = userEvent.setup();
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await user.click(await screen.findByRole("button", { name: "Show access" }));
    await user.click(await screen.findByText("a"));
    expect(screen.getByTestId("geoq-preview")).toHaveAccessibleName(/within Polygon/);
    const details = screen.getByTestId("geoq-preview").closest("details") as HTMLElement;
    await user.click(within(details).getByRole("button", { name: "Copy ngsi-ld URL" }));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("/api/endpoint/a-slug/ngsi-ld/v1/types"));
  });

  // F-R2UI2-019: Math.min(...xs) over a large polygon threw a RangeError and took the result down.
  it("draws a geoQ polygon of 200 000 points", () => {
    const ring = Array.from({ length: 200_000 }, (_, i) => [Math.cos(i), Math.sin(i)]);
    render(
      <I18nextProvider i18n={i18n}>
        <GeoPreview geoQ={{ georel: "within", geometry: "Polygon", coordinates: [ring] }} />
      </I18nextProvider>,
    );
    expect(screen.getByTestId("geoq-preview")).toBeInTheDocument();
  });

  it("revokes a person's consent only after the confirmation", async () => {
    const sent = stub(true, {
      subject: { user: "jana@hel.fi", groups: [], roles: [] },
      endpoints: [row("a")],
      mcpServers: [],
      consents: [{ clientId: "mcp-hub", grantedClientScopes: ["endpoint:a-slug"], lastUpdatedDate: 1_758_800_000_000 }],
    });
    renderAt("/projects/helsinki/settings/explorer?kind=person&name=jana-id");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Show access" }));
    await user.click(await screen.findByRole("button", { name: "Revoke" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(sent.some((s) => s.method === "DELETE")).toBe(false);
    await user.click(within(dialog).getByRole("button", { name: "Revoke" }));
    await waitFor(() =>
      expect(sent.some((s) => s.method === "DELETE" && s.path === "/api/v1/organization/people/jana-id/consents/mcp-hub")).toBe(true),
    );
  });

  it("walks by keyboard alone: pick, show, reach the first row", async () => {
    stub(true, FIVE);
    renderAt("/projects/helsinki/settings/explorer");
    const user = userEvent.setup();
    const kind = await screen.findByLabelText(/Who/);
    kind.focus();
    await user.selectOptions(kind, "serviceAccount");
    await waitFor(() => expect(screen.getByRole("combobox", { name: /service ?account/i })).toHaveValue("etl"));
    await user.tab();
    await user.tab();
    expect(screen.getByRole("button", { name: "Show access" })).toHaveFocus();
    await user.keyboard("{Enter}");
    const first = await screen.findByText("a");
    // The next stop after the button is the first row's summary, which a browser opens on Enter
    // natively (jsdom does not toggle <details>, so the stop is what is checked here).
    await user.tab();
    expect(first.closest("summary")).toHaveFocus();
  });

  it("shows nobody but an organization administrator the tab", async () => {
    const sent = stub(false, FIVE);
    renderAt("/projects/helsinki/settings/general");
    await screen.findByRole("tab", { name: "General" });
    expect(screen.queryByRole("tab", { name: "Access explorer" })).not.toBeInTheDocument();
    expect(sent.some((s) => s.path.endsWith("/access/explore"))).toBe(false);
  });

  it("renders nothing for someone else, even on a direct link", async () => {
    const sent = stub(false, FIVE);
    window.history.pushState({}, "", "/projects/helsinki/settings/explorer?kind=public");
    const { container } = render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <AccessExplorer project="helsinki" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(sent.some((s) => s.path.endsWith("/permissions/me"))).toBe(true));
    expect(container).toBeEmptyDOMElement();
    expect(sent.some((s) => s.path.endsWith("/access/explore"))).toBe(false);
  });
});
