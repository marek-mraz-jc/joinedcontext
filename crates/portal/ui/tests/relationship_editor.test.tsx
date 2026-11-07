/** T-2737: relationships built in the model editor without touching the YAML (DM-13, DM-64…DM-66, DM-73, UI-84). */
import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectNoRawKeys, expectNoViolations } from "./checks";
import { LinkmlVisualEditor } from "../src/pages/models/LinkmlVisualEditor";
import { suggestedName } from "../src/pages/models/RelationshipEditor";
import { CARDINALITIES, parseModel, relationships } from "../src/pages/models/linkml";
import type { Cardinality } from "../src/pages/models/linkml";

const SOURCE = `id: https://rozvoj.sk/models/schools
name: schools
prefixes:
  rz: https://rozvoj.sk/terms/
default_prefix: rz
classes:
  School:
    slots:
      - title
  User:
    slots: []
  Course:
    slots: []
slots:
  title:
    range: string
    slot_uri: rz:title
enums: {}
`;

/** The editor is controlled, so the test holds the document the views share. */
function Harness({ initial }: { initial: string }) {
  const [source, setSource] = useState(initial);
  return (
    <>
      <LinkmlVisualEditor source={source} onChange={setSource} locales={["sk", "en"]} />
      <textarea readOnly aria-label="source" value={source} />
    </>
  );
}

function renderEditor(initial = SOURCE) {
  const view = render(
    <I18nextProvider i18n={i18n}>
      <Harness initial={initial} />
    </I18nextProvider>,
  );
  return {
    view,
    source: () => (screen.getByLabelText("source") as HTMLTextAreaElement).value,
    user: userEvent.setup(),
    form: () => screen.getByRole("form", { name: "Add a relationship from School" }),
    // A link is added through Add field (owner 2026-11-01, T-6344): what the field holds first, and a
    // link takes the relationship form in the same dialog.
    openForm: async () => {
      await userEvent.click(screen.getByRole("button", { name: i18n.t("models.addSlot") }));
      await userEvent.click(screen.getByRole("radio", { name: new RegExp(i18n.t("models.addKind.Relationship")) }));
      return screen.getByRole("form", { name: i18n.t("models.relationships.add.title", { owner: "School" }) });
    },
  };
}

const NAMES: Record<Cardinality, string> = {
  "one-to-one": "One to one",
  "one-to-many": "One to many",
  "many-to-one": "Many to one",
  "many-to-many": "Many to many",
};

describe("relationship editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it.each(CARDINALITIES)("adds a %s relationship with both ends from the form", async (cardinality) => {
    const { source, user, openForm } = renderEditor();
    const add = within(await openForm());

    await user.click(add.getByRole("radio", { name: NAMES[cardinality] }));
    await user.selectOptions(add.getByLabelText("Target class"), "User");
    // The names follow the target and the cardinality: one School has many `users`.
    expect(add.getByLabelText("Name on School")).toHaveValue(suggestedName("User", cardinality === "one-to-many" || cardinality === "many-to-many"));
    expect(add.getByLabelText(/^Inverse on User/)).toHaveValue(suggestedName("School", cardinality === "many-to-one" || cardinality === "many-to-many"));
    await user.click(add.getByRole("button", { name: "Add relationship" }));

    const model = parseModel(source());
    const [relationship] = relationships(model);
    expect(relationship).toMatchObject({ cardinality, source: { class: "School" }, target: { class: "User" }, onDelete: "restrict" });
    expect(model.classes.find((klass) => klass.name === "User")?.slots).toEqual([relationship.target.slot]);
    // The row says it as a sentence, and the dialog closed.
    const table = screen.getByRole("table", { name: "Relationships" });
    expect(within(table).getByRole("cell", { name: relationship.source.slot })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("builds School, User and Course with every cardinality", async () => {
    const { source, user, openForm } = renderEditor();
    const build = async (cardinality: Cardinality, target: string, name: string, inverse: string) => {
      const add = within(await openForm());
      await user.click(add.getByRole("radio", { name: NAMES[cardinality] }));
      await user.selectOptions(add.getByLabelText("Target class"), target);
      await user.clear(add.getByLabelText("Name on School"));
      await user.type(add.getByLabelText("Name on School"), name);
      await user.clear(add.getByLabelText(new RegExp(`^Inverse on ${target}`)));
      await user.type(add.getByLabelText(new RegExp(`^Inverse on ${target}`)), inverse);
      await user.click(add.getByRole("button", { name: "Add relationship" }));
    };
    await build("one-to-one", "User", "principal", "principalOf");
    await build("one-to-many", "User", "pupils", "school");
    await build("many-to-one", "Course", "flagship", "flagshipOf");
    await build("many-to-many", "Course", "courses", "schools");

    expect(relationships(parseModel(source())).map((one) => [one.source.slot, one.cardinality, one.target.slot])).toEqual([
      ["principal", "one-to-one", "principalOf"],
      ["pupils", "one-to-many", "school"],
      ["flagship", "many-to-one", "flagshipOf"],
      ["courses", "many-to-many", "schools"],
    ]);
    expect(screen.getByText("One School has many User entities")).toBeInTheDocument();
    expect(screen.getByText("Many School entities have many Course entities")).toBeInTheDocument();
  });

  it("requires the inverse: an empty one keeps Add disabled and says why", async () => {
    const { source, user, openForm } = renderEditor();
    const add = within(await openForm());
    await user.selectOptions(add.getByLabelText("Target class"), "User");
    await user.clear(add.getByLabelText(/^Inverse on User/));

    expect(add.getByRole("button", { name: "Add relationship" })).toHaveAttribute("aria-disabled", "true");
    expect(add.getByLabelText(/^Inverse on User/)).toHaveAccessibleDescription(expect.stringContaining("Name the field."));
    expect(source()).toBe(SOURCE);
  });

  it("keeps Add disabled while the form is invalid", async () => {
    const { user, openForm } = renderEditor();
    const add = within(await openForm());
    const button = () => add.getByRole("button", { name: "Add relationship" });
    // No target yet, and the form says so where a sighted person reads it (T-6344 judge).
    expect(button()).toHaveAttribute("aria-disabled", "true");
    expect(add.getByText("Pick the target class and name both ends first.", { selector: "p" })).toBeVisible();
    expect(add.getByLabelText(/^Inverse on the class you pick/)).toBeInTheDocument();

    await user.selectOptions(add.getByLabelText("Target class"), "User");
    expect(button()).not.toHaveAttribute("aria-disabled");
    expect(add.queryByText("Pick the target class and name both ends first.", { selector: "p" })).toBeNull();
    // A name that cannot become a slot.
    await user.clear(add.getByLabelText("Name on School"));
    await user.type(add.getByLabelText("Name on School"), "9lives");
    expect(button()).toHaveAttribute("aria-disabled", "true");
    expect(add.getByLabelText("Name on School")).toHaveAccessibleDescription(expect.stringContaining("starting with a letter"));
    // A name the model already has.
    await user.clear(add.getByLabelText("Name on School"));
    await user.type(add.getByLabelText("Name on School"), "title");
    expect(button()).toHaveAttribute("aria-disabled", "true");
    expect(add.getByLabelText("Name on School")).toHaveAccessibleDescription(expect.stringContaining("A field named title already exists."));
    // The inverse the same as the name.
    await user.clear(add.getByLabelText("Name on School"));
    await user.type(add.getByLabelText("Name on School"), "users");
    await user.clear(add.getByLabelText(/^Inverse on User/));
    await user.type(add.getByLabelText(/^Inverse on User/), "users");
    expect(button()).toHaveAttribute("aria-disabled", "true");
  });

  it("lets only the stored end be required", async () => {
    const { source, user, openForm } = renderEditor();
    const add = within(await openForm());
    await user.click(add.getByRole("radio", { name: "One to many" }));
    await user.selectOptions(add.getByLabelText("Target class"), "User");

    // One School has many users: each User stores its school, the School's list is computed.
    const computed = add.getByRole("checkbox", { name: "Required on School" });
    expect(computed).toHaveAttribute("aria-disabled", "true");
    await user.click(computed);
    expect(computed).not.toBeChecked();

    await user.click(add.getByRole("checkbox", { name: "Required on User" }));
    await user.click(add.getByRole("button", { name: "Add relationship" }));
    const model = parseModel(source());
    expect(model.slots.find((slot) => slot.name === "school")?.required).toBe(true);
    expect(model.slots.find((slot) => slot.name === "users")?.required).toBe(false);
  });

  it("asks before removing, and removes both ends", async () => {
    const { source, user, openForm } = renderEditor();
    const add = within(await openForm());
    await user.selectOptions(add.getByLabelText("Target class"), "User");
    await user.click(add.getByRole("button", { name: "Add relationship" }));
    const withRelationship = source();

    await user.click(screen.getByRole("button", { name: "Remove the relationship users" }));
    const dialog = screen.getByRole("alertdialog", { name: "Remove the relationship users?" });
    expect(within(dialog).getByText("Both ends leave the model: School.users and User.school.")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(source()).toBe(withRelationship);

    await user.click(screen.getByRole("button", { name: "Remove the relationship users" }));
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Remove" }));
    expect(relationships(parseModel(source()))).toEqual([]);
    expect(screen.getByText("School has no relationship yet.")).toBeInTheDocument();
  });

  it("changes the cardinality and the delete rule from the row", async () => {
    const { source, user, openForm } = renderEditor();
    const add = within(await openForm());
    await user.selectOptions(add.getByLabelText("Target class"), "User");
    await user.click(add.getByRole("button", { name: "Add relationship" }));

    await user.selectOptions(screen.getByLabelText("Cardinality of users"), "many-to-many");
    await user.selectOptions(screen.getByLabelText("On delete of users"), "cascade");
    const [relationship] = relationships(parseModel(source()));
    expect(relationship).toMatchObject({ cardinality: "many-to-many", onDelete: "cascade" });
  });

  it("opens the form on a class picked as a range, and writes no one-sided slot", async () => {
    const { source, user, form } = renderEditor();
    await user.click(screen.getByRole("button", { name: "title" }));
    const range = screen.getByLabelText("Type of value");
    expect(within(range).getByRole("group", { name: "Links (opens Add a relationship)" })).toBeInTheDocument();

    await user.selectOptions(range, "User");
    expect(source()).toBe(SOURCE);
    // The field's dialog closes so the form under the page is the one in view.
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(within(form()).getByLabelText("Target class")).toHaveValue("User");
    expect(within(form()).getByLabelText("Name on School")).toHaveValue("users");
  });

  it("offers the inverse for a relationship saved without one (DM-73)", async () => {
    const oneSided = SOURCE.replace("      - title\n", "      - title\n      - enrolled\n").replace(
      "slots:\n  title:",
      "slots:\n  enrolled:\n    range: User\n    multivalued: true\n    annotations:\n      ngsi_ld_kind: Relationship\n  title:",
    );
    const { source, user } = renderEditor(oneSided);
    expect(screen.getByText("enrolled points at User but names no inverse, so it is no relationship yet.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Add the inverse schools on User" }));
    const [relationship] = relationships(parseModel(source()));
    // The slot holding the data today stays the stored end.
    expect(relationship).toMatchObject({ source: { class: "School", slot: "enrolled" }, target: { class: "User", slot: "schools" } });
  });

  it("shows a refusal as text", async () => {
    // `schools` already exists, so the inverse the button suggests is refused and the reason shown.
    const taken = SOURCE.replace("      - title\n", "      - title\n      - enrolled\n").replace(
      "slots:\n  title:",
      "slots:\n  schools:\n    range: string\n  enrolled:\n    range: User\n    annotations:\n      ngsi_ld_kind: Relationship\n  title:",
    );
    const { source, user } = renderEditor(taken);
    await user.click(screen.getByRole("button", { name: "Add the inverse schools on User" }));
    const alert = within(screen.getByRole("region", { name: "Relationships" })).getByRole("alert");
    expect(alert).toHaveTextContent("slot 'schools' already exists");
    expect(source()).toBe(taken);
  });

  it.each(SUPPORTED_LOCALES)("reads in %s with no raw key and no axe violation", async (locale) => {
    await i18n.changeLanguage(locale);
    const { view, user, openForm } = renderEditor();
    const add = within(await openForm());
    await user.selectOptions(add.getAllByRole("combobox")[0], "User");
    await user.click(add.getAllByRole("button").at(-1) as HTMLElement);
    expectNoRawKeys(view.container);
    await expectNoViolations(view.container);
  });
});
