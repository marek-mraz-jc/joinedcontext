/**
 * T-5626, PF-37: a ServiceAccount's client secret. An approver issues a new one behind a
 * confirmation, sees it once with its client id and token URL, and Hide forgets it; a person who
 * may not issue is told who does; an account with a workload or a key pair says why it has none.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { AccountSecret } from "../src/pages/access/AccountSecret";

const ISSUED = {
  clientId: "helsinki-vendor",
  clientSecret: "s3cret-shown-once",
  tokenUrl: "https://idm.example/realms/dev/protocol/openid-connect/token",
};

function stub(access: Record<string, unknown>, issue: { status: number; body: unknown } = { status: 200, body: ISSUED }) {
  const sent: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      sent.push(`${request.method} ${url.pathname}`);
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/serviceaccounts/vendor/access")) {
        return json({ account: "vendor", project: "helsinki", delegates: false, mayPropose: true, mayRevoke: true, grants: [], ...access });
      }
      if (url.pathname.endsWith("/serviceaccounts/vendor/secret")) return json(issue.body, issue.status);
      return json({ items: [] });
    }),
  );
  return sent;
}

function show(): QueryClient {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <AccountSecret project="helsinki" account="vendor" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return client;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("AccountSecret (PF-37, T-5626)", () => {
  it("issues a secret after the confirmation, shows it once and forgets it on Done", async () => {
    const sent = stub({ noSecret: null, mayIssueSecret: true });
    const client = show();
    expect(await screen.findByText(/signs in as client helsinki-vendor/)).toBeInTheDocument();
    expect(screen.queryByDisplayValue(ISSUED.clientSecret)).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Issue new secret" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Issue a new secret for vendor?")).toBeInTheDocument();
    expect(sent.filter((call) => call.startsWith("POST"))).toEqual([]);
    await userEvent.click(within(dialog).getByRole("button", { name: "Issue new secret" }));

    expect(await screen.findByLabelText("Client secret")).toHaveValue(ISSUED.clientSecret);
    expect(screen.getByLabelText("Client ID")).toHaveValue("helsinki-vendor");
    expect(screen.getByLabelText("Token URL")).toHaveValue(ISSUED.tokenUrl);
    expect(screen.getByText("Copy the secret now")).toBeInTheDocument();
    expect(sent.filter((call) => call.startsWith("POST"))).toEqual(["POST /api/v1/projects/helsinki/serviceaccounts/vendor/secret"]);

    await userEvent.click(screen.getByRole("button", { name: "Hide the secret" }));
    await waitFor(() => expect(screen.queryByDisplayValue(ISSUED.clientSecret)).toBeNull());
    // F-R2UI2-012: forgotten in the mutation cache too, not only off the screen.
    await waitFor(() =>
      expect(JSON.stringify(client.getMutationCache().getAll().map((mutation) => mutation.state.data))).not.toContain(
        ISSUED.clientSecret,
      ),
    );
  });

  it("says what failed when the Portal refuses, and shows no secret", async () => {
    stub(
      { noSecret: null, mayIssueSecret: true },
      { status: 409, body: { type: "about:blank", title: "Conflict", status: 409, detail: "client helsinki-vendor does not exist yet; the Portal creates it within a minute of the account's merge, then try again" } },
    );
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Issue new secret" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Issue new secret" }));
    expect(await screen.findByText(/does not exist yet/)).toBeInTheDocument();
    expect(screen.queryByLabelText("Client secret")).toBeNull();
  });

  it("tells a person who may not issue who does", async () => {
    stub({ noSecret: null, mayIssueSecret: false });
    show();
    expect(await screen.findByText("A person who may approve service accounts issues its secret.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Issue new secret" })).toBeNull();
  });

  it.each([
    ["workload", /pod's own token/],
    ["key-pair", /own key pair/],
    ["no-credential", /add one, for example named main/],
  ])("explains why an account with %s has no secret", async (noSecret, says) => {
    stub({ noSecret, mayIssueSecret: true });
    show();
    expect(await screen.findByText(says)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Issue new secret" })).toBeNull();
  });
});
