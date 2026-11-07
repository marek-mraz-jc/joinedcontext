/**
 * UI-01, UI-15, UI-16 (T-2137, T-2326): what a Policy grants, picked by the CIM 009 group names.
 *
 * The rule this widget carries is that a name nobody can expand is a word nobody can check: each
 * of the five groups says which operations it stands for, a group that changes context data is
 * marked as one, and a name a hand-written manifest already grants stays visible and removable
 * rather than being dropped on the first tick. The order it writes back is the manifest's order,
 * not the order a person happened to tick in, so two policies written the same way read the same.
 */
import { screen, within } from "@testing-library/react";
import type { WidgetProps } from "@rjsf/utils";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { OperationsPicker } from "../src/components/forms/widgets/OperationsPicker";
import { OPERATION_GROUPS } from "../src/components/endpoints/operationGroups";
import { expectNoAxeViolations, renderPart } from "./page_contract";

function props(overrides?: Partial<WidgetProps>): WidgetProps {
  return {
    id: "root_operations",
    name: "operations",
    schema: { type: "array" },
    value: [],
    required: true,
    disabled: false,
    readonly: false,
    autofocus: false,
    options: {},
    label: "Operations",
    onChange: vi.fn(),
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    registry: {} as WidgetProps["registry"],
    ...overrides,
  };
}

function show(value: unknown, overrides?: Partial<WidgetProps>) {
  const given = props({ value, ...overrides });
  const view = renderPart(<OperationsPicker {...given} />);
  return { ...view, onChange: given.onChange as ReturnType<typeof vi.fn> };
}

/** A group's box by the words it is offered in, not its CIM 009 name (T-2756). */
const groupBox = (name: string) =>
  screen.getByRole("checkbox", {
    name: new RegExp(
      `^${en.choice.operationGroup[name as keyof typeof en.choice.operationGroup]}( ${en.policies.operations.writes})?$`,
    ),
  });

describe("the operations a Policy grants", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("offers the five group names and shows what each one stands for", () => {
    show([]);
    for (const [name, group] of Object.entries(OPERATION_GROUPS)) {
      expect(groupBox(name)).toBeInTheDocument();
      expect(screen.getByText(group.operations.join(", "))).toBeInTheDocument();
    }
  });

  // T-4211 (photo audit T-4158, rule 1): a group reads as a sentence, and its CIM 009 operation
  // names wait behind a closed disclosure that says how many there are, so a phone does not
  // scroll through two screens of identifiers.
  it("keeps each group's operation names behind a closed disclosure that counts them", () => {
    show([]);
    for (const group of Object.values(OPERATION_GROUPS)) {
      const names = screen.getByText(group.operations.join(", "));
      const fold = names.closest("details");
      expect(fold).not.toBeNull();
      expect(fold).not.toHaveAttribute("open");
      expect(within(fold as HTMLElement).getByText(`Show its ${group.operations.length} operations`)).toBeInTheDocument();
    }
  });

  it("marks the groups that change context data, and only those", () => {
    const { container } = show([]);
    const marked = [...container.querySelectorAll("label")]
      .filter((label) => label.textContent?.includes(en.policies.operations.writes))
      .map((label) => label.querySelector("input")?.id.replace("root_operations-", ""));
    expect(marked.sort()).toEqual(["redirectionOps", "updateOps"]);
  });

  it("writes the groups back in the manifest's order, whatever order they were ticked in", async () => {
    const user = userEvent.setup();
    const { onChange } = show(["redirectionOps"]);
    await user.click(groupBox("retrieveOps"));
    expect(onChange).toHaveBeenCalledWith(["retrieveOps", "redirectionOps"]);
  });

  it("counts every operation the choice covers, groups expanded", async () => {
    show([]);
    expect(screen.getByTestId("operations-summary")).toHaveTextContent(en.policies.operations.none);

    show(["retrieveOps"]);
    const summaries = screen.getAllByTestId("operations-summary");
    expect(summaries[summaries.length - 1]).toHaveTextContent(
      String(OPERATION_GROUPS.retrieveOps.operations.length),
    );
  });

  it("keeps an operation a hand-written manifest grants, so a person can see it and take it away", async () => {
    const user = userEvent.setup();
    // `deleteEntity` is in no group of Table 4.20-2 by itself; a manifest may still name it.
    const { onChange } = show(["deleteEntity"]);
    const kept = screen.getByRole("checkbox", { name: "deleteEntity" });
    expect(kept).toBeChecked();
    await user.click(kept);
    expect(onChange).toHaveBeenCalledWith([]);
  });

  it("takes no tick while the field is read-only", async () => {
    const user = userEvent.setup();
    const { onChange } = show([], { readonly: true });
    await user.click(groupBox("retrieveOps"));
    expect(onChange).not.toHaveBeenCalled();
    expect(groupBox("retrieveOps")).toBeDisabled();
  });

  it("says what the server refused where a person is looking", () => {
    show([], { rawErrors: ["updateOps is not granted to this project"] });
    expect(screen.getByText("updateOps is not granted to this project")).toBeInTheDocument();
  });

  it("has no axe violation", async () => {
    const { container } = show(["retrieveOps"]);
    expect(within(container).getAllByRole("group").length).toBeGreaterThan(0);
    await expectNoAxeViolations(container);
  });
});
