/**
 * T-2731: one validation suite for every kind's form, run by one thin file per kind
 * (`form_validation.<kind>.test.tsx`; UI-16, PF-44, AP-14).
 *
 * Each file opens the kind's real create form at its own address, through the App, with the API
 * answered here, and asks what the server's check asks of the same fields
 * (`tests/form_validation_api_tests.rs` proves the Portal answers each case):
 *
 * - Check on an empty name marks it next to the field, in words;
 * - a name that breaks the rule is marked on the field;
 * - the server's own sentence for a name another project holds (ADR-N-030) and for a reference
 *   that does not exist lands on the field its path names, word for word;
 * - a secret-shaped value the server refuses is never repeated back: not in the message, not in an
 *   alert, not in the console.
 *
 * `form_validation_guard.test.ts` fails on a form kind with no file.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { list, jsonResponse, renderRoute } from "./pageHarness";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

export interface KindForm {
  /** The kind as the manifest names it. */
  kind: string;
  /** The address the create form opens at. */
  address: string;
  /** Lists the form's pickers read: `{ "/spaces": [...] }`, by the end of the API path. */
  lists?: Record<string, unknown[]>;
  /** The button that opens the form, where the page has no address of its own for it. */
  button?: string;
  /**
   * A reference the kind holds, as the server's finding names it and as the form's field is
   * called (`spec.contextSpaceRef`, `root_contextSpaceRef`): the picker offers only what exists,
   * so a reference that is gone by the time of the check is the server's to say.
   */
  reference?: { path: string; field: string };
}

/** The field's own description, where a screen reader reads its message. */
function describedBy(field: HTMLElement): string {
  return (field.getAttribute("aria-describedby") ?? "")
    .split(/\s+/)
    .filter(Boolean)
    .map((id) => document.getElementById(id)?.textContent ?? "")
    .join(" ");
}

/** A red verdict as the Portal's check answers it (verdict_findings_tests.rs). */
function red(path: string, message: string) {
  return {
    valid: false,
    verdict: { ok: false, findings: [{ level: "error", path, message }], checkedAt: new Date().toISOString(), inputDigest: "sha256:0" },
  };
}

interface World {
  /** What a check answers; `undefined` answers a green verdict. */
  verdict?: ReturnType<typeof red>;
  checks: string[];
}

async function openForm(form: KindForm, world: World) {
  const rendered = await renderRoute({
    path: `${form.address}?lang=en`,
    answer: (path, request) => {
      const verdict = world.verdict ?? { valid: true, verdict: { ok: true, findings: [], checkedAt: "", inputDigest: "sha256:1" } };
      // A form checks by one of two doors: a dry run of the manifest, or its shared draft, which
      // the Portal answers with the verdict of what it holds (AG-61).
      if (request.method === "POST" && new URL(request.url).searchParams.get("dryRun") === "All") {
        world.checks.push(path);
        return jsonResponse(verdict);
      }
      const draft = /\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
      if (request.method === "PUT" && draft) {
        world.checks.push(path);
        return request
          .clone()
          .json()
          .then((body: { manifest?: unknown }) =>
            jsonResponse({
              kind: decodeURIComponent(draft[1]),
              name: decodeURIComponent(draft[2]),
              project: "helsinki",
              manifest: body.manifest ?? {},
              touchedBy: "jana.kovacova",
              touchedKind: "person",
              updatedAt: new Date().toISOString(),
              verdict: verdict.verdict,
              version: world.checks.length,
            }),
          );
      }
      for (const [suffix, items] of Object.entries(form.lists ?? {})) {
        if (path.endsWith(suffix)) return jsonResponse(list(items));
      }
      return undefined;
    },
  });
  // The header's button; an empty page offers the same one again as its next step.
  if (form.button !== undefined) {
    const [button] = await screen.findAllByRole("button", { name: form.button }, { timeout: 4000 });
    await userEvent.click(button);
  }
  return rendered;
}

/** The name field, by the id every schema form gives it: its label differs by kind ("Role name"). */
async function nameField(): Promise<HTMLElement> {
  return waitFor(
    () => {
      const field = document.getElementById("root_name");
      expect(field, "the form has a name field").not.toBeNull();
      return field as HTMLElement;
    },
    { timeout: 4000 },
  );
}

async function check(): Promise<void> {
  const button = await screen.findByRole("button", { name: "Check" });
  await userEvent.click(button);
}

export function formValidationSuite(form: KindForm): void {
  describe(`the ${form.kind} form validates as the server does (T-2731)`, () => {
    afterEach(() => {
      vi.restoreAllMocks();
      vi.unstubAllGlobals();
      window.localStorage.clear();
      window.sessionStorage.clear();
    });

    it("marks an empty name next to the field, in words", async () => {
      const world: World = { checks: [] };
      await openForm(form, world);
      const name = await nameField();
      await check();
      await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"), { timeout: 5000 });
      // "Required", or the name rule where the form starts the name as an empty string; a form
      // whose other fields are all there still asks the server (T-2634), and the mark stays.
      expect(describedBy(name)).toMatch(new RegExp(`${i18n.t("form.required")}|letters`));
    });

    it("marks a name that breaks the rule on the field", async () => {
      const world: World = { checks: [] };
      await openForm(form, world);
      const name = await nameField();
      await userEvent.type(name, "Air Quality!");
      await check();
      await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"), { timeout: 5000 });
      expect(describedBy(name).length).toBeGreaterThan(0);
    });

    it("puts the server's sentence for a name another project holds on the name, word for word", async () => {
      const sentence = `the name air-quality is held by the ${form.kind} of project espoo; names are unique in the organization`;
      const world: World = { checks: [], verdict: red("metadata.name", sentence) };
      await openForm(form, world);
      const name = await nameField();
      await userEvent.type(name, "air-quality");
      await check();
      await waitFor(() => expect(describedBy(name)).toContain(sentence), { timeout: 5000 });
      expect(name).toHaveAttribute("aria-invalid", "true");
    });

    it.runIf(form.reference !== undefined)("puts the server's sentence for a reference that does not exist on that field", async () => {
      const { path, field: id } = form.reference ?? { path: "", field: "" };
      const sentence = `${path} names air, which does not exist in project helsinki`;
      const world: World = { checks: [], verdict: red(path, sentence) };
      await openForm(form, world);
      await userEvent.type(await nameField(), "air-quality");
      await check();
      await waitFor(() => expect(describedBy(document.getElementById(id) as HTMLElement)).toContain(sentence), { timeout: 5000 });
    });

    it("marks a name over 63 characters on the field", async () => {
      const world: World = { checks: [] };
      await openForm(form, world);
      const name = await nameField();
      await userEvent.type(name, "a".repeat(64));
      await check();
      await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"), { timeout: 5000 });
      expect(describedBy(name).length).toBeGreaterThan(0);
    });

    it("never repeats a secret the server refused, in the message, an alert or the console", async () => {
      // Token-shaped and within the name rule, so it is the server's refusal that answers it.
      // Built from parts: a fixture, not a credential.
      const secret = ["sk", "live", "4f9a7c2e1b8d6a3f", "5e0c9b7a"].join("-");
      const errors = vi.spyOn(console, "error");
      const logs = vi.spyOn(console, "log");
      const world: World = {
        checks: [],
        verdict: red("metadata.name", "a literal secret was written into this field; reference it with a secretRef instead"),
      };
      await openForm(form, world);
      const name = await nameField();
      await userEvent.type(name, secret);
      await check();
      await waitFor(() => expect(describedBy(name)).toContain("secretRef"), { timeout: 5000 });
      for (const alert of screen.queryAllByRole("alert")) expect(within(alert).queryByText(new RegExp(secret))).toBeNull();
      expect(describedBy(name)).not.toContain(secret);
      const logged = [...errors.mock.calls, ...logs.mock.calls].flat().map(String).join(" ");
      expect(logged).not.toContain(secret);
    });
  });
}
