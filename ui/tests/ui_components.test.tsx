// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/Badge.tsx, src/components/ui/Button.tsx, src/components/ui/Switch.tsx, src/components/ui/Table.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import {
  Badge,
  Button,
  Dialog,
  EmptyState,
  Field,
  Input,
  Select,
  Switch,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../src/components/ui";

describe("ui components", () => {
  it("a button is a button by default, not a submit, and is inert while loading", async () => {
    const onClick = vi.fn();
    render(
      <>
        <Button onClick={onClick}>Plain</Button>
        <Button variant="primary" loading>
          Saving
        </Button>
      </>,
    );
    const plain = screen.getByRole("button", { name: "Plain" });
    expect(plain).toHaveAttribute("type", "button");
    await userEvent.click(plain);
    expect(onClick).toHaveBeenCalledTimes(1);

    const saving = screen.getByRole("button", { name: "Saving" });
    expect(saving).toBeDisabled();
    expect(saving).toHaveAttribute("aria-busy", "true");
  });

  it("a field labels its control and hangs the help and the error off it", () => {
    render(
      <Field
        id="slug"
        label="Slug"
        required
        description="Part of the public URL."
        help="Lower case, dashes."
        errors={["This field is required"]}
      >
        {/* No hand-wiring: the Field puts the ids on the control itself now (T-1739). */}
        <Input id="slug" />
      </Field>,
    );
    const input = screen.getByLabelText(/Slug/);
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription(
      "Part of the public URL. Lower case, dashes. This field is required",
    );
    expect(screen.getByRole("alert")).toHaveTextContent("This field is required");
    expect(screen.getByText("Part of the public URL.")).toHaveAttribute("id", "slug__description");
  });

  it("a select is native, so its options are real options", () => {
    render(
      <Select aria-label="Audience" defaultValue="public">
        <option value="public">Public</option>
        <option value="internal">Internal</option>
      </Select>,
    );
    const select = screen.getByRole("combobox", { name: "Audience" });
    expect(within(select).getAllByRole("option")).toHaveLength(2);
    expect(select).toHaveValue("public");
  });

  it("a switch toggles by click and by keyboard and reports its state in aria-checked", async () => {
    function Harness() {
      const [on, setOn] = useState(false);
      return <Switch checked={on} onCheckedChange={setOn} label="Enabled" />;
    }
    render(<Harness />);
    const toggle = screen.getByRole("switch", { name: "Enabled" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");

    toggle.focus();
    await userEvent.keyboard(" ");
    expect(toggle).toHaveAttribute("aria-checked", "false");
  });

  it("a table has a caption, an empty row and a loading body that announces itself", () => {
    const { rerender } = render(
      <Table caption="Endpoints" status="Loading endpoints">
        <TableHead>
          <TableHeaderCell>Name</TableHeaderCell>
          <TableHeaderCell align="right">Actions</TableHeaderCell>
        </TableHead>
        <TableSkeleton columns={2} rows={2} />
      </Table>,
    );
    expect(screen.getByRole("table", { name: "Endpoints" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Loading endpoints");

    rerender(
      <Table caption="Endpoints">
        <TableHead>
          <TableHeaderCell>Name</TableHeaderCell>
          <TableHeaderCell align="right">Actions</TableHeaderCell>
        </TableHead>
        <TableBody>
          <TableEmpty columns={2}>
            <EmptyState bare title="No endpoints yet" description="Publish a space first." />
          </TableEmpty>
        </TableBody>
      </Table>,
    );
    expect(screen.getByText("No endpoints yet")).toBeInTheDocument();
    // Loading is over; what is announced now is that the list is empty (T-1393).
    expect(screen.getByRole("status")).toHaveTextContent("No endpoints yet");
    expect(screen.getByRole("status")).not.toHaveTextContent("Loading endpoints");

    rerender(
      <Table caption="Endpoints">
        <TableHead>
          <TableHeaderCell>Name</TableHeaderCell>
          <TableHeaderCell align="right">Actions</TableHeaderCell>
        </TableHead>
        <TableBody>
          <TableRow>
            <TableCell primary>public-air</TableCell>
            <TableCell align="right">
              <Badge tone="success">Live</Badge>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    const row = screen.getByText("public-air").closest("tr") as HTMLElement;
    expect(within(row).getByText("Live")).toBeInTheDocument();
    expect(screen.getAllByRole("columnheader")).toHaveLength(2);
  });

  it("a dialog is labelled by its title and closes from the corner control", async () => {
    function Harness() {
      const [open, setOpen] = useState(true);
      return (
        <Dialog
          open={open}
          onOpenChange={setOpen}
          title="Add an endpoint"
          description="It is proposed, not created."
          closeLabel="Close"
        >
          <p>Body</p>
        </Dialog>
      );
    }
    render(<Harness />);
    const dialog = screen.getByRole("dialog", { name: "Add an endpoint" });
    expect(dialog).toHaveAccessibleDescription("It is proposed, not created.");

    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

describe("EmptyState as a status (T-1393)", () => {
  it("is announced: a status with its title and description", () => {
    render(<EmptyState title="No spaces yet" description="Add the first one." />);
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("No spaces yet");
    expect(status).toHaveTextContent("Add the first one.");
  });
});
