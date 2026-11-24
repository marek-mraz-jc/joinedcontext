// T-7706: Try a person edges: endpoint, person search, service accounts, public, action choice, a refused ask and Retry.
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { asOData, odataCall } from "./odata";

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const endpoint = (name: string, slug: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki" },
  spec: { slug },
});
const account = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ServiceAccount",
  metadata: { name, namespace: "helsinki" },
  spec: {},
});
const person = (id: string, email: string) => ({ id, email, firstName: id, lastName: "K", enabled: true, emailVerified: true });
const ADMIN_GRANT = { role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: ["RoleBinding"], verbs: ["approve", "delete"] } };

interface Sent {
  path: string;
  body: unknown;
  search: string;
}

function stub({ accounts = [], failFirst = false }: { accounts?: unknown[]; failFirst?: boolean } = {}): Sent[] {
  const sent: Sent[] = [];
  let asked = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const text = request.method === "POST" ? await request.text() : "";
      const called = odataCall(url);
      sent.push({ path: called ? called.name : url.pathname, body: called ? called.params : text ? JSON.parse(text) : undefined, search: url.search });
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "anna", roles: [] });
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
      if (url.pathname.endsWith("/permissions/me")) return json({ project: "org", bootstrap: false, grants: [ADMIN_GRANT] });
      if (url.pathname === "/api/v1/projects/helsinki/endpoints") return json(LIST([endpoint("air", "air-quality"), endpoint("noise", "noise-levels")]));
      if (url.pathname === "/api/v1/projects/helsinki/serviceaccounts") return json(LIST(accounts));
      if (url.pathname === "/api/v1/organization/people") return json({ items: [person("jana-id", "jana@hel.fi"), person("petr-id", "petr@hel.fi")], total: 2 });
      if (called?.name === "jc_access_simulate") {
        asked += 1;
        return failFirst && asked === 1
          ? asOData(json({ title: "Unavailable", status: 503, detail: "the gateway did not answer" }, 503))
          : asOData(json({ decision: true, reason: "policy_grant_matched", policy: "air-read", subject: { groups: [], roles: [] } }));
      }
      return json(LIST([]));
    }),
  );
  return sent;
}

async function panel(): Promise<HTMLElement> {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  const heading = await screen.findByRole("heading", { name: "Try a person" });
  return heading.closest("div.rounded-lg") as HTMLElement;
}

const simulated = (sent: Sent[]) => sent.filter((s) => s.path === "jc_access_simulate").map((s) => s.body);

beforeEach(async () => {
  await i18n.changeLanguage("en");
  window.history.pushState({}, "", "/projects/helsinki/policies");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Try a person, the choices", () => {
  it("asks on the second endpoint about the second person found by a search, for another action", async () => {
    const user = userEvent.setup();
    const sent = stub();
    const box = await panel();
    await within(box).findByRole("option", { name: "jana@hel.fi" });
    await user.selectOptions(within(box).getByLabelText(/^Endpoint/), "noise");
    await user.type(within(box).getByRole("searchbox", { name: /Find a person/ }), "petr");
    await waitFor(() => expect(sent.some((s) => s.path === "/api/v1/organization/people" && s.search.includes("petr"))).toBe(true));
    await user.selectOptions(within(box).getByLabelText(/^Person/), "petr-id");
    await user.selectOptions(within(box).getByLabelText(/^Action/), "queryEntity");
    await user.click(within(box).getByRole("button", { name: "Ask the gateway" }));
    expect(await screen.findByTestId("try-verdict")).toHaveTextContent("Allowed");
    expect(simulated(sent)).toEqual([{ subject: { kind: "person", id: "petr-id" }, action: "queryEntity", endpoint: "noise" }]);
  });

  it("asks about a service account picked from the project's, and about the public", async () => {
    const user = userEvent.setup();
    const sent = stub({ accounts: [account("feed-a"), account("feed-b")] });
    const box = await panel();
    await within(box).findByRole("option", { name: "air" });
    await user.selectOptions(within(box).getByLabelText(/^Who/), "serviceAccount");
    await within(box).findByRole("option", { name: "feed-b" });
    await user.selectOptions(within(box).getByLabelText(/^A service account/), "feed-b");
    await user.click(within(box).getByRole("button", { name: "Ask the gateway" }));
    await screen.findByTestId("try-verdict");
    await user.selectOptions(within(box).getByLabelText(/^Who/), "public");
    await user.click(within(box).getByRole("button", { name: "Ask the gateway" }));
    await waitFor(() => expect(simulated(sent)).toHaveLength(2));
    expect(simulated(sent)).toEqual([
      { subject: { kind: "serviceAccount", name: "feed-b" }, action: "retrieveEntity", endpoint: "air" },
      { subject: { kind: "public" }, action: "retrieveEntity", endpoint: "air" },
    ]);
  });

  it("defaults to the first service account and says so when the project has none", async () => {
    const user = userEvent.setup();
    const sent = stub({ accounts: [account("feed-a")] });
    const box = await panel();
    await within(box).findByRole("option", { name: "air" });
    await user.selectOptions(within(box).getByLabelText(/^Who/), "serviceAccount");
    await within(box).findByRole("option", { name: "feed-a" });
    await user.click(within(box).getByRole("button", { name: "Ask the gateway" }));
    await screen.findByTestId("try-verdict");
    expect(simulated(sent)[0]).toMatchObject({ subject: { kind: "serviceAccount", name: "feed-a" } });
    cleanup();
    stub();
    const empty = await panel();
    await within(empty).findByRole("option", { name: "air" });
    await user.selectOptions(within(empty).getByLabelText(/^Who/), "serviceAccount");
    expect(await within(empty).findByRole("option", { name: "No service account yet" })).toBeInTheDocument();
    expect(within(empty).getByRole("button", { name: "Ask the gateway" })).toHaveAttribute("aria-disabled", "true");
  });

  it("sends nothing when the form is submitted with nobody picked", async () => {
    const user = userEvent.setup();
    const sent = stub();
    const box = await panel();
    await within(box).findByRole("option", { name: "air" });
    await user.selectOptions(within(box).getByLabelText(/^Who/), "group");
    fireEvent.submit(within(box).getByRole("button", { name: "Ask the gateway" }).closest("form") as HTMLFormElement);
    expect(simulated(sent)).toEqual([]);
  });
});

describe("Try a person, a failed ask", () => {
  it("says the gateway could not be asked and asks again on Retry", async () => {
    const user = userEvent.setup();
    const sent = stub({ failFirst: true });
    const box = await panel();
    await within(box).findByRole("option", { name: "jana@hel.fi" });
    await user.click(within(box).getByRole("button", { name: "Ask the gateway" }));
    expect(await within(box).findByText("The gateway could not be asked. The reason and Retry are below.")).toBeInTheDocument();
    expect(within(box).getByRole("alert")).toHaveTextContent("the gateway did not answer");
    await user.click(within(box).getByRole("button", { name: "Retry" }));
    expect(await screen.findByTestId("try-verdict")).toHaveTextContent("Allowed");
    expect(simulated(sent)).toHaveLength(2);
    expect(simulated(sent)[1]).toEqual(simulated(sent)[0]);
  });
});
