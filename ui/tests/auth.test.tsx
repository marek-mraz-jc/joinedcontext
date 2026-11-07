import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { AuthProvider, useAuth } from "../src/auth/AuthProvider";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer", "space-editor"],
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function problem(status: number): Response {
  return new Response(
    JSON.stringify({ type: "https://joinedcontext.com/errors/unauthorized", status }),
    { status, headers: { "Content-Type": "application/problem+json" } },
  );
}

function renderApp(client: QueryClient) {
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

function Probe() {
  const { status, roles, hasRole, signOut } = useAuth();
  return (
    <ul>
      <li data-testid="status">{status}</li>
      <li data-testid="roles">{roles.join(",")}</li>
      <li data-testid="editor">{hasRole("space-editor") ? "yes" : "no"}</li>
      <li data-testid="admin">{hasRole("city-admin") ? "yes" : "no"}</li>
      <li>
        <button type="button" onClick={() => void signOut()}>
          Sign out
        </button>
      </li>
    </ul>
  );
}

/** jsdom does not navigate; the assignment is what the test reads. */
function stubNavigation(): ReturnType<typeof vi.fn> {
  const assign = vi.fn();
  Object.defineProperty(window, "location", {
    configurable: true,
    value: { ...window.location, assign, origin: "http://localhost:3000", pathname: "/" },
  });
  return assign;
}

describe("authentication", () => {
  let client: QueryClient;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/");
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sends an unauthenticated visitor to the login route", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(problem(401)));

    renderApp(client);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Sign in" })).toBeInTheDocument();
    });
    expect(window.location.pathname).toBe("/login");
  });

  it("treats a failed session call as anonymous rather than authenticated", async () => {
    fetchMock.mockImplementation(() => Promise.reject(new Error("network down")));

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <AuthProvider>
            <Probe />
          </AuthProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId("status")).toHaveTextContent("anonymous");
    });
    expect(screen.getByTestId("editor")).toHaveTextContent("no");
  });

  it("exposes the realm roles Keycloak asserted and answers hasRole from them", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(json(IDENTITY)));

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <AuthProvider>
            <Probe />
          </AuthProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId("status")).toHaveTextContent("authenticated");
    });
    expect(screen.getByTestId("roles")).toHaveTextContent("portal-viewer,space-editor");
    expect(screen.getByTestId("editor")).toHaveTextContent("yes");
    expect(screen.getByTestId("admin")).toHaveTextContent("no");
  });

  // ADR-N-019, AP-29: behind the edge the logout ends at the plugin's `/logout`, after the
  // Portal's own cookies are cleared: an earlier code-flow login may have left them, and the
  // edge's logout never reaches the Portal.
  it("signs out through the Portal and then the edge's logout path when the session came from the edge", async () => {
    const assign = stubNavigation();
    fetchMock.mockImplementation((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      if (url.includes("/auth/logout")) {
        return Promise.resolve(json({ endSessionUrl: "https://idm.example.test/logout" }));
      }
      return Promise.resolve(json({ ...IDENTITY, front: "edge" }));
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <AuthProvider>
            <Probe />
          </AuthProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => {
      expect(screen.getByTestId("status")).toHaveTextContent("authenticated");
    });

    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledWith("/logout");
    });
    expect(assign).not.toHaveBeenCalledWith("https://idm.example.test/logout");
    const posted = fetchMock.mock.calls.filter((call: unknown[]) => {
      const input = call[0] as RequestInfo | URL;
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      return url.includes("/auth/logout");
    });
    expect(posted).toHaveLength(1);
    // Through the typed client (UI-07) the method is the Request's own.
    const [input, init] = posted[0] as [RequestInfo | URL, RequestInit | undefined];
    expect(input instanceof Request ? input.method : init?.method).toBe("POST");
  });

  it("signs out through the Portal and on to Keycloak when the session is the Portal's own", async () => {
    const assign = stubNavigation();
    fetchMock.mockImplementation((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      if (url.includes("/auth/logout")) {
        return Promise.resolve(json({ endSessionUrl: "https://idm.example.test/logout" }));
      }
      return Promise.resolve(json({ ...IDENTITY, front: "portal" }));
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <AuthProvider>
            <Probe />
          </AuthProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => {
      expect(screen.getByTestId("status")).toHaveTextContent("authenticated");
    });

    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledWith("https://idm.example.test/logout");
    });
  });

  it("renders the protected shell for a live session", async () => {
    fetchMock.mockImplementation((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      if (url.includes("/auth/me")) {
        return Promise.resolve(json(IDENTITY));
      }
      if (url.endsWith("/api/v1/projects")) {
        return Promise.resolve(
          json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "helsinki" }] }),
        );
      }
      return Promise.resolve(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
    });

    renderApp(client);

    await waitFor(() => {
      expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
    });
    expect(screen.getByRole("banner")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Sign in" })).not.toBeInTheDocument();
  });
});
