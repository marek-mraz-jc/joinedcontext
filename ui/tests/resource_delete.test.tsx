import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { expectDenied, expectOpen } from "./checks";

const PROJECT = "banskabystrica";
const NAME = "zvolen-ovzdusie";

const REGISTRATIONS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSourceRegistration",
      metadata: { name: NAME, namespace: PROJECT, title: { en: "Zvolen air quality" } },
      spec: { contextSpaceRef: "hub" },
      status: { phase: "Live" },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002a", namespace: PROJECT },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 0, delete: 1 } },
};

const may = (verbs: string[]) => ({
  grants: [{ rule: { kinds: ["ContextSourceRegistration"], verbs } }],
});

function renderList(options: { verbs: string[]; answer?: "change" | "referenced" | "open" }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200, type = "application/json") =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": type } }));
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "jana.kovacova", roles: [] });
    }
    if (path.endsWith("/permissions/me")) {
      return json(may(options.verbs));
    }
    if (request.method === "DELETE" && options.answer === "open") {
      // Nothing references it, but a removal proposed earlier still waits for its decision.
      return new URL(request.url).searchParams.has("dryRun")
        ? json({ goesWith: [] })
        : json(
            {
              type: "https://joinedcontext.com/errors/conflict",
              title: "Conflict",
              status: 409,
              detail: "a change for Endpoint 'live-traffic' is already open: chg-000000d0; approve or reject it first",
            },
            409,
            "application/problem+json",
          );
    }
    if (request.method === "DELETE") {
      return options.answer === "referenced"
        ? json(
            {
              type: "https://joinedcontext.com/errors/conflict",
              title: "Conflict",
              status: 409,
              detail: "1 dependent resource blocks deletion: Endpoint live-traffic",
            },
            409,
            "application/problem+json",
          )
        : json(CHANGE, 202);
    }
    if (path.endsWith("/csrs")) {
      return json(REGISTRATIONS);
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

// The removals proposed: the dialog's own dry run, which says what goes with the resource
// (T-3247), removes nothing and is not one.
const deletes = (fetchMock: ReturnType<typeof vi.fn>) =>
  fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method === "DELETE" && !new URL(request.url).searchParams.has("dryRun"));

describe("removing a resource from its list", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-value; Secure; path=/";
    window.history.pushState({}, "", `/projects/${PROJECT}/csrs`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

/** The row's actions live behind its one menu now (T-2287): open it and hand back the item. */
async function rowMenuItem(name: string | RegExp) {
  await userEvent.click(await screen.findByRole("button", { name: /More actions/ }));
  return screen.findByRole("menuitem", { name });
}

  it("keeps Delete disabled with the reason for a person whose role may not delete the kind (UI-44)", async () => {
    renderList({ verbs: ["propose"] });
    expect(await screen.findByText("Zvolen air quality")).toBeInTheDocument();
    const remove = await rowMenuItem(new RegExp(en.resourceDelete.button));
    expect(remove).toHaveAttribute("aria-disabled", "true");
    expect(remove.getAttribute("title")).toMatch(/delete/);
  });

  it("proposes the removal only once the name is typed back, and shows the change", async () => {
    const fetchMock = renderList({ verbs: ["propose", "delete"] });
    await userEvent.click(await rowMenuItem(new RegExp(en.resourceDelete.button)));

    const dialog = await screen.findByRole("dialog");
    const propose = within(dialog).getByRole("button", { name: en.resourceDelete.propose });
    // Refused until the name matches, and reachable while it is refused, so the reason can be
    // read (T-1743, T-1751): `toBeDisabled` stopped meaning "closed" when the reason arrived.
    expectDenied(propose, `Type ${NAME} to propose its removal.`);
    const field = within(dialog).getByLabelText(`Type ${NAME} to confirm`);
    await userEvent.type(field, "zvolen-ovzdusi");
    expectDenied(propose);
    await userEvent.type(field, "e");
    expectOpen(propose);

    await userEvent.click(propose);
    expect(await within(dialog).findByText(CHANGE.metadata.name)).toBeInTheDocument();
    expect(within(dialog).getByText(en.changes.accepted)).toBeInTheDocument();
    const sent = deletes(fetchMock);
    expect(sent).toHaveLength(1);
    expect(new URL(sent[0].url).pathname).toBe(`/api/v1/projects/${PROJECT}/csrs/${NAME}`);
    expect(sent[0].headers.get("x-csrf-token")).toBe("csrf-token-value");
  });

  /// T-1054: the dialog exists to have a name typed into it, so that is where the caret goes.
  it("opens with the caret in the field the dialog is for", async () => {
    renderList({ verbs: ["propose", "delete"] });
    await userEvent.click(await rowMenuItem(new RegExp(en.resourceDelete.button)));

    const dialog = await screen.findByRole("dialog");
    const field = within(dialog).getByLabelText(`Type ${NAME} to confirm`);
    await waitFor(() => {
      expect(document.activeElement).toBe(field);
    });
  });

  it("names what still references the resource when the removal is refused", async () => {
    renderList({ verbs: ["delete"], answer: "referenced" });
    await userEvent.click(await rowMenuItem(new RegExp(en.resourceDelete.button)));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(`Type ${NAME} to confirm`), NAME);
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceDelete.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(en.resourceDelete.referenced);
    expect(alert).toHaveTextContent("Endpoint live-traffic");
  });

  it("says a removal is already open without calling the resource still in use (T-5066)", async () => {
    renderList({ verbs: ["delete"], answer: "open" });
    await userEvent.click(await rowMenuItem(new RegExp(en.resourceDelete.button)));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(`Type ${NAME} to confirm`), NAME);
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceDelete.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("already open: chg-000000d0; approve or reject it first");
    expect(alert).not.toHaveTextContent(en.resourceDelete.referenced);
  });

  it("opens the confirmation at once for a page opened to remove one resource", async () => {
    window.history.pushState({}, "", `/projects/${PROJECT}/csrs?delete=${NAME}`);
    const fetchMock = renderList({ verbs: ["delete"] });
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Remove Zvolen air quality");
    expect(deletes(fetchMock)).toHaveLength(0);
  });
});
