// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/access/ServiceAccounts.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { findFormPage } from "./formPage";
import { App } from "../src/App";
import { ServiceAccounts } from "../src/pages/access/ServiceAccounts";
import { expectNoAxeViolations, json, list, problem, renderPart } from "./page_contract";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const ACCOUNTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ServiceAccount",
      metadata: { name: "vendorx-parking-push", namespace: "banskabystrica" },
      spec: {
        owner: { user: "jana.kovacova" },
        purpose: "VendorX pushes ParkingSpot updates every 10 s",
        roles: [{ role: "space-writer", scope: { contextSpace: "parking" } }],
        credentials: [{ kind: "oauth-client", name: "main" }],
      },
    },
  ],
};

function renderAccess(
  options: {
    identity?: typeof IDENTITY;
    grants?: unknown[];
    accounts?: unknown;
  } = {},
) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    // The generated client hands over a `Request`; the gateway call is a plain `fetch(url)`.
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const href = request ? request.url : String(input);
    const path = new URL(href, window.location.origin).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(status === 204 ? null : JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(options.identity ?? IDENTITY);
    }
    if (path.endsWith("/permissions/me") && options.grants) {
      return json({ grants: options.grants });
    }
    if (path.endsWith("/serviceaccounts")) {
      return json(options.accounts ?? ACCOUNTS);
    }
    if (path === "/api/v1/projects/org/roles") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items: [
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Role",
            metadata: { name: "viewer", namespace: "org", title: { en: "Viewer" } },
            spec: {},
          },
        ],
      });
    }
    if (path.endsWith("/access")) {
      return json({ permissions: [], prohibitions: [] });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe("service accounts view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-value; Secure; path=/";
    window.history.pushState({}, "", "/projects/banskabystrica/settings/service-accounts");
    Object.assign(navigator, { clipboard: { writeText: vi.fn(() => Promise.resolve()) } });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists the account, its owner and its purpose", async () => {
    renderAccess();

    expect(await screen.findByText("vendorx-parking-push")).toBeInTheDocument();
    expect(screen.getByText(/VendorX pushes ParkingSpot updates/)).toBeInTheDocument();
    expect(screen.getByText("jana.kovacova", { selector: "dd" })).toBeInTheDocument();
  });

  // T-4616: an account signs in with its Keycloak client only, so even its owner is offered no
  // key, and the page reads none.
  it("offers its owner no key and reads none", async () => {
    const fetchMock = renderAccess();

    expect(await screen.findByText("vendorx-parking-push")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /API key/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(
      fetchMock.mock.calls.some((call) => String((call[0] as Request).url ?? call[0]).includes("/keys")),
    ).toBe(false);
  });
});

/**
 * The panel on its own (T-2137): what it shows before there is a list — the wait, the refusal in
 * the API's own words, and a project that holds no service account yet.
 */
describe("the service accounts panel, mounted on its own", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  const mounted = (answer: (url: URL) => Response | undefined) =>
    renderPart(<ServiceAccounts project="helsinki" />, { answer });

  it("announces the wait instead of an empty heading", () => {
    mounted(() => undefined as never);
    expect(screen.getByRole("status")).toHaveAttribute("aria-busy", "true");
  });

  it("says what the API said when the list is refused, with a way to ask again", async () => {
    mounted((url) =>
      url.pathname.endsWith("/serviceaccounts")
        ? problem(403, "You may not read the service accounts of helsinki.")
        : undefined,
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("You may not read the service accounts of helsinki.");
    // A refusal is not asked again: the same question gets the same answer (T-3244, T-3300).
    expect(within(alert).queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });

  it("offers Retry when the list could not be read, and reads it again", async () => {
    let asked = 0;
    mounted((url) => {
      if (!url.pathname.endsWith("/serviceaccounts")) return undefined;
      asked += 1;
      return asked === 1 ? problem(503, "The configuration store did not answer.") : json(list([]));
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The configuration store did not answer.");
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(en.access.accounts.empty)).toBeInTheDocument();
  });

  it("says the project holds none yet, and asks for no key while it holds none", async () => {
    const seen: string[] = [];
    mounted((url) => {
      seen.push(url.pathname);
      return url.pathname.endsWith("/serviceaccounts") ? json(list([])) : undefined;
    });
    expect(await screen.findByText(en.access.accounts.empty)).toBeInTheDocument();
    expect(seen.some((path) => path.includes("/keys")), "no key is read for an empty list").toBe(false);
  });

  // T-2758: the role was a box to type a role's name into from memory.
  it("picks the role from the organization's roles and the ones accounts hold", async () => {
    renderAccess();
    const user = userEvent.setup();
    await screen.findByText("vendorx-parking-push");
    await user.click(screen.getAllByRole("button", { name: en.access.accounts.add })[0]);
    const dialog = await findFormPage();
    const role = await within(dialog).findByRole("combobox", { name: /^Role/ });
    await waitFor(() =>
      expect(within(role).getByRole("option", { name: "Viewer" })).toBeInTheDocument(),
    );
    // Held by the listed account though no list names it: an edit must not lose it.
    expect(within(role).getByRole("option", { name: "space-writer" })).toBeInTheDocument();
    // F-R2UI2-013: the new account starts owned by its creator's Keycloak id, as the owner
    // picker writes it, never by their e-mail (ADR-N-061).
    expect(within(dialog).getByRole("combobox", { name: /^Owner/ })).toHaveValue(IDENTITY.subject);
  });

  // T-2759: "space-reader, space-reader".
  it("names where each grant holds", async () => {
    renderAccess({
      accounts: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items: [
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "ServiceAccount",
            metadata: { name: "hub-reader", namespace: "banskabystrica" },
            spec: {
              owner: { user: "jana.kovacova" },
              purpose: "Reads the hub",
              roles: [
                { role: "space-reader", scope: { contextSpace: "helsinki" } },
                { role: "space-reader", scope: { contextSpace: "helsinki-kpi" } },
              ],
              credentials: [{ kind: "oauth-client", name: "main" }],
            },
          },
        ],
      },
    });
    expect(
      await screen.findByText("space-reader in helsinki, space-reader in helsinki-kpi"),
    ).toBeInTheDocument();
  });

  it("has no axe violation", async () => {
    const { container } = renderPart(<ServiceAccounts project="helsinki" />, {
      answer: (url) => (url.pathname.endsWith("/serviceaccounts") ? json(list([])) : undefined),
    });
    await screen.findByText(en.access.accounts.empty);
    await expectNoAxeViolations(container);
  });
});
