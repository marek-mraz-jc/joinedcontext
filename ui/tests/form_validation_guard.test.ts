/**
 * T-2731: every kind with a form has its validation suite, `form_validation.<kind>.test.tsx`
 * (tests/formValidation.tsx), and a kind that gains a form without one fails here (UI-16, PF-44).
 *
 * The kinds are read, not listed: every `draftKind` a page hands `ResourceFormDialog`, and every
 * kind the UI ships an arrangement for (src/schemas/forms). A kind with an arrangement and no
 * schema form of its own is excused below with what it is edited in instead, and the excuse
 * fails the day a schema form renders that kind.
 */
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { shippedForms } from "../src/schemas/forms";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** The kinds with an arrangement whose editor is not a schema form, and what edits them. */
const NO_SCHEMA_FORM: Record<string, string> = {
  App: "the app generator's conversation and the app's YAML (src/pages/apps)",
  DataModel: "the model editor, LinkML in Monaco (src/pages/models)",
  Mapping: "the mappings editor's table (src/pages/models/MappingsEditor.tsx)",
  Organization: "the organization settings tabs (src/pages/organization)",
  Project: "the project settings tabs (src/pages/projectSettings)",
};

function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sources(path);
    return entry.name.endsWith(".tsx") ? [path] : [];
  });
}

/** `draftKind="Role"`, or `draftKind={KIND}` with `const KIND = "…"` in the same file. */
function dialogKinds(): Set<string> {
  const kinds = new Set<string>();
  for (const path of sources(join(ui, "src"))) {
    const text = readFileSync(path, "utf8");
    if (!text.includes("<ResourceFormDialog")) continue;
    for (const [, literal, name] of text.matchAll(/draftKind=(?:"([A-Za-z]+)"|\{([A-Z_]+)\})/g)) {
      const kind = literal ?? new RegExp(`const ${name} = "([A-Za-z]+)"`).exec(text)?.[1];
      expect(kind, `${path}: draftKind={${name}} resolves to a kind`).toBeDefined();
      if (kind) kinds.add(kind);
    }
  }
  return kinds;
}

const file = (kind: string) => join(ui, "tests", `form_validation.${kind.toLowerCase()}.test.tsx`);

describe("every form kind has its validation suite (T-2731)", () => {
  const dialogs = dialogKinds();
  const arranged = shippedForms.map((form) => form.spec.for);

  it("reads the kinds from the pages and the arrangements", () => {
    // Twelve dialogs and eighteen arrangements on 2026-09-25: an empty read is a broken reader.
    expect(dialogs.size).toBeGreaterThanOrEqual(12);
    expect(arranged.length).toBeGreaterThanOrEqual(18);
  });

  it("has a file for every kind a form renders", () => {
    const kinds = [...new Set([...dialogs, ...arranged])].filter((kind) => !(kind in NO_SCHEMA_FORM));
    const missing = kinds.filter((kind) => !existsSync(file(kind)));
    expect(missing, "add tests/form_validation.<kind>.test.tsx: formValidationSuite({ kind, address })").toEqual([]);
  });

  it("excuses only kinds no schema form renders, and only kinds that still have an arrangement", () => {
    for (const kind of Object.keys(NO_SCHEMA_FORM)) {
      expect(dialogs.has(kind), `${kind} is rendered by ResourceFormDialog now: give it its suite`).toBe(false);
      expect(arranged, `${kind} lost its arrangement: drop the excuse`).toContain(kind);
    }
  });
});
