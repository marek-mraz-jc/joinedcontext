/**
 * The edge cases of the six layout components (T-1736, T-1737, T-1738, T-1741, T-1742, T-1747):
 * what each one does with nothing, with one thing, with markup typed at it, and in the state a
 * caller reaches least often. The contract cases live in contract_card_dialog_menu_table.test.tsx;
 * these are the ones that break a page rather than a rule.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/Menu.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
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
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../src/components/ui";

const wrap = (node: React.ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

/** What a person can type into a name, and what an API can hand back. */
const MARKUP = '<img src=x onerror="alert(1)">';

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("Table, at its edges", () => {
  it("a_table_with_no_rows_still_names_itself_and_says_why_it_is_empty", () => {
    wrap(
      <Table caption="Endpoints of this project">
        <TableHead>
          <TableHeaderCell>Name</TableHeaderCell>
        </TableHead>
        <TableBody>
          <TableEmpty columns={1}>No endpoints yet.</TableEmpty>
        </TableBody>
      </Table>,
    );
    expect(screen.getByRole("group", { name: "Endpoints of this project" })).toBeInTheDocument();
    expect(screen.getByRole("table")).toHaveAccessibleName("Endpoints of this project");
    expect(screen.getByText("No endpoints yet.")).toBeInTheDocument();
    // On a phone a wide head scrolls; the message holds still in view instead of scrolling away.
    expect(screen.getByText("No endpoints yet.")).toHaveClass("sticky");
  });

  it("the_skeleton_rows_are_never_read_out_as_data", () => {
    // Four rows of grey bars announced cell by cell would be four rows of nothing; the `status`
    // is what a screen reader is meant to hear while the list is on its way.
    wrap(
      <Table caption="Endpoints" status="Loading endpoints">
        <TableSkeleton columns={3} />
      </Table>,
    );
    const skeleton = screen.getByRole("table").querySelector("tbody");
    expect(skeleton).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByRole("status")).toHaveTextContent("Loading endpoints");
  });

  it("a_cell_holding_markup_shows_it_as_text", () => {
    wrap(
      <Table caption="Endpoints">
        <TableBody>
          <TableRow>
            <TableCell primary>{MARKUP}</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    const cell = screen.getByRole("cell");
    expect(cell.querySelector("img")).toBeNull();
    expect(cell).toHaveTextContent(MARKUP);
  });

  it("a_caption_a_locale_made_long_still_names_the_frame_and_the_table_alike", async () => {
    await i18n.changeLanguage("de");
    const caption = "Endpunkte dieses Projekts, zuletzt geändert";
    wrap(
      <Table caption={caption}>
        <TableBody>
          <TableRow>
            <TableCell>ep-bikes</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    expect(screen.getByRole("group", { name: caption })).toBeInTheDocument();
    expect(screen.getByRole("table")).toHaveAccessibleName(caption);
  });

  it("columns_past_the_right_edge_show_a_cue_that_leaves_at_the_end_of_the_scroll", () => {
    // jsdom lays nothing out: the frame says it is 300 px wide over a 700 px table.
    const sizes = [
      vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(300),
      vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(700),
    ];
    try {
      const { container } = wrap(
        <Table caption="Policies">
          <TableHead>
            <TableHeaderCell>Name</TableHeaderCell>
            <TableHeaderCell>Granted to</TableHeaderCell>
          </TableHead>
          <TableBody>
            <TableRow>
              <TableCell>app-air-quality</TableCell>
              <TableCell>role:endpoint</TableCell>
            </TableRow>
          </TableBody>
        </Table>,
      );
      const cue = screen.getByTestId("table-more-columns");
      expect(cue).toHaveAttribute("aria-hidden", "true");
      expect(cue.className).toContain("pointer-events-none");

      const frame = screen.getByRole("group", { name: "Policies" });
      frame.scrollLeft = 400;
      fireEvent.scroll(frame);
      expect(screen.queryByTestId("table-more-columns")).toBeNull();
      expect(container.querySelector("table")).toHaveAccessibleName("Policies");
    } finally {
      sizes.forEach((spy) => spy.mockRestore());
    }
  });

  it("a_table_that_fits_shows_no_cue", () => {
    wrap(
      <Table caption="Roles">
        <TableBody>
          <TableRow>
            <TableCell>editor</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    expect(screen.queryByTestId("table-more-columns")).toBeNull();
  });

  it("zebra_off_leaves_the_row_hover_as_the_only_rule", () => {
    wrap(
      <Table caption="Endpoints" zebra={false}>
        <TableBody>
          <TableRow>
            <TableCell>ep-bikes</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    expect(screen.getByRole("table").className).not.toContain("nth-child(even)");
    expect(screen.getByRole("row").className).toContain("hover:bg-primary-50");
  });
});

describe("Menu, at its edges", () => {
  it("a_menu_whose_every_action_is_refused_still_opens_and_says_so", async () => {
    // The permission-denied shape: nothing is clickable, and a person must be able to find out
    // why rather than meet a menu that appears to do nothing.
    wrap(
      <Menu>
        <MenuTrigger>Actions</MenuTrigger>
        <MenuContent>
          <MenuItem disabled>Approve</MenuItem>
          <MenuItem disabled tone="danger">
            Delete
          </MenuItem>
        </MenuContent>
      </Menu>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Actions" }));
    const items = await screen.findAllByRole("menuitem");
    expect(items).toHaveLength(2);
    for (const item of items) expect(item).toHaveAttribute("data-disabled");
  });

  it("the_current_item_is_marked_by_more_than_the_highlight_that_follows_the_pointer", async () => {
    wrap(
      <Menu>
        <MenuTrigger>Language</MenuTrigger>
        <MenuContent>
          <MenuItem aria-current="true">English</MenuItem>
          <MenuItem>Deutsch</MenuItem>
        </MenuContent>
      </Menu>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Language" }));
    const current = await screen.findByRole("menuitem", { name: "English" });
    expect(current.className).toContain("aria-[current=true]:font-semibold");
  });

  it("escape_closes_it_and_hands_focus_back_to_what_opened_it", async () => {
    wrap(
      <Menu>
        <MenuTrigger>Actions</MenuTrigger>
        <MenuContent>
          <MenuItem>Duplicate</MenuItem>
        </MenuContent>
      </Menu>,
    );
    const trigger = screen.getByRole("button", { name: "Actions" });
    await userEvent.click(trigger);
    await screen.findByRole("menu");
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("an_open_menu_hides_no_link_of_the_page_from_a_screen_reader", async () => {
    wrap(
      <>
        <a href="/projects">Projects</a>
        <Menu>
          <MenuTrigger>Actions</MenuTrigger>
          <MenuContent>
            <MenuItem>Duplicate</MenuItem>
          </MenuContent>
        </Menu>
      </>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Actions" }));
    await screen.findByRole("menu");
    // A modal menu set aria-hidden on the page around it while its links kept focus (axe aria-hidden-focus, T-3986).
    expect(document.querySelector("[aria-hidden='true'] a, a[aria-hidden='true']")).toBeNull();
    expect(screen.getByRole("link", { name: "Projects" })).toBeInTheDocument();
  });
});

describe("Dialog, at its edges", () => {
  function OneDialog(props: { description?: string; footer?: React.ReactNode; title?: string }) {
    const [open, setOpen] = useState(true);
    return (
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title={props.title ?? "Delete this endpoint"}
        description={props.description}
        closeLabel="Close"
        footer={props.footer}
      >
        <input aria-label="Type the name to confirm" />
      </Dialog>
    );
  }

  it("a_dialog_with_nothing_to_explain_claims_no_description", () => {
    wrap(<OneDialog />);
    const dialog = screen.getByRole("dialog");
    expect(dialog).not.toHaveAttribute("aria-describedby");
    expect(dialog).toHaveAccessibleName("Delete this endpoint");
  });

  it("a_dialog_with_no_footer_draws_no_empty_bar_under_its_body", () => {
    const { container } = wrap(<OneDialog />);
    expect(container.ownerDocument.querySelector(".border-t")).toBeNull();
  });

  it("a_closed_dialog_puts_nothing_in_the_document", () => {
    function Closed() {
      return (
        <Dialog open={false} onOpenChange={() => {}} title="Delete" closeLabel="Close">
          <p>Body</p>
        </Dialog>
      );
    }
    wrap(<Closed />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByText("Body")).not.toBeInTheDocument();
  });

  it("it_opens_on_the_first_field_rather_than_on_the_close_button", () => {
    // Radix hands focus to the first tabbable element, which is the close control in the corner;
    // a person filling a form would start one tab stop away from the field every time.
    wrap(<OneDialog />);
    expect(screen.getByLabelText("Type the name to confirm")).toHaveFocus();
  });

  it("a_title_that_arrived_as_markup_is_read_as_text", () => {
    wrap(<OneDialog title={MARKUP} />);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByRole("img")).not.toBeInTheDocument();
    expect(dialog).toHaveAccessibleName(MARKUP);
  });
});

describe("Card, EmptyState and PageHeader, at their edges", () => {
  it("a_flush_card_carries_no_padding_of_its_own_for_the_table_inside_it", () => {
    const { container } = wrap(
      <Card flush>
        <Table caption="Endpoints">
          <TableBody>
            <TableRow>
              <TableCell>ep-bikes</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </Card>,
    );
    expect(container.firstElementChild?.className).not.toMatch(/\bp-5\b/);
  });

  it("a_card_carries_exactly_one_padding_and_the_element_it_is_asked_for", () => {
    // T-4888: a `p-3` class beside the default `p-5` lost by CSS source order; the prop is the one padding.
    const { container } = wrap(
      <ul>
        <Card as="li" padding="sm">
          ovzdusie
        </Card>
      </ul>,
    );
    const card = container.querySelector("li");
    expect(card?.className.match(/(?<![\w-])p-\d/g)).toEqual(["p-3"]);
    expect(wrap(<Card>quota</Card>).container.firstElementChild?.className).toMatch(/(?<![\w-])p-5\b/);
  });

  it("a_card_header_with_only_a_title_renders_neither_an_empty_description_nor_an_empty_action_bar", () => {
    const { container } = wrap(
      <Card>
        <CardHeader title="Quota" />
      </Card>,
    );
    expect(container.querySelectorAll("p"), "no empty description paragraph").toHaveLength(0);
    // The header row holds the title block and nothing else: no actions container is drawn for
    // a card that was given no actions, which would otherwise take a gap of its own.
    const header = screen.getByRole("heading", { level: 2 }).parentElement?.parentElement;
    expect(header?.children).toHaveLength(1);
  });

  it("a_card_title_that_arrived_as_markup_is_read_as_text", () => {
    wrap(
      <Card>
        <CardHeader title={MARKUP} />
      </Card>,
    );
    const heading = screen.getByRole("heading", { level: 2 });
    expect(heading.querySelector("img")).toBeNull();
    expect(heading).toHaveTextContent(MARKUP);
  });

  it("an_empty_state_with_no_action_offers_no_empty_space_where_a_button_would_be", () => {
    wrap(<EmptyState title="No endpoints yet" />);
    expect(screen.getByRole("status").querySelectorAll("button")).toHaveLength(0);
  });

  it("a_page_header_with_a_title_alone_renders_no_action_row", () => {
    wrap(<PageHeader title="Endpoints" />);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Endpoints");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("a_page_title_that_arrived_as_markup_is_read_as_text", () => {
    wrap(<PageHeader title={MARKUP} />);
    const heading = screen.getByRole("heading", { level: 1 });
    expect(heading.querySelector("img")).toBeNull();
    expect(heading).toHaveTextContent(MARKUP);
  });
});
