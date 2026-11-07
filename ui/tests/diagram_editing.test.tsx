/**
 * T-3589: editing the model on its drawing. A relationship made by dragging from one class to
 * another, renamed and removed from its line, a class and a field added in place, the last edit
 * undone; each writes the same LinkML the forms' operations write, a breaking edit waits for a
 * confirmation, a person who may not propose the model sees no edit handle, an imported class
 * takes none, and names from the model stay text.
 */
import { cleanup, fireEvent, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { useDiagramEditing } from "../src/pages/models/DiagramEditing";
import { LinkmlGraphView } from "../src/pages/models/LinkmlGraphView";
import { ModelsPage } from "../src/pages/models/ModelsPage";
import { applyOperations } from "../src/pages/models/operations";
import type { Operation } from "../src/pages/models/operations";
import { expectNoRawKeys } from "./checks";
import { expectNoAxeViolations, inEveryLocale, renderPart } from "./page_contract";

const SCHOOL = `name: school
classes:
  School:
    slots: [title]
  Pupil:
    slots: [age]
slots:
  title:
    range: string
  age:
    range: integer
`;

const RELATED = `name: school
classes:
  School:
    slots: [pupils]
  Pupil:
    slots: [school]
slots:
  pupils:
    range: Pupil
    multivalued: true
    inverse: school
    annotations: { ngsi_ld_kind: Relationship, on_delete: restrict }
  school:
    range: School
    inverse: pupils
    annotations: { ngsi_ld_kind: Relationship }
`;

const PEOPLE = `name: people
classes:
  Person:
    slots: [label]
slots:
  label: { range: string }
`;

const WITH_IMPORT = `name: school
imports: [people]
classes:
  School:
    slots: [title]
slots:
  title: { range: string }
`;

/** The drawing over a source it holds itself, as the editor holds it; `written` sees every write. */
function Editable({ initial, written }: { initial: string; written?: (source: string) => void }) {
  const [source, setSource] = useState(initial);
  return (
    <>
      <LinkmlGraphView
        source={source}
        imports={{ people: PEOPLE }}
        onChange={(next) => {
          written?.(next);
          setSource(next);
        }}
      />
      <pre data-testid="source">{source}</pre>
    </>
  );
}

const sourceNow = () => screen.getByTestId("source").textContent ?? "";
const expected = (source: string, operations: Operation[]) => {
  const applied = applyOperations(source, operations);
  expect(applied.refused).toEqual([]);
  return applied.source;
};
const box = (name: string) => screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", name) });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("editing on the drawing", () => {
  it("relates two classes by dragging from one to the other, writing what the form writes", async () => {
    const user = userEvent.setup();
    renderPart(<Editable initial={SCHOOL} />);

    fireEvent.pointerDown(screen.getByTestId("diagram-connect-School"));
    fireEvent.pointerUp(box("Pupil"));

    const dialog = await screen.findByRole("dialog", { name: "New relationship from School" });
    const form = within(dialog).getByRole("form", { name: "Add a relationship from School" });
    expect(within(form).getByLabelText(en.models.relationships.add.target)).toHaveValue("Pupil");
    const name = (within(form).getByLabelText(/^Name on School/) as HTMLInputElement).value;
    const inverse = (within(form).getByLabelText(/^Inverse on Pupil/) as HTMLInputElement).value;
    await user.click(within(form).getByRole("button", { name: en.models.relationships.add.submit }));

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sourceNow()).toBe(
      expected(SCHOOL, [
        {
          op: "addRelationship",
          from: "School",
          to: "Pupil",
          name,
          inverse,
          cardinality: "one-to-many",
          required: false,
          inverseRequired: false,
          onDelete: "restrict",
        },
      ]),
    );
    expect(screen.getByRole("button", { name: /^Open the relationship/ })).toBeInTheDocument();
  });

  it("ends a drag anywhere else without a form", () => {
    renderPart(<Editable initial={SCHOOL} />);
    fireEvent.pointerDown(screen.getByTestId("diagram-connect-School"));
    fireEvent.pointerUp(screen.getByRole("group", { name: en.models.graph.title }));
    fireEvent.pointerUp(box("Pupil"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("renames a relationship from its line and changes how many, as renameSlot and setCardinality do", async () => {
    const user = userEvent.setup();
    renderPart(<Editable initial={RELATED} />);
    await user.click(screen.getByRole("button", { name: /^Open the relationship pupils \/ school/ }));

    const dialog = await screen.findByRole("dialog", { name: "Relationship pupils / school" });
    const name = within(dialog).getByLabelText(/^Name on School/);
    await user.clear(name);
    await user.type(name, "students");
    await user.selectOptions(within(dialog).getByLabelText(en.models.relationships.cardinality), "many-to-many");
    await user.click(within(dialog).getByRole("button", { name: en.models.graph.edit.apply }));

    // A rename is breaking (data stored under `pupils` no longer reads): it waits for a yes.
    const confirm = await screen.findByRole("alertdialog", { name: en.models.graph.edit.breakingTitle });
    expect(within(confirm).getByRole("list")).toHaveTextContent("pupils");
    await user.click(within(confirm).getByRole("button", { name: en.models.graph.edit.breakingConfirm }));

    expect(sourceNow()).toBe(
      expected(RELATED, [
        { op: "renameSlot", name: "pupils", to: "students" },
        { op: "setCardinality", name: "students", cardinality: "many-to-many" },
      ]),
    );
  });

  it("removes a relationship only after the breaking edit is confirmed, and Cancel keeps it", async () => {
    const user = userEvent.setup();
    const written = vi.fn();
    renderPart(<Editable initial={RELATED} written={written} />);

    await user.click(screen.getByRole("button", { name: /^Open the relationship pupils/ }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: en.models.relationships.remove }));
    let confirm = await screen.findByRole("alertdialog", { name: en.models.graph.edit.breakingTitle });
    await user.click(within(confirm).getByRole("button", { name: en.app.cancel }));
    expect(written).not.toHaveBeenCalled();
    expect(sourceNow()).toBe(RELATED);

    await user.click(screen.getByRole("button", { name: /^Open the relationship pupils/ }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: en.models.relationships.remove }));
    confirm = await screen.findByRole("alertdialog", { name: en.models.graph.edit.breakingTitle });
    await user.click(within(confirm).getByRole("button", { name: en.models.graph.edit.breakingConfirm }));
    expect(sourceNow()).toBe(expected(RELATED, [{ op: "removeRelationship", name: "pupils" }]));
    expect(screen.queryByRole("button", { name: /^Open the relationship/ })).toBeNull();
  });

  it("adds a class and a field in place, refuses a name the model has, and undoes the last edit", async () => {
    const user = userEvent.setup();
    renderPart(<Editable initial={SCHOOL} />);
    const undo = screen.getByRole("button", { name: en.models.graph.edit.undo });
    expect(undo).toHaveAttribute("aria-disabled", "true");

    await user.click(screen.getByRole("button", { name: en.models.graph.edit.addClass }));
    let dialog = await screen.findByRole("dialog", { name: en.models.graph.edit.addClassTitle });
    await user.type(within(dialog).getByLabelText(en.models.graph.edit.className), "Pupil");
    const taken = en.models.relationships.add.nameTaken.replace("{name}", "Pupil");
    // Said beside the field and as the refused button's own description (ui-writing rule 7).
    expect(within(dialog).getAllByText(taken).length).toBeGreaterThan(0);
    const add = within(dialog).getByRole("button", { name: en.models.graph.edit.add });
    expect(add).toHaveAttribute("aria-disabled", "true");
    expect(add).toHaveAccessibleDescription(taken);
    await user.clear(within(dialog).getByLabelText(en.models.graph.edit.className));
    await user.type(within(dialog).getByLabelText(en.models.graph.edit.className), "Teacher{Enter}");
    const withClass = expected(SCHOOL, [{ op: "addClass", name: "Teacher" }]);
    expect(sourceNow()).toBe(withClass);

    await user.click(screen.getByRole("button", { name: en.models.graph.edit.addField.replace("{name}", "Teacher") }));
    dialog = await screen.findByRole("dialog", { name: en.models.graph.edit.addFieldTitle.replace("{name}", "Teacher") });
    await user.type(within(dialog).getByLabelText(en.models.graph.edit.fieldName), "subject");
    await user.selectOptions(within(dialog).getByLabelText(en.models.graph.edit.fieldRange), "string");
    await user.click(within(dialog).getByRole("button", { name: en.models.graph.edit.add }));
    expect(sourceNow()).toBe(expected(withClass, [{ op: "addSlot", name: "subject", class: "Teacher", range: "string" }]));

    await user.click(undo);
    expect(sourceNow()).toBe(withClass);
    await user.click(undo);
    expect(sourceNow()).toBe(SCHOOL);
    expect(undo).toHaveAttribute("aria-disabled", "true");
  });

  it("offers no edits without somewhere to write them", () => {
    expect(renderHook(() => useDiagramEditing(SCHOOL)).result.current).toBeUndefined();
  });

  it("gives an imported class no edit handle", () => {
    renderPart(<Editable initial={WITH_IMPORT} />);
    expect(screen.getByTestId("diagram-connect-School")).toBeInTheDocument();
    expect(screen.queryByTestId("diagram-connect-Person")).toBeNull();
    expect(screen.queryByRole("button", { name: en.models.graph.edit.addField.replace("{name}", "Person") })).toBeNull();
    expect(screen.queryByRole("button", { name: en.models.graph.addRelationship.replace("{name}", "Person") })).toBeNull();
  });

  it("shows no edit handle without somewhere to write, and a line still opens its class", async () => {
    const user = userEvent.setup();
    const onOpenRelationship = vi.fn();
    renderPart(<LinkmlGraphView source={RELATED} onOpenRelationship={onOpenRelationship} />);
    expect(screen.queryByRole("button", { name: en.models.graph.edit.addClass })).toBeNull();
    expect(screen.queryByTestId("diagram-connect-School")).toBeNull();
    await user.click(screen.getByRole("button", { name: /^Open the relationship pupils/ }));
    expect(onOpenRelationship).toHaveBeenCalledExactlyOnceWith("School");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keeps a class name that arrives as markup as text in its dialogs", async () => {
    const user = userEvent.setup();
    const hostile = 'name: x\nclasses:\n  "<img src=x onerror=alert(1)>":\n    slots: []\n';
    renderPart(<Editable initial={hostile} />);
    await user.click(screen.getByRole("button", { name: en.models.graph.edit.addField.replace("{name}", "<img src=x onerror=alert(1)>") }));
    const dialog = await screen.findByRole("dialog", { name: "New field on <img src=x onerror=alert(1)>" });
    expect(dialog.querySelector("img")).toBeNull();
    expect(document.querySelector("img")).toBeNull();
  });

  it("is axe-clean and translated with its edit controls, in every locale", async () => {
    await inEveryLocale(async () => {
      const { container, unmount } = renderPart(<Editable initial={RELATED} />);
      expectNoRawKeys(container);
      unmount();
    });
    const { container } = renderPart(<Editable initial={RELATED} />);
    await expectNoAxeViolations(container);
  });
});

describe("who may edit (RBAC)", () => {
  const SOURCE = `name: air-quality
classes:
  Station:
    slots: [label]
slots:
  label:
    range: string
`;

  /** The models page with the editor's graph open, for a caller holding `verbs` on DataModel. */
  async function openAs(verbs: string[]) {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = typeof input === "string" ? input : input instanceof Request ? input.url : input.toString();
        if (url.includes("/permissions/me")) {
          return new Response(JSON.stringify({ grants: [{ rule: { kinds: ["DataModel"], verbs } }] }), { status: 200 });
        }
        if (url.includes("/datamodels/air-quality/source")) return new Response(SOURCE, { status: 200 });
        return new Response(JSON.stringify({ items: [] }), { status: 200 });
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const root = createRootRoute();
    const home = createRoute({
      getParentRoute: () => root,
      path: "/",
      component: () => (
        <QueryClientProvider client={client}>
          <ModelsPage project="ovzdusie" baseline={{ name: "air-quality", version: "1.0.0", lifecycle: "published" }} />
        </QueryClientProvider>
      ),
    });
    render(
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={createRouter({ routeTree: root.addChildren([home]) })} />
      </I18nextProvider>,
    );
    const user = userEvent.setup();
    await user.click(await screen.findByRole("tab", { name: en.models.view.graph }));
    await screen.findByRole("group", { name: en.models.graph.title });
  }

  it("draws the model for a reader with no edit handle", async () => {
    await openAs(["read"]);
    await waitFor(() => expect(screen.queryByRole("button", { name: en.models.graph.edit.addClass })).toBeNull());
    expect(screen.queryByTestId("diagram-connect-Station")).toBeNull();
    expect(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Station") })).toBeInTheDocument();
  });

  it("gives a person who may propose the model the edit handles", async () => {
    await openAs(["read", "propose"]);
    expect(await screen.findByRole("button", { name: en.models.graph.edit.addClass })).toBeInTheDocument();
    expect(screen.getByTestId("diagram-connect-Station")).toBeInTheDocument();
  });
});
