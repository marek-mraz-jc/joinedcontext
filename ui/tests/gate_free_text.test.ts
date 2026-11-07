/**
 * T-4945: a text box whose label names something that exists (a space, a type, an Endpoint, a
 * person, a role) or a fixed set is a picker, a select or a combobox, not a box to type into.
 *
 * `free_text.ts` finds every text box of the Portal's pages with its label; this gate refuses a
 * new one whose label names such a thing unless `free_text.allow.json` explains it. The manifest
 * forms are held by `reference_pickers.test.ts` (REFERENCE_PICKERS), the one mechanism for rjsf.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  keyOf,
  stale,
  textBoxesIn,
  unexplained,
  type Allowed,
} from "./free_text";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const en = JSON.parse(
  readFileSync(join(ui, "src/locales/en.json"), "utf8"),
) as Record<string, unknown>;

/** The English text of an i18n key, or `undefined` for a key built at run time. */
function t(key: string): string | undefined {
  let node: unknown = en;
  for (const part of key.split("."))
    node = (node as Record<string, unknown> | undefined)?.[part];
  return typeof node === "string" ? node : undefined;
}

function walk(directory: string): string[] {
  return readdirSync(join(ui, directory), { withFileTypes: true }).flatMap(
    (entry) => {
      const path = `${directory}/${entry.name}`;
      if (entry.isDirectory()) return walk(path);
      return entry.name.endsWith(".tsx") ? [path] : [];
    },
  );
}

const boxes = walk("src").flatMap((file) =>
  textBoxesIn(file, readFileSync(join(ui, file), "utf8"), t),
);
const allowed = (
  JSON.parse(readFileSync(join(ui, "tests/free_text.allow.json"), "utf8")) as {
    fields: Record<string, Allowed>;
  }
).fields;

describe("the free-text gate (T-4945)", () => {
  it("finds the Portal's text boxes", () => {
    // 190 Input/Textarea tags on main on 2026-10-21, less the number, date and file ones.
    expect(boxes.length).toBeGreaterThan(150);
  });

  it("refuses a text box whose label names a picked thing, unless the allow-list says why", () => {
    expect(
      unexplained(boxes, allowed).map(
        (box) => `${box.file}:${box.line} "${box.label}"`,
      ),
      "use ResourcePicker, EntityPicker, a Select or SuggestWidgets; or add the field to tests/free_text.allow.json with its class and reason",
    ).toEqual([]);
  });

  it("drops an entry once its field is a picker", () => {
    expect(
      stale(boxes, allowed),
      "these fields are gone or became pickers: remove them from tests/free_text.allow.json",
    ).toEqual([]);
  });

  it("gives each waiting field its task, and each reference the server rule behind its choices", () => {
    const bad = Object.entries(allowed).filter(
      ([, entry]) =>
        !["FREE", "REF", "ENUM", "SUGGEST"].includes(entry.class) ||
        entry.reason.trim() === "" ||
        (entry.class === "REF" && !(entry.task && entry.rule)) ||
        (entry.class === "ENUM" && !entry.task),
    );
    expect(bad.map(([key]) => key)).toEqual([]);
  });

  it("fails a planted 'Context space' box and passes a planted 'Title' box", () => {
    const planted = `
      <Field id="space" label={t("planted.space")}>
        <Input id="space" value={space} onChange={(e) => setSpace(e.target.value)} />
      </Field>
      <Field id="title" label={t("planted.title")}>
        <Input id="title" value={title} onChange={(e) => setTitle(e.target.value)} />
      </Field>
      <Input type="number" aria-label="Context space count" />`;
    const english = (key: string) =>
      ({ "planted.space": "Context space", "planted.title": "Title" })[key];
    const found = textBoxesIn("src/pages/Planted.tsx", planted, english);
    expect(found.map((box) => box.label)).toEqual(["Context space", "Title"]);
    expect(unexplained(found, allowed).map(keyOf)).toEqual([
      "src/pages/Planted.tsx :: Context space",
    ]);
    const explained = {
      "src/pages/Planted.tsx :: Context space": {
        class: "FREE",
        reason: "planted",
      },
    } as const;
    expect(unexplained(found, explained)).toEqual([]);
  });
});
