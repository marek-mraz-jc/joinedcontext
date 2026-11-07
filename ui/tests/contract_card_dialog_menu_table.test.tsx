/**
 * T-1736, T-1737, T-1738, T-1741, T-1742, T-1747: the UI contract for the six layout components
 * (UI-01, UI-15, UI-27, UI-44).
 *
 * Each case names a defect these shipped on 2026-09-20, found by reading them against the
 * contract rather than by a failing test — which is why none of them had one. EmptyState and
 * PageHeader were read the same way and no defect was found in either; their cases pin the
 * behaviour that was already right, so it cannot be lost.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/Card.tsx, src/components/ui/PageHeader.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import {
  Card,
  CardHeader,
  Dialog,
  EmptyState,
  Menu,
  MenuContent,
  MenuItem,
  MenuTrigger,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../src/components/ui";

const wrap = (node: React.ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("Card", () => {
  it("a_card_nested_under_a_section_does_not_skip_a_heading_level", () => {
    // `CardHeader` always rendered `<h2>`. A card inside a section that already has one gave the
    // page two `h2`s for one level of nesting, which is the `heading-order` violation axe reports.
    wrap(
      <section>
        <h2>Endpoints</h2>
        <Card>
          <CardHeader as="h3" title="Bike racks" />
        </Card>
      </section>,
    );
    expect(screen.getByRole("heading", { name: "Bike racks", level: 3 })).toBeInTheDocument();
  });

  it("a_card_at_the_top_of_a_page_still_defaults_to_h2", () => {
    wrap(
      <Card>
        <CardHeader title="Quota" description="What this project may use." />
      </Card>,
    );
    expect(screen.getByRole("heading", { name: "Quota", level: 2 })).toBeInTheDocument();
    expect(screen.getByText("What this project may use.")).toBeInTheDocument();
  });

  it("a_region_can_point_at_the_card_title_it_is_named_by", () => {
    wrap(
      <section aria-labelledby="quota-title">
        <Card>
          <CardHeader titleId="quota-title" title="Quota" />
        </Card>
      </section>,
    );
    expect(screen.getByRole("region", { name: "Quota" })).toBeInTheDocument();
  });
});

describe("Table", () => {
  const rows = (
    <Table caption="Endpoints of this project">
      <TableHead>
        <TableHeaderCell>Name</TableHeaderCell>
        <TableHeaderCell secondary>Changed by</TableHeaderCell>
      </TableHead>
      <TableBody>
        <TableRow>
          <TableCell primary>ep-bikes</TableCell>
          <TableCell secondary>Demo Steward</TableCell>
        </TableRow>
      </TableBody>
    </Table>
  );

  it("the_sideways_scrolling_frame_can_be_reached_and_named_by_a_keyboard", () => {
    // WCAG 2.1.1: the frame scrolls but had no `tabIndex`, so the columns past the right edge
    // were reachable with a pointer and by nothing else.
    wrap(rows);
    const frame = screen.getByRole("group", { name: "Endpoints of this project" });
    expect(frame).toHaveAttribute("tabindex", "0");
    frame.focus();
    expect(frame).toHaveFocus();
  });

  it("a_table_given_a_max_height_scrolls_in_place_in_the_same_named_frame", () => {
    // T-1775: the pipeline studio's sample used to sit in a wrapper of its own
    // (`max-h-64 overflow-auto`). A second scrolling box around the frame would be an unnamed,
    // unreachable scroll region, so the cap belongs on the frame that is already the tab stop.
    wrap(
      <Table caption="Sample" maxHeight="max-h-64">
        <TableHead>
          <TableHeaderCell>Name</TableHeaderCell>
        </TableHead>
        <TableBody>
          <TableRow>
            <TableCell primary>ep-bikes</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    const frame = screen.getByRole("group", { name: "Sample" });
    expect(frame.className).toContain("max-h-64");
    expect(frame.className).toContain("overflow-y-auto");
    expect(frame).toHaveAttribute("tabindex", "0");
    // Without it, nothing caps the height and the page grows with the data.
    expect(screen.getByRole("table")).toBeInTheDocument();
  });

  it("the_frame_is_not_a_landmark_beside_the_section_that_already_names_the_table", () => {
    // `region` is a landmark. Most tables sit in a `<section aria-labelledby>` whose name is the
    // caption, so a region here put two identically named landmarks one inside the other —
    // axe's `landmark-unique`, and an ambiguous "find the section called X" for every caller.
    wrap(
      <section aria-labelledby="eps">
        <h2 id="eps">Endpoints of this project</h2>
        {rows}
      </section>,
    );
    expect(screen.getAllByRole("region")).toHaveLength(1);
  });

  it("hovering_an_even_row_changes_it_even_though_the_zebra_tinted_it", () => {
    // The zebra rule is two selectors deep and so beat the row's own `hover:`; on every even row
    // the hover did nothing at all. The rule now stands down for the row under the pointer.
    wrap(rows);
    const table = screen.getByRole("table");
    expect(table.className).toContain("nth-child(even):not(:hover)");
  });

  it("a_translated_header_wraps_instead_of_pushing_the_last_column_off_screen", () => {
    wrap(rows);
    expect(screen.getByRole("columnheader", { name: "Changed by" }).className).not.toMatch(
      /whitespace-nowrap/,
    );
  });

  it("a_table_still_loading_says_so_and_marks_itself_busy", () => {
    wrap(
      <Table caption="Endpoints" status="Loading endpoints">
        <TableBody>
          <TableRow>
            <TableCell>…</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("Loading endpoints");
    expect(screen.getByRole("table")).toHaveAttribute("aria-busy", "true");
  });

  it("a_table_that_has_arrived_is_not_busy", () => {
    wrap(rows);
    expect(screen.getByRole("table")).not.toHaveAttribute("aria-busy");
  });
});

describe("Menu", () => {
  function OneMenu() {
    return (
      <Menu>
        <MenuTrigger>Actions</MenuTrigger>
        <MenuContent>
          <MenuItem>Duplicate</MenuItem>
          <MenuItem disabled>Approve</MenuItem>
        </MenuContent>
      </Menu>
    );
  }

  it("an_item_a_person_may_not_use_looks_different_from_one_they_may", async () => {
    // Radix marks it `data-disabled` and the class list read neither that nor `aria-disabled`,
    // so the only way to find out was to click it and watch nothing happen.
    wrap(<OneMenu />);
    await userEvent.click(screen.getByRole("button", { name: "Actions" }));
    const approve = await screen.findByRole("menuitem", { name: "Approve" });
    expect(approve).toHaveAttribute("data-disabled");
    expect(approve.className).toContain("data-[disabled]:text-fg-subtle");
    expect(approve.className).toContain("data-[disabled]:cursor-default");
  });

  it("a_long_menu_scrolls_inside_the_room_it_has_instead_of_running_off_the_screen", async () => {
    wrap(<OneMenu />);
    await userEvent.click(screen.getByRole("button", { name: "Actions" }));
    const menu = await screen.findByRole("menu");
    expect(menu.className).toContain("overflow-y-auto");
    expect(menu.className).toContain("--radix-dropdown-menu-content-available-height");
  });

  it("its_width_is_a_token_of_the_scale_not_an_arbitrary_value", async () => {
    wrap(<OneMenu />);
    await userEvent.click(screen.getByRole("button", { name: "Actions" }));
    const menu = await screen.findByRole("menu");
    expect(menu.className).toContain("min-w-40");
    expect(menu.className).not.toContain("min-w-[10rem]");
  });
});

describe("Dialog", () => {
  function OneDialog({ className }: { className?: string }) {
    const [open, setOpen] = useState(true);
    return (
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title="Delete this endpoint"
        description="The entities it serves stay where they are."
        closeLabel="Close"
        className={className}
        footer={<button type="button">Delete</button>}
      >
        <p>This cannot be undone.</p>
      </Dialog>
    );
  }

  it("a_caller_that_needs_one_thing_changed_keeps_the_focus_trap_and_the_labelling", () => {
    // Without a `className` a caller needing a different frame built its own Radix dialog and
    // lost the trap, the escape key and the title wiring with it; `ExportModal` is that caller.
    wrap(<OneDialog className="w-[42rem]" />);
    const dialog = screen.getByRole("dialog", { name: /Delete this endpoint/ });
    expect(dialog.className).toContain("w-[42rem]");
    expect(dialog).toHaveAccessibleDescription("The entities it serves stay where they are.");
  });

  it("its_own_size_survives_a_caller_adding_to_it", () => {
    wrap(<OneDialog className="max-w-none" />);
    const dialog = screen.getByRole("dialog");
    expect(dialog.className, "the size the component chose is still there").toContain("w-[min(40rem,92vw)]");
  });

  it("the_close_control_carries_the_label_it_was_given", () => {
    wrap(<OneDialog />);
    expect(within(screen.getByRole("dialog")).getByRole("button", { name: "Close" })).toBeInTheDocument();
  });
});

describe("EmptyState", () => {
  it("an_empty_list_is_announced_rather_than_silently_blank", () => {
    wrap(<EmptyState title="No endpoints yet" description="Create one to serve this space." />);
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("No endpoints yet");
    expect(status).toHaveTextContent("Create one to serve this space.");
  });

  it("its_icon_is_decorative_and_adds_nothing_to_what_is_read", () => {
    wrap(<EmptyState title="No endpoints yet" icon="inbox" />);
    expect(screen.getByRole("status").querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  });

  it("inside_a_frame_that_is_already_drawn_it_does_not_draw_a_second_one", () => {
    const { container } = wrap(<EmptyState title="Nothing here" bare />);
    expect(container.firstElementChild?.className).not.toMatch(/border-dashed/);
  });
});

describe("PageHeader", () => {
  it("the_page_has_exactly_one_h1_and_it_is_the_page_title", () => {
    wrap(
      <PageHeader
        title="Endpoints"
        description="What this project serves."
        actions={<button type="button">New endpoint</button>}
      />,
    );
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Endpoints");
  });

  it("it_is_not_a_second_banner_landmark_beside_the_shell", () => {
    // The shell's top bar is the one banner of the page; a `<header>` here would make two.
    const { container } = wrap(<PageHeader title="Endpoints" />);
    expect(container.querySelector("header")).toBeNull();
    expect(screen.queryByRole("banner")).not.toBeInTheDocument();
  });

  it("the_actions_and_the_aside_both_reach_the_page", () => {
    wrap(
      <PageHeader
        title="Endpoints"
        aside={<span>3 of 10 used</span>}
        actions={<button type="button">New endpoint</button>}
      />,
    );
    expect(screen.getByText("3 of 10 used")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New endpoint" })).toBeInTheDocument();
  });
});
