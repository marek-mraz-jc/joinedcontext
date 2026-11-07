/**
 * T-3311, EP-103: "Try a person" on the Policies page asks the gateway, through the Portal,
 * what it would decide for one person, and names the Policy that decided. Only an organization
 * administrator sees it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { TryAPerson } from "../src/pages/policies/TryAPerson";

const LIST = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "air", namespace: "helsinki" },
  spec: { slug: "air-quality" },
};
/** The project's model, the source of the types worth trying (T-4947). */
const AIR_MODEL = {
  name: "air-quality",
  level: "project",
  project: "helsinki",
  version: "1.0.0",
  lifecycle: "published",
  classes: ["AirQualityObserved"],
};
const STEWARDS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Group",
  metadata: { name: "stewards", namespace: "org" },
  spec: { members: [] },
};
const JANA = {
  id: "jana-id",
  email: "jana@hel.fi",
  firstName: "Jana",
  lastName: "K",
  enabled: true,
  emailVerified: true,
};
const ADMIN_GRANT = {
  role: "org-admin",
  binding: "admins",
  scope: "organization",
  rule: { kinds: ["RoleBinding"], verbs: ["approve", "delete"] },
};

interface Sent {
  path: string;
  body: unknown;
}

function stub(
  administrator: boolean,
  answer: { status: number; body: unknown },
): Sent[] {
  const sent: Sent[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const text = request.method === "POST" ? await request.text() : "";
      sent.push({
        path: url.pathname,
        body: text ? JSON.parse(text) : undefined,
      });
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });
      if (url.pathname.endsWith("/auth/me"))
        return json({ subject: "s", username: "anna", roles: [] });
      if (url.pathname === "/api/v1/projects")
        return json({ items: [{ name: "helsinki" }] });
      if (url.pathname.endsWith("/permissions/me")) {
        return json({
          project: "org",
          bootstrap: false,
          grants: administrator ? [ADMIN_GRANT] : [],
        });
      }
      if (url.pathname === "/api/v1/projects/helsinki/endpoints")
        return json(LIST([ENDPOINT]));
      if (url.pathname === "/api/v1/organization/people")
        return json({ items: [JANA], total: 1 });
      if (url.pathname === "/api/v1/organization/datamodels")
        return json({ items: [AIR_MODEL] });
      if (url.pathname === "/api/v1/projects/org/groups")
        return json(LIST([STEWARDS]));
      if (url.pathname.endsWith("/access/simulate"))
        return json(answer.body, answer.status);
      return json(LIST([]));
    }),
  );
  return sent;
}

function renderPage() {
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  window.history.pushState({}, "", "/projects/helsinki/policies");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Try a person (T-3311)", () => {
  it("asks about the chosen person and names the Policy that decided", async () => {
    const sent = stub(true, {
      status: 200,
      body: {
        decision: true,
        reason: "policy_grant_matched",
        policy: "air-read",
        subject: { user: "jana@hel.fi", groups: ["stewards"], roles: [] },
      },
    });
    renderPage();
    const panel = (
      await screen.findByRole("heading", { name: "Try a person" })
    ).closest("div.rounded-lg") as HTMLElement;
    await within(panel).findByRole("option", { name: "jana@hel.fi" });
    // The type is picked from the project's models, never typed (T-4947).
    await userEvent.click(within(panel).getByRole("combobox", { name: "Entity type" }));
    await userEvent.click(await screen.findByRole("option", { name: /AirQualityObserved/ }));
    await userEvent.click(
      within(panel).getByRole("button", { name: "Ask the gateway" }),
    );

    const verdict = await screen.findByTestId("try-verdict");
    expect(verdict).toHaveTextContent("Allowed");
    expect(verdict).toHaveTextContent("A Policy grants this action.");
    expect(
      within(verdict).getByRole("link", { name: "air-read" }),
    ).toBeInTheDocument();
    expect(sent.find((s) => s.path.endsWith("/access/simulate"))).toEqual({
      path: "/api/v1/projects/helsinki/endpoints/air/access/simulate",
      body: {
        subject: { kind: "person", id: "jana-id" },
        action: "retrieveEntity",
        type: "AirQualityObserved",
      },
    });
  });

  it("asks about a group member by the name typed, and says a refusal plainly", async () => {
    const sent = stub(true, {
      status: 200,
      body: {
        decision: false,
        reason: "no_grant",
        subject: { groups: ["stewards"], roles: [] },
      },
    });
    renderPage();
    const panel = (
      await screen.findByRole("heading", { name: "Try a person" })
    ).closest("div.rounded-lg") as HTMLElement;
    await within(panel).findByRole("option", { name: "air" });
    await userEvent.selectOptions(
      within(panel).getByLabelText(/^Who/),
      "group",
    );
    const ask = within(panel).getByRole("button", { name: "Ask the gateway" });
    // Nothing to ask about until a group is named.
    expect(ask).toHaveAttribute("aria-disabled", "true");
    expect(ask).toHaveAccessibleDescription("Pick the Endpoint and who to try first.");
    // A group the organization has, picked from its list (T-4947).
    await userEvent.click(within(panel).getByRole("combobox", { name: /A member of a group/ }));
    await userEvent.click(await screen.findByRole("option", { name: /stewards/ }));
    await userEvent.click(ask);

    const verdict = await screen.findByTestId("try-verdict");
    expect(verdict).toHaveTextContent("Refused");
    expect(verdict).toHaveTextContent("No Policy grants this action.");
    expect(sent.find((s) => s.path.endsWith("/access/simulate"))?.body).toEqual(
      {
        subject: { kind: "group", name: "stewards" },
        action: "retrieveEntity",
      },
    );
  });

  it("shows the Portal's reason when the gateway cannot be asked", async () => {
    stub(true, {
      status: 503,
      body: {
        title: "Unavailable",
        status: 503,
        detail: "the gateway did not answer; try again shortly",
      },
    });
    renderPage();
    const panel = (
      await screen.findByRole("heading", { name: "Try a person" })
    ).closest("div.rounded-lg") as HTMLElement;
    await within(panel).findByRole("option", { name: "jana@hel.fi" });
    await userEvent.click(
      within(panel).getByRole("button", { name: "Ask the gateway" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "the gateway did not answer; try again shortly",
    );
  });

  it("is not shown, and asks nothing, for someone who does not administer the organization", async () => {
    const sent = stub(false, { status: 200, body: {} });
    renderPage();
    await screen.findByRole("heading", { name: "Policies", level: 1 });
    await waitFor(() =>
      expect(sent.some((s) => s.path.endsWith("/permissions/me"))).toBe(true),
    );
    expect(
      screen.queryByRole("heading", { name: "Try a person" }),
    ).not.toBeInTheDocument();
    expect(sent.some((s) => s.path === "/api/v1/organization/people")).toBe(
      false,
    );
  });

  it("renders nothing on its own for a non-administrator, whatever page holds it", async () => {
    const sent = stub(false, { status: 200, body: {} });
    const { container } = render(
      <QueryClientProvider
        client={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <I18nextProvider i18n={i18n}>
          <TryAPerson project="helsinki" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() =>
      expect(sent.some((s) => s.path.endsWith("/permissions/me"))).toBe(true),
    );
    expect(container).toBeEmptyDOMElement();
    expect(sent.some((s) => s.path.endsWith("/endpoints"))).toBe(false);
  });
});
