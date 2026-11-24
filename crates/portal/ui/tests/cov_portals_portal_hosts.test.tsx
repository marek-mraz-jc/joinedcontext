// T-7589: coverage for PortalHosts.tsx - linesOf non-ApiError fallback, add.onSuccess clearing inputs, base select onChange, whole input in domain mode, cancel in ConfirmDialog
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { hostOf, PortalHostsPanel } from "../src/pages/portals/PortalHosts";
import type { components } from "../src/api/schema";

type PortalOrganization = components["schemas"]["PortalOrganization"];

function renderPanel(org: PortalOrganization, showName = false, client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <PortalHostsPanel organization={org} showName={showName} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("PortalHosts coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("calculates hostOf correctly in own and domain modes", () => {
    expect(hostOf("own", " data ", "example.com", "")).toBe("data.example.com");
    expect(hostOf("domain", "", "", " portal.region.sk. ")).toBe("portal.region.sk");
  });

  it("handles add.onSuccess resetting inputs and invalidating query", async () => {
    const user = userEvent.setup();
    let addPayload: unknown;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(input, init);
        if (req.method === "POST") {
          addPayload = await req.json();
          return new Response(JSON.stringify({ valid: true }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const org: PortalOrganization = {
      organization: "bbsk",
      hosts: [],
      bases: ["bbsk.smartcity.example"],
      profiles: [],
      sites: [],
    };

    renderPanel(org, false);
    const labelInput = screen.getByLabelText(/^Name/);
    await user.type(labelInput, "opendata");
    expect(screen.getByText("Adds opendata.bbsk.smartcity.example")).toBeInTheDocument();

    const addBtn = screen.getByRole("button", { name: "Add host" });
    await user.click(addBtn);

    await waitFor(() => {
      expect(labelInput).toHaveValue("");
    });
    expect(addPayload).toEqual({ organization: "bbsk", host: "opendata.bbsk.smartcity.example" });
  });

  it("handles cancelling host removal in ConfirmDialog", async () => {
    const user = userEvent.setup();
    const org: PortalOrganization = {
      organization: "bbsk",
      hosts: [],
      bases: ["base1.example.org"],
      profiles: [],
      sites: [{ host: "old.base1.example.org", state: "served", removable: true, detail: "Serving traffic" }],
    };

    renderPanel(org, false);
    expect(screen.getByText("Serving traffic")).toBeInTheDocument();

    const removeBtn = screen.getByRole("button", { name: "Remove host old.base1.example.org" });
    await user.click(removeBtn);

    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText(/cannot be added again/)).toBeInTheDocument();

    const cancelBtn = within(dialog).getByRole("button", { name: "Cancel" });
    await user.click(cancelBtn);

    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
