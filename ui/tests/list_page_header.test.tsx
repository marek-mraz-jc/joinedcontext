/**
 * T-6095: a list page is one compact line (owner 2026-10-29): the title, its description on hover or
 * focus, the search right of it and the primary action, so the grid starts right below. The row menu
 * carries "View source (Git)" instead of an icon column.
 */
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import "../src/i18n";
import en from "../src/locales/en.json";
import { PageHeader } from "../src/components/ui";
import { ResourceRowActions } from "../src/components/ResourceRowActions";
import { json, renderPage } from "./page_contract";

afterEach(() => {
  cleanup();
});

describe("PageHeader compact", () => {
  function show() {
    render(
      <PageHeader
        compact
        title="Endpoints"
        description="Where a space is published."
        search={<input type="search" aria-label="Find an endpoint" />}
        actions={<button type="button">New endpoint</button>}
      />,
    );
  }

  it("keeps the description out of sight until the info mark is hovered or focused", async () => {
    show();
    const hint = screen.getByRole("button", { name: en.pageHeader.about });
    const tooltip = screen.getByRole("tooltip", { hidden: true });
    expect(tooltip).toHaveTextContent("Where a space is published.");
    expect(tooltip).toHaveClass("sr-only");
    // A screen reader hears the description with the mark, open or not.
    expect(hint).toHaveAccessibleDescription("Where a space is published.");

    await userEvent.hover(hint);
    expect(tooltip).not.toHaveClass("sr-only");
    await userEvent.unhover(hint);
    expect(tooltip).toHaveClass("sr-only");

    await userEvent.tab();
    expect(hint).toHaveFocus();
    expect(tooltip).not.toHaveClass("sr-only");
    await userEvent.keyboard("{Escape}");
    expect(tooltip).toHaveClass("sr-only");
  });

  it("puts title, search and actions on one line, the search before the primary action", () => {
    show();
    const heading = screen.getByRole("heading", { level: 1, name: "Endpoints" });
    const line = heading.parentElement?.parentElement as HTMLElement;
    const search = within(line).getByRole("searchbox", { name: "Find an endpoint" });
    const add = within(line).getByRole("button", { name: "New endpoint" });
    expect(search.compareDocumentPosition(add) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The description is no paragraph of its own under the title.
    expect(document.querySelector("p")).toBeNull();
  });

  it("keeps the description line on a detail page without compact", () => {
    render(<PageHeader title="Air quality" description="Readings of the city's stations." />);
    expect(screen.getByText("Readings of the city's stations.").tagName).toBe("P");
    expect(screen.queryByRole("button", { name: en.pageHeader.about })).toBeNull();
  });
});

describe("ResourceRowActions source item", () => {
  const TARGET = { project: "helsinki", kind: "Endpoint", plural: "endpoints", name: "air", label: "Air" };
  const grants = { grants: [{ rule: { kinds: ["Endpoint"], verbs: ["propose", "delete"] } }], bootstrap: false };

  async function open(sourceUrl: string) {
    renderPage(<ResourceRowActions project="helsinki" target={TARGET} sourceUrl={sourceUrl} />, {
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(grants) : undefined),
    });
    await userEvent.click(await screen.findByRole("button", { name: en.rowActions.more.replace("{name}", "Air") }));
  }

  it("lists the manifest in the forge as a menu link", async () => {
    await open("https://git.example.sk/helsinki/config/src/branch/main/endpoints/air.yaml");
    const item = await screen.findByRole("menuitem", { name: en.resourceList.viewSource });
    expect(item.closest("a")).toHaveAttribute(
      "href",
      "https://git.example.sk/helsinki/config/src/branch/main/endpoints/air.yaml",
    );
  });

  it("lists no source for an address that cannot navigate", async () => {
    await open("javascript:alert(1)");
    await screen.findByRole("menuitem", { name: en.resourceEdit.button });
    expect(screen.queryByRole("menuitem", { name: en.resourceList.viewSource })).toBeNull();
  });
});
