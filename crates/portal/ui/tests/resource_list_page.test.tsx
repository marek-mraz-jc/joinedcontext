/**
 * UI-01, UI-15, UI-16 (T-2137; MF-11…MF-15): `/projects/{project}/{plural}` — a kind's own page,
 * or the table every other kind falls through to.
 *
 * The kind pages have their own tests, so they are stubbed here and this file holds the two
 * things only this route decides: which of them a plural opens, and what a kind with no page of
 * its own gets — a heading in the person's own language rather than the URL segment, the
 * project's manifests in a table, one menu per row, and the empty, waiting and refused states.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { expectHeadingOutline, expectNoAxeViolations, json, list, problem, renderPage } from "./page_contract";

vi.mock("../src/routes/SpacesPage", () => ({
  SpacesPage: ({ project }: { project: string }) => <p>the spaces page of {project}</p>,
}));

const { ResourceListPage } = await import("../src/routes/ResourceListPage");

function manifest(name: string, over: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Role",
    metadata: { name, namespace: "helsinki" },
    status: { phase: "Ready" },
    ...over,
  };
}

function show(plural: string, answer?: (url: URL) => Response | undefined) {
  return renderPage(<ResourceListPage project="helsinki" plural={plural} />, {
    answer: (url) =>
      answer?.(url) ??
      (url.pathname.endsWith(`/${plural}`)
        ? json(list([manifest("steward"), manifest("editor", { metadata: { name: "editor", namespace: "helsinki", title: { en: "Editor of the city", sk: "Editor mesta" } } })]))
        : undefined),
  });
}

describe("a kind's list page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("opens the kind's own page where there is one", async () => {
    show("spaces");
    expect(await screen.findByText("the spaces page of helsinki")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("names a kind with no page of its own in words, never as the URL segment", async () => {
    const { container } = show("roles");
    const heading = await screen.findByRole("heading", { level: 1 });
    // `nav.roles` is not in the catalogue, so the segment is read as words instead of shown raw.
    expect(heading).toHaveTextContent("Roles");
    expect(heading.textContent).not.toBe("roles");
    expectHeadingOutline(container);
  });

  it("lists what the project holds, each row with its own menu", async () => {
    show("roles");
    expect(await screen.findByText("steward")).toBeInTheDocument();
    // A titled manifest reads as its title, with the name it is addressed by underneath.
    expect(screen.getByText("Editor of the city")).toBeInTheDocument();
    expect(screen.getByText("editor")).toBeInTheDocument();
    expect(await screen.findAllByRole("button", { name: /More actions/ })).toHaveLength(2);
  });

  it("reads a title in the person's own language", async () => {
    await i18n.changeLanguage("sk");
    show("roles");
    expect(await screen.findByText("Editor mesta")).toBeInTheDocument();
    await i18n.changeLanguage("en");
  });

  it("says the project holds nothing of this kind rather than showing an empty table", async () => {
    show("roles", (url) => (url.pathname.endsWith("/roles") ? json(list([])) : undefined));
    expect(await screen.findByText(en.resourceList.empty)).toBeInTheDocument();
    expect(screen.getByText(en.resourceList.emptyHint)).toBeInTheDocument();
  });

  it("names no create action on a page that has none (T-2488)", async () => {
    show("uischemas", (url) => (url.pathname.endsWith("/uischemas") ? json(list([])) : undefined));
    // A form's arrangement ships with the UI and is changed in the organization's repository
    // (UI-02, API/01 §8a), so the project page lists and reads, and says where the change is made.
    expect(await screen.findByText(en.resourceList.emptyHintFor.uischemas)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /new|add|create/i })).toBeNull();
    await i18n.loadLanguages(["sk", "cs", "de"]);
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      expect(bundle.resourceList.emptyHintFor.uischemas, locale).toContain("portal/forms/");
      // The generic list has no New button either, so its hint promises none.
      expect(bundle.resourceList.emptyHint, locale).not.toMatch(
        /Add the first|Pridajte prvé|Přidejte první|Legen Sie das erste/,
      );
    }
  });

  it("says why the list could not be read, in the API's own words, with a way to ask again", async () => {
    show("roles", (url) =>
      url.pathname.endsWith("/roles") ? problem(403, "You may not read the roles of helsinki.") : undefined,
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("You may not read the roles of helsinki.");
    expect(alert).toHaveTextContent(en.app.error.forbiddenHint);
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });

  it("offers to ask again after a server failure, and asks", async () => {
    let failing = true;
    show("roles", (url) => (url.pathname.endsWith("/roles") && failing ? problem(500, "The store is away.") : undefined));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The store is away.");
    failing = false;
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });

  it("announces the wait rather than showing an empty table", async () => {
    renderPage(<ResourceListPage project="helsinki" plural="roles" />, {
      answer: (url) =>
        url.pathname.endsWith("/roles") ? new Promise<Response>(() => undefined) as never : undefined,
    });
    expect(await screen.findByText(en.app.loading)).toBeInTheDocument();
  });

  it("has no axe violation", async () => {
    const { container } = show("roles");
    await screen.findByText("steward");
    await waitFor(() => expect(screen.getAllByRole("row").length).toBeGreaterThan(1));
    await expectNoAxeViolations(container);
  });
});
