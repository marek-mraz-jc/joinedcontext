/**
 * T-1830, T-1832, T-1833, T-1834, T-1835, T-1836: the cards an agent run draws (UI-01, UI-15,
 * UI-16, UI-44, PF-50).
 *
 * The two comparison tables here were hand-made — one with `<th>` cells carrying no `scope` at
 * all, so nothing reading the table as a table could pair a value with the entity it belongs
 * to, and one that styled its own row headers because the shared Table had no cell for them.
 * `TableRowHeaderCell` is that cell, and these are the cases that hold both tables to it, and
 * the catalog row's Use button to the shared Button it is now.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { expectDenied } from "./checks";
import { CatalogCards } from "../src/pages/apps/CatalogCards";
import type { CatalogItem } from "../src/pages/apps/CatalogCards";
import { EntityWriteCard } from "../src/pages/apps/EntityWriteCard";

const NOW = Date.parse("2026-09-13T08:00:12Z");

const ENDPOINT: CatalogItem = {
  kind: "Endpoint",
  name: "helsinki-bikes",
  space: "helsinki",
  owner: "helsinki",
  title: "Helsinki city bikes",
  endpointSlug: "bikesslug00000000000000000000000",
  matchReason: ["title"],
  access: { verdict: "allowed", reason: "audience public" },
  freshness: null,
};

const WRITE = {
  endpoint: "helsinki-bikes",
  slug: "bikesslug00000000000000000000000",
  entities: [
    {
      id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:kamppi",
      changes: [
        { attribute: "pm10", before: 12, after: 18 },
        { attribute: "pm25", before: undefined, after: 7 },
      ],
    },
  ],
};

function withRouter(node: React.ReactNode) {
  const rootRoute = createRootRoute({ component: () => <>{node}</> });
  const router = createRouter({ routeTree: rootRoute });
  return render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the entity write card", () => {
  it("names_every_row_with_the_entity_it_changes_so_a_value_is_never_read_alone", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <EntityWriteCard write={WRITE} live={false} />
      </I18nextProvider>,
    );
    const table = screen.getByRole("table");
    // The caption is what a screen reader announces on entering; the hand-made table had none.
    expect(within(table).getByText(en.agentRun.write.title)).toBeInTheDocument();

    const rows = within(table).getAllByRole("row");
    // Head plus one row per changed attribute.
    expect(rows).toHaveLength(3);
    for (const row of rows.slice(1)) {
      const header = within(row).getAllByRole("rowheader");
      expect(header, "every row names itself; the hand-made cells carried no scope").toHaveLength(1);
    }
    expect(within(rows[1]).getByRole("rowheader")).toHaveAttribute(
      "title",
      "urn:ngsi-ld:AirQualityObserved:hel.fi:air:kamppi",
    );
    // An attribute that had no value before says so in words rather than being blank.
    expect(within(rows[2]).getAllByRole("cell")[1]).toHaveTextContent(en.agentRun.write.empty);
  });

  it("renders_a_written_value_out_of_the_run_as_text", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <EntityWriteCard
          live={false}
          write={{
            endpoint: "helsinki-bikes",
            slug: "bikesslug00000000000000000000000",
            entities: [
              {
                id: "urn:ngsi-ld:X:hel.fi:a:b",
                changes: [{ attribute: "note", before: undefined, after: "<img src=x onerror=alert(1)>" }],
              },
            ],
          }}
        />
      </I18nextProvider>,
    );
    const table = screen.getByRole("table");
    expect(within(table).getByText(/<img src=x onerror=alert\(1\)>/)).toBeInTheDocument();
    expect(table.querySelector("img")).toBeNull();
  });
});

describe("the catalog row's Use", () => {
  it("is_the_shared_button_and_says_why_it_is_refused_while_staying_reachable", async () => {
    withRouter(
      <CatalogCards
        project="helsinki"
        items={[ENDPOINT]}
        now={NOW}
        onUseEndpoint={() => undefined}
        usedEndpoints={["helsinki-bikes"]}
      />,
    );
    const button = await screen.findByRole("button", { name: "helsinki-bikes is in use" });
    expect(button).toHaveClass("focus-ring");
    expectDenied(button, "helsinki-bikes is in use");

    // Reachable means reachable: it takes the focus and refuses the click all the same.
    button.focus();
    expect(button).toHaveFocus();
    await userEvent.click(button);
    expect(button).toHaveTextContent(en.agentRun.catalog.inUse);
  });

  it("is_pressed_from_the_keyboard_when_the_endpoint_is_not_in_the_conversation_yet", async () => {
    const used: string[] = [];
    withRouter(
      <CatalogCards
        project="helsinki"
        items={[ENDPOINT]}
        now={NOW}
        onUseEndpoint={(name) => used.push(name)}
      />,
    );
    const button = await screen.findByRole("button", {
      name: "Use helsinki-bikes in this conversation",
    });
    button.focus();
    await userEvent.keyboard("{Enter}");
    expect(used).toEqual(["helsinki-bikes"]);
  });
});
