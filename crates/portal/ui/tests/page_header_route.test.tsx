/**
 * T-1777…T-1787: a page says where you are, to the tab and to a screen reader (UI-15, UI-16).
 *
 * Every page of the Portal was titled "joinedcontext" in the tab, the history and the task
 * switcher, and a route change left focus on the link that was pressed, so a screen reader said
 * nothing about the page that had arrived. `PageHeader` is on every page and knows the title, so
 * it is the one place that fixes both.
 */
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { PageHeader } from "../src/components/ui";
import { applyBranding, NEUTRAL_BRANDING } from "../src/branding";
import { resetDocumentTitle, setInstanceName } from "../src/documentTitle";

beforeEach(() => {
  resetDocumentTitle();
  setInstanceName("joinedcontext");
});

afterEach(() => {
  cleanup();
  document.title = "";
});

function at(path: string) {
  window.history.pushState({}, "", path);
}

describe("PageHeader on a route", () => {
  // First in the file on purpose: "the first page of a visit" is the first header this module
  // ever mounts.
  // UI-16: the first page of a visit leaves focus where the browser put it, the next page takes
  // it to its heading, and a header that mounts again on the same address takes nothing.
  it("moves focus to the heading on a route change and only then", () => {
    at("/projects/helsinki/spaces");
    const first = render(<PageHeader title="Spaces" />);
    expect(document.activeElement).toBe(document.body);
    first.unmount();

    at("/projects/helsinki/access");
    const second = render(<PageHeader title="Access" />);
    expect(document.activeElement).toBe(screen.getByRole("heading", { level: 1, name: "Access" }));
    second.unmount();

    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    render(<PageHeader title="Access" />);
    expect(document.activeElement).toBe(field);
    field.remove();
  });

  // UI-15: the document title names the page, the project and the instance.
  it("names the page and the project in the document title", () => {
    at("/projects/helsinki/access");
    render(<PageHeader title="Access" />);
    expect(document.title).toBe("Access · helsinki · joinedcontext");
  });

  // UI-15: a page outside a project has no project to name.
  it("leaves the project out where the address has none", () => {
    at("/endpoints");
    render(<PageHeader title="All endpoints" />);
    expect(document.title).toBe("All endpoints · joinedcontext");
  });

  // UI-15: the title follows the language, which changes the prop and not the address.
  it("follows the title when it changes", () => {
    at("/projects/helsinki/access");
    const view = render(<PageHeader title="Access" />);
    view.rerender(<PageHeader title="Prístup" />);
    expect(document.title).toBe("Prístup · helsinki · joinedcontext");
  });

  // UI-15: branding answering again does not blank the page out of the title. `applyBranding`
  // runs on every answer of /api/v1/branding and used to assign `document.title` itself, so a
  // refetch while a page was open left the tab reading only the installation's name.
  it("keeps the page in the title when branding answers again", () => {
    at("/projects/helsinki/access");
    render(<PageHeader title="Access" />);
    applyBranding({ ...NEUTRAL_BRANDING, instanceName: "Helsinki Region Context" });
    expect(document.title).toBe("Access · helsinki · Helsinki Region Context");
  });

  // UI-16: a project segment that is not a project name never reaches the title.
  it("keeps an address segment that is not a name out of the title", () => {
    at("/projects/%3Cimg%20src=x%3E/access");
    render(<PageHeader title="Access" />);
    expect(document.title).toBe("Access · joinedcontext");
  });
});
