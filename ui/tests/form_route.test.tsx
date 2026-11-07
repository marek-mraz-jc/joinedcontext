/**
 * T-2474, UI-27: a bigger form is a page with its own address, not a popup.
 *
 * `/projects/{project}/{plural}/new` and `/projects/{project}/{plural}/{name}/edit` open the
 * kind's form in the list's place. The address survives a reload, the browser's back button
 * leaves the form, and leaving it by its own controls leaves the address too. The same form,
 * rendered where no route hosts it, is still the dialog it was.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { FormFrame, FormHeading } from "../src/components/forms/FormRoute";
import { answeringChecks } from "./checks";
import { findFormPage, queryFormPage } from "./formPage";
import { jsonResponse, list, renderRoute } from "./pageHarness";

const LIST = "/projects/helsinki/policies";

const POLICY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name: "open-read", namespace: "helsinki" },
  spec: {
    contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
    assigner: "did:web:hel.fi",
    assignee: { kind: "role", id: "public" },
    operations: ["retrieveOps"],
  },
};

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "ovzdusie", namespace: "helsinki" },
  spec: {},
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-000024a4", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
};

function answer(path: string, request: Request): Response | undefined {
  if (request.method === "PUT") return jsonResponse(CHANGE, 202);
  if (path.endsWith("/policies")) return jsonResponse(list([POLICY]));
  if (path.endsWith("/spaces")) return jsonResponse(list([SPACE]));
  if (path.endsWith("/policies/open-read")) return jsonResponse(POLICY);
  return undefined;
}

const open = (path: string) => renderRoute({ path, answer });

/** The back control of the open form page. */
const back = (page: HTMLElement) => within(page).getByRole("button", { name: en.form.cancel });

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("a kind's forms at their own addresses", () => {
  // TS-19, UI-27: the host is one block of the page's column. An empty slot beside the list was a
  // second flex child and pushed the page down by one section gap (T-2489, the endpoints goldens).
  it("adds no empty block to the page's column while the list shows", async () => {
    await open(LIST);
    await screen.findByRole("heading", { level: 1, name: en.policies.title });

    const column = document.querySelector("#main > div");
    expect(column).not.toBeNull();
    const empty = [...(column?.children ?? [])].filter((child) => child.childNodes.length === 0);
    expect(empty).toEqual([]);
  });

  it("opens the create form at /new, in the list's place, and goes back to the list", async () => {
    await open(`${LIST}/new`);

    const page = await findFormPage(en.policies.add);
    expect(within(page).getByRole("heading", { level: 1, name: en.policies.add })).toBeInTheDocument();
    // The list is behind the form, out of the accessibility tree.
    expect(screen.queryByRole("heading", { level: 1, name: en.policies.title })).toBeNull();

    await userEvent.click(back(page));
    await waitFor(() => expect(window.location.pathname).toBe(LIST));
    // The address changes a render before the form leaves: wait for it to go, it must go.
    await waitFor(() => expect(queryFormPage()).toBeNull());
    expect(await screen.findByRole("heading", { level: 1, name: en.policies.title })).toBeInTheDocument();
  });

  // T-2577, AG-73: the assistant opens a kind it does not draft at its /new address; a
  // ServiceAccount is created on the access page, and that address opens its form too.
  it("opens the service account form at /settings/service-accounts/new", async () => {
    await open("/projects/helsinki/settings/service-accounts/new");
    expect(await findFormPage(en.access.accounts.add)).toBeInTheDocument();
  });

  // T-2582: a SyncSource is created in the Sync page's dialog, and its /new address opens it.
  it("opens the sync source form at /syncsources/new", async () => {
    await open("/projects/helsinki/syncsources/new");
    expect(await findFormPage(en.syncSources.dialog.title)).toBeInTheDocument();
  });

  it("moves the address when the list's own button opens the form, and the back button leaves it", async () => {
    await open(LIST);

    await userEvent.click(await screen.findByRole("button", { name: en.policies.add }));
    await waitFor(() => expect(window.location.pathname).toBe(`${LIST}/new`));
    await findFormPage(en.policies.add);

    act(() => window.history.back());
    await waitFor(() => expect(window.location.pathname).toBe(LIST));
    await waitFor(() => expect(queryFormPage()).toBeNull());
    expect(await screen.findByRole("button", { name: en.policies.add })).toBeVisible();
  });

  it("opens the same edit form again after a reload of its address", async () => {
    const first = await open(`${LIST}/open-read/edit`);
    const title = en.resourceEdit.title.replace("{name}", "open-read");
    expect(await findFormPage(title)).toBeInTheDocument();
    first.unmount();

    // A reload is a fresh Portal on the same address.
    await open(`${LIST}/open-read/edit`);
    const page = await findFormPage(title);
    expect(window.location.pathname).toBe(`${LIST}/open-read/edit`);
    expect(await within(page).findByDisplayValue("open-read")).toBeInTheDocument();
  });

  it("saves from a deep-linked edit form and lands on the list, with the change it opened", async () => {
    const { fetchMock } = await open(`${LIST}/open-read/edit`);
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
    const page = await findFormPage(en.resourceEdit.title.replace("{name}", "open-read"));
    await within(page).findByDisplayValue("open-read");

    await userEvent.click(within(page).getByRole("button", { name: en.resourceEdit.propose }));

    await waitFor(() => expect(window.location.pathname).toBe(LIST));
    expect(await screen.findByText(CHANGE.metadata.name)).toBeInTheDocument();
    expect(queryFormPage()).toBeNull();
    expect(screen.getByRole("heading", { level: 1, name: en.policies.title })).toBeVisible();
    const puts = fetchMock.mock.calls.filter((call) => (call[0] as Request).method === "PUT");
    expect(puts).toHaveLength(1);
    expect(new URL((puts[0][0] as Request).url).pathname).toBe("/api/v1/projects/helsinki/policies/open-read");
    // The address carried the name and nothing else: what was saved came from the form.
    expect(window.location.search).toBe("");
  });

  // T-3423: the editor registers in an effect, after the page's reads have answered; for one render
  // the page took that for "nothing opened it" and showed the alert, which a screen reader announces
  // and a slow CI run caught (row_link, ci-full 37830651396).
  it("never says a form cannot be opened on the way to opening it", async () => {
    const said: string[] = [];
    const observer = new MutationObserver(() => {
      if (document.body.textContent?.includes("This form cannot be opened")) said.push(document.body.textContent);
    });
    observer.observe(document.body, { childList: true, subtree: true, characterData: true });
    try {
      await open(`${LIST}/open-read/edit`);
      const page = await findFormPage(en.resourceEdit.title.replace("{name}", "open-read"));
      expect(await within(page).findByDisplayValue("open-read")).toBeInTheDocument();
      expect(said).toEqual([]);
    } finally {
      observer.disconnect();
    }
  });

  // T-3423 follow-up: the form page is committed a moment before it registers (a passive effect,
  // after paint); in that moment nothing hid the list behind it, its heading and the alert in the
  // accessibility tree (form_route.test.tsx "opens the create form" on a loaded CI runner).
  it.each([["the create form", `${LIST}/new`], ["an edit form", `${LIST}/open-read/edit`]])(
    "never shows %s and the list at once",
    async (_, address) => {
      const both: string[] = [];
      const listShown = () =>
        Array.from(document.querySelectorAll("h1")).some(
          (h1) => h1.textContent === en.policies.title && h1.closest("[hidden]") === null,
        );
      const observer = new MutationObserver(() => {
        if (document.querySelector('[data-testid="form-page"]') && listShown()) both.push(address);
      });
      observer.observe(document.body, { childList: true, subtree: true, attributes: true });
      try {
        await open(address);
        await findFormPage();
        await waitFor(() => expect(listShown()).toBe(false));
        expect(both).toEqual([]);
      } finally {
        observer.disconnect();
      }
    },
  );

  it("says so when the address names something the list does not hold, with a way back", async () => {
    await open(`${LIST}/no-such-policy/edit`);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(en.form.notOpen.replace("{name}", "no-such-policy"));
    // The page keeps its heading and its list under the warning: an unheaded page with one alert
    // was all a person had (T-2730, UI-16), and what does exist is the next thing they need.
    expect(screen.getByRole("heading", { level: 1, name: en.policies.title })).toBeVisible();
    expect(screen.getByText("open-read")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: en.form.backToList }));
    await waitFor(() => expect(window.location.pathname).toBe(LIST));
    expect(await screen.findByRole("heading", { level: 1, name: en.policies.title })).toBeInTheDocument();
  });

  // T-6525, UI-16: a shell read refreshes data the page already holds a moment after every load (the
  // Endpoint list on dev). The whole page went behind the skeleton for that refresh, its h1 with it.
  it("keeps the page and its heading while a read it already holds refreshes", async () => {
    let release = () => {};
    let held = false;
    const rendered = await renderRoute({
      path: `${LIST}/no-such-policy/edit`,
      answer: (path, request) => {
        if (held && path.endsWith("/policies")) {
          return new Promise<Response>((resolve) => {
            release = () => resolve(jsonResponse(list([POLICY])));
          });
        }
        return answer(path, request);
      },
    });
    await screen.findByRole("alert");

    held = true;
    await act(async () => {
      void rendered.client.invalidateQueries();
      await new Promise((resolve) => setTimeout(resolve, 20));
    });

    expect(screen.getByRole("heading", { level: 1, name: en.policies.title })).toBeVisible();
    expect(screen.getByRole("alert")).toHaveTextContent(en.form.notOpen.replace("{name}", "no-such-policy"));
    expect(screen.queryByText(en.form.opening)).not.toBeInTheDocument();
    await act(async () => release());
  });

  // T-3241, UI-91: every item has its bare address; a kind with no page of its own opens its form.
  it("opens a kind with no page of its own on its form at the item's bare address", async () => {
    await open(`${LIST}/open-read`);
    await waitFor(() => expect(window.location.pathname).toBe(`${LIST}/open-read/edit`));
    expect(screen.queryByText(en.app.notFound.title)).toBeNull();
  });
});

describe("the form's frame where no route hosts it", () => {
  it("is a dialog, with its section headings one level under the dialog's title", async () => {
    await i18n.changeLanguage("en");
    render(
      <I18nextProvider i18n={i18n}>
        <FormFrame open onOpenChange={() => undefined} title="Alone" closeLabel="Close">
          <FormHeading>Section</FormHeading>
          <FormHeading sub>Part</FormHeading>
        </FormFrame>
      </I18nextProvider>,
    );

    const dialog = await screen.findByRole("dialog", { name: "Alone" });
    expect(within(dialog).getByRole("heading", { level: 3, name: "Section" })).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { level: 4, name: "Part" })).toBeInTheDocument();
    expect(queryFormPage()).toBeNull();
  });
});
