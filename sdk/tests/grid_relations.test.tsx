/**
 * A relationship is picked, never typed (T-2741, UI-84, DM-64): a single end holds one target of
 * its class, a many end several, a required end keeps its last one, the computed end is a
 * read-only list of what points back, and a refusal of the end is shown at its cell.
 */
import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, SourceError } from "../src/grid/source";
import type { EntitySource, GridQuery } from "../src/grid/source";
import { pointingAt, relationsOf, searchTargets } from "../src/relations";
import type { RelationEnd } from "../src/relations";

const S1 = "urn:ngsi-ld:School:bb.sk:skoly:gymnazium-jb";
const S2 = "urn:ngsi-ld:School:bb.sk:skoly:zs-sladkovicova";
const C1 = "urn:ngsi-ld:Course:bb.sk:skoly:fyzika";
const C2 = "urn:ngsi-ld:Course:bb.sk:skoly:chemia";
const U1 = "urn:ngsi-ld:User:bb.sk:skoly:jana";
const U2 = "urn:ngsi-ld:User:bb.sk:skoly:peter";

const named = (id: string, type: string, name: string, extra: Record<string, unknown> = {}) => ({
  id,
  type,
  name: { type: "Property", value: name },
  ...extra,
});

const entities: Record<string, unknown>[] = [
  named(S1, "School", "Gymnázium J. G. Tajovského"),
  named(S2, "School", "ZŠ Sládkovičova"),
  named(C1, "Course", "Fyzika"),
  named(C2, "Course", "Chémia"),
  named(U1, "User", "Jana", {
    school: { type: "Relationship", object: S1 },
    courses: { type: "Relationship", object: [C1] },
  }),
  named(U2, "User", "Peter", { school: { type: "Relationship", object: S2 } }),
];

const USER_ENDS: Record<string, RelationEnd> = {
  school: { target: "School", many: false, required: true },
  courses: { target: "Course", many: true, required: false },
};

function config(type: string, columns: string[], editable: string[]) {
  const parsed = parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type,
    columns: columns.map((attr) => ({ attr, label: attr })),
    mode: editable.length > 0 ? "edit" : "view",
    ...(editable.length > 0 ? { editableAttrs: editable } : {}),
    pageSize: 10,
  });
  expect(parsed.findings, JSON.stringify(parsed.findings)).toEqual([]);
  return parsed.config!;
}

/** The fixture source, recording what it is asked, taking writes, refusing where told. */
function recording(refuse?: (id: string) => SourceError | undefined, only?: (row: { id: string }) => boolean) {
  const inner = fixtureSource(entities);
  const asked: GridQuery[] = [];
  const written: { id: string; attrs: Record<string, unknown> }[] = [];
  const source: EntitySource = {
    async query(q, page) {
      asked.push(q);
      const answer = await inner.query(q, page);
      return only ? { rows: answer.rows.filter(only) } : answer;
    },
    get: inner.get,
    async patch(id, attrs) {
      written.push({ id, attrs });
      const problem = refuse?.(id);
      if (problem) throw problem;
    },
  };
  return { source, asked, written };
}

function userGrid(source: EntitySource) {
  render(<EntityGrid config={config("User", ["name", "school", "courses"], ["school", "courses"])} source={source} relations={USER_ENDS} />);
}

/** The picker of one cell: the row by its entity's name, the column by its end. */
async function picker(rowName: string, end: string) {
  const row = (await screen.findByText(rowName)).closest("tr")!;
  return within(row).getByRole("group", { name: `Edit ${end}` });
}

async function apply() {
  fireEvent.click(screen.getByRole("button", { name: "Review the changes" }));
  fireEvent.click(screen.getByRole("button", { name: "Apply" }));
}

describe("a single end", () => {
  it("shows its target, lists the target class, and a pick replaces it", async () => {
    const { source, written, asked } = recording();
    userGrid(source);
    const cell = await picker("Jana", "school");
    expect(within(cell).getByTitle(S1)).toBeInTheDocument();
    // A required end's last target has no remove button: the write it would make is refused.
    expect(within(cell).queryByRole("button", { name: /Remove/ })).toBeNull();

    fireEvent.focus(within(cell).getByRole("combobox", { name: "Search School" }));
    const option = await within(cell).findByRole("option", { name: /ZŠ Sládkovičova/ });
    // The current target is not offered again.
    expect(within(cell).queryByRole("option", { name: /Tajovského/ })).toBeNull();
    fireEvent.click(option);
    expect(asked.some((q) => q.type === "School")).toBe(true);

    await apply();
    await waitFor(() => expect(written).toHaveLength(1));
    expect(written[0]).toEqual({ id: U1, attrs: { school: { type: "Relationship", object: S2 } } });
  });

  it("is picked with the keyboard: arrows move, Enter picks, Escape closes", async () => {
    const { source } = recording();
    userGrid(source);
    const cell = await picker("Peter", "school");
    const box = within(cell).getByRole("combobox");
    fireEvent.focus(box);
    await within(cell).findByRole("option", { name: /Tajovského/ });
    fireEvent.keyDown(box, { key: "Escape" });
    expect(box).toHaveAttribute("aria-expanded", "false");
    fireEvent.keyDown(box, { key: "ArrowDown" });
    expect(box).toHaveAttribute("aria-expanded", "true");
    await within(cell).findByRole("option", { name: /Tajovského/ });
    expect(box.getAttribute("aria-activedescendant")).toBeTruthy();
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(within(cell).getByTitle(S1)).toBeInTheDocument());
    expect(cell).toHaveAttribute("data-changed", "true");
  });

  it("offers only the entities the source answers, searched by the text typed", async () => {
    // The endpoint lets this person read one school: the other is never offered.
    const { source, asked } = recording(undefined, (row) => row.id !== S2);
    userGrid(source);
    const cell = await picker("Peter", "school");
    const box = within(cell).getByRole("combobox");
    // The id matches, and still it is not offered: the endpoint did not answer it.
    fireEvent.change(box, { target: { value: "sladk" } });
    await waitFor(() => expect(asked.some((q) => q.idPattern === ".*sladk.*")).toBe(true));
    await waitFor(() => expect(within(cell).getByRole("status")).toHaveTextContent("Nothing you can read matches"));
    expect(within(cell).queryByRole("option")).toBeNull();
    // A typed regular-expression character is matched as itself.
    fireEvent.change(box, { target: { value: "a.b" } });
    await waitFor(() => expect(asked.some((q) => q.idPattern === ".*a\\.b.*")).toBe(true));
  });
});

describe("a many end", () => {
  it("adds a target to the ones it holds and writes the list", async () => {
    const { source, written } = recording();
    userGrid(source);
    const cell = await picker("Jana", "courses");
    fireEvent.focus(within(cell).getByRole("combobox", { name: "Search Course" }));
    fireEvent.click(await within(cell).findByRole("option", { name: /Chémia/ }));
    await apply();
    await waitFor(() => expect(written).toHaveLength(1));
    expect(written[0].attrs).toEqual({ courses: { type: "Relationship", object: [C1, C2] } });
  });

  it("removes its last target of an optional end as NGSI-LD's null", async () => {
    const { source, written } = recording();
    userGrid(source);
    const cell = await picker("Jana", "courses");
    fireEvent.click(within(cell).getByRole("button", { name: `Remove ${C1}` }));
    await apply();
    await waitFor(() => expect(written).toHaveLength(1));
    expect(written[0].attrs).toEqual({ courses: { type: "Relationship", object: "urn:ngsi-ld:null" } });
  });

  it("keeps the last target of a required many end", async () => {
    const { source } = recording();
    render(
      <EntityGrid
        config={config("User", ["name", "courses"], ["courses"])}
        source={source}
        relations={{ courses: { target: "Course", many: true, required: true } }}
      />,
    );
    const cell = await picker("Jana", "courses");
    expect(within(cell).queryByRole("button", { name: /Remove/ })).toBeNull();
    const box = within(cell).getByRole("combobox");
    expect(box).toHaveAttribute("aria-required", "true");
  });
});

describe("a refusal of the end", () => {
  it("is shown at its cell, in the endpoint's words", async () => {
    const detail = "`school` of `User` points at `…:gymnazium-jb`, which is no School of this space you may read";
    const { source } = recording((id) => (id === U2 ? new SourceError(400, detail, "school") : undefined));
    userGrid(source);
    const cell = await picker("Peter", "school");
    fireEvent.focus(within(cell).getByRole("combobox"));
    fireEvent.click(await within(cell).findByRole("option", { name: /Tajovského/ }));
    await apply();
    const box = await waitFor(() => {
      const found = within(cell).getByRole("combobox");
      expect(found).toHaveAttribute("aria-invalid", "true");
      return found;
    });
    expect(box).toHaveAccessibleDescription(detail);
    // The person's pick stays, unapplied, beside the reason.
    expect(within(cell).getByTitle(S1)).toBeInTheDocument();
  });
});

describe("a computed end", () => {
  it("lists the entities pointing back as links, from one read of the page", async () => {
    const asked: GridQuery[] = [];
    const inner = fixtureSource(entities);
    const source: EntitySource = {
      async query(q, page) {
        asked.push(q);
        if (q.type === "User") {
          // What a broker answers to `school=="…","…"`.
          const users = (await inner.query({ type: "User" }, page)).rows;
          return { rows: users };
        }
        return inner.query(q, page);
      },
      get: inner.get,
    };
    const opened: string[] = [];
    render(
      <EntityGrid
        config={config("School", ["name", "users"], [])}
        source={source}
        relations={{ users: { target: "User", many: true, required: false, inverseOf: "school" } }}
        onOpenRelationship={(urn) => opened.push(urn)}
      />,
    );
    const row = (await screen.findByText("ZŠ Sládkovičova")).closest("tr")!;
    fireEvent.click(await within(row).findByRole("button", { name: "Peter" }));
    expect(opened).toEqual([U2]);
    expect(within(row).queryByRole("combobox")).toBeNull();
    const reads = asked.filter((q) => q.type === "User");
    expect(reads).toHaveLength(1);
    expect(reads[0].q).toBe(`school=="${S1}","${S2}"`);
    // The computed end is not stored, so the filter row offers nothing for it.
    expect(screen.queryByRole("textbox", { name: /users/ })).toBeNull();
  });
});

describe("the readers", () => {
  it("reads the stored ends of a generated JSON Schema", () => {
    const schema = {
      required: ["school"],
      properties: {
        school: { type: "string", "x-ngsi-ld-relationship": { target: "School", cardinality: "many-to-one" } },
        courses: { type: "array", "x-ngsi-ld-relationship": { target: "Course" } },
        // An external reference names no target and stays a text field (DM-69).
        refDevice: { type: "string", "x-ngsi-ld-relationship": {} },
        name: { type: "string" },
      },
    };
    expect(relationsOf(schema)).toEqual({
      school: { target: "School", many: false, required: true },
      courses: { target: "Course", many: true, required: false },
    });
    expect(relationsOf(undefined)).toEqual({});
  });

  it("says when the computed end's read reached its limit", async () => {
    const source: Pick<EntitySource, "query"> = {
      async query() {
        return { rows: [1, 2].map((n) => ({ id: `u${n}`, type: "User", cells: { school: { kind: "relationship", value: undefined, object: S1 } }, raw: {} })) };
      },
    };
    const read = await pointingAt(source, "User", "school", [S1, S2], 2);
    expect(read.full).toBe(true);
    expect(read.byId[S1]?.map((one) => one.id)).toEqual(["u1", "u2"]);
    expect(read.byId[S2]).toBeUndefined();
    expect(await pointingAt(source, "User", "school", [])).toEqual({ byId: {}, full: false });
  });

  it("searches with no pattern when nothing is typed", async () => {
    const asked: GridQuery[] = [];
    const source: Pick<EntitySource, "query"> = {
      async query(q) {
        asked.push(q);
        return { rows: [] };
      },
    };
    await searchTargets(source, "School", "  ");
    expect(asked[0]).toEqual({ type: "School", idPattern: undefined });
  });
});
