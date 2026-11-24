// Hosts panel, remaining branches: choosing a second base, typing a whole host under the verified domain, a network failure.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { PortalHostsPanel } from "../src/pages/portals/PortalHosts";
import type { components } from "../src/api/schema";

type PortalOrganization = components["schemas"]["PortalOrganization"];

function renderPanel(org: PortalOrganization) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <PortalHostsPanel organization={org} showName={false} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

function recordPosts(posts: unknown[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      if (req.method === "POST") {
        posts.push(await req.json());
        return new Response(JSON.stringify({ valid: true }), { status: 200, headers: { "Content-Type": "application/json" } });
      }
      return new Response("{}", { status: 404 });
    }),
  );
}

describe("PortalHosts, more branches", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("adds a host under the second base the person picks", async () => {
    const user = userEvent.setup();
    const posts: unknown[] = [];
    recordPosts(posts);
    renderPanel({ organization: "bbsk", hosts: [], bases: ["a.example.org", "b.example.org"], profiles: [], sites: [] });
    await user.selectOptions(screen.getByLabelText(/^Under/), ".b.example.org");
    await user.type(screen.getByLabelText(/^Name/), "data");
    expect(screen.getByText("Adds data.b.example.org")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Add host" }));
    await waitFor(() => expect(posts).toEqual([{ organization: "bbsk", host: "data.b.example.org" }]));
  });

  it("adds a whole host name under the verified domain", async () => {
    const user = userEvent.setup();
    const posts: unknown[] = [];
    recordPosts(posts);
    renderPanel({ organization: "bbsk", hosts: [], bases: [], verifiedDomain: "bbsk.sk", profiles: [], sites: [] });
    await user.type(screen.getByLabelText(/^Host/), "Data.bbsk.sk");
    expect(screen.getByText("Adds data.bbsk.sk")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Add host" }));
    await waitFor(() => expect(posts).toEqual([{ organization: "bbsk", host: "data.bbsk.sk" }]));
    await waitFor(() => expect(screen.getByLabelText(/^Host/)).toHaveValue(""));
  });

  it("names the failure when the add never reaches the server", async () => {
    const user = userEvent.setup();
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));
    renderPanel({ organization: "bbsk", hosts: [], bases: ["a.example.org"], profiles: [], sites: [] });
    await user.type(screen.getByLabelText(/^Name/), "data");
    await user.click(screen.getByRole("button", { name: "Add host" }));
    expect(await screen.findByText("Failed to fetch")).toBeInTheDocument();
  });
});
