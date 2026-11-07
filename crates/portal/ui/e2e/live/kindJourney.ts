/**
 * One kind, end to end on dev, the way a person does it (T-1536…T-1548; UI-44, PF-50).
 *
 * The coverage measurement of 2026-09-18 found kinds named in unit, Rust and assistant tests and
 * in no live journey: a kind nobody ever created, changed and removed on dev by hand is not known
 * to work there. Each `kind-<plural>.spec.ts` hands this module what is particular to its kind —
 * the page a person creates one on and how its form is filled, how one is changed — and gets the
 * same three tests:
 *
 * 1. the steward creates one through the page, the approver approves it, it answers on its route
 *    with a phase that is not an error, one field is changed and approved, and it is removed again
 *    (a Red change the steward approves under the administrator exception of CC-34, as
 *    `removeCompletely` says);
 * 2. the steward asks the assistant to create one and is taken to the kind's form, and nothing is
 *    proposed on their behalf (AG-73);
 * 3. the viewer finds every write control of the kind's page disabled with the verb and the kind
 *    it lacks, and the create call made by hand answers 403 (UI-44, PF-50).
 *
 * What a journey makes is named `t<task>-<HHMM>`, which `residue.spec.ts` sweeps if a run dies
 * half way, and each test cleans up in `finally` whichever way it ends.
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import {
  APPROVER,
  STEWARD,
  VIEWER,
  approve,
  ask,
  csrf,
  hiddenSections,
  inHiddenSection,
  proposedChange,
  removeCompletely,
  signIn,
  sweepDrafts,
} from "./portal";

export const PROJECT = "helsinki";

export interface KindJourney {
  /** The task the journey is for, in lower case: `t1541`. Prefixes every name it makes. */
  task: string;
  kind: string;
  plural: string;
  /** The page the kind lives on for a person: where it is created and where a viewer looks. */
  page: string;
  /**
   * Opens the create form on `page`, fills it for `name` and submits it; the page then shows the
   * change it opened, which the journey reads. A kind whose page names the resource itself (a
   * shared reference is named after its source) returns the name it made.
   */
  create: (page: Page, name: string) => Promise<string | void>;
  /**
   * Changes one field of `name` and submits; `value` is what the stored resource then holds. Left
   * out only for a kind whose page offers no edit at all, and the spec then says which task that is.
   */
  change?: { apply: (page: Page, name: string) => Promise<void>; value: (name: string) => string };
  /** What the steward asks the assistant, and what shows that it opened the kind's form. */
  assistant: { ask: (name: string) => string; opened: (page: Page) => Locator };
  /** The page's write controls by name, where they are not New, Edit, Remove, Delete or Propose. */
  writeControls?: RegExp;
  /**
   * The kind lives in the organization namespace `org`, not in the project. The steward administers
   * the organization and proposes it; the approver approves across the organization (the seed's
   * `approvers` binding), so the proposer still approves nothing.
   */
  organization?: boolean;
}

const WRITE_CONTROLS = /^(New|Edit|Remove|Delete|Propose)(\b|$)/i;

/** A name nobody has used, recognisable as this journey's. */
export function journeyName(task: string): string {
  return `${task}-${new Date().toISOString().slice(11, 16).replace(":", "")}`;
}

/** Proposes from a routed form, running its check first when the form asks for a fresh one. */
export async function proposeFrom(form: Locator): Promise<void> {
  const propose = form.getByRole("button", { name: /^Propose/ }).first();
  if (await propose.isDisabled()) {
    await form.getByRole("button", { name: "Check", exact: true }).click();
  }
  await expect(propose).toBeEnabled({ timeout: 120_000 });
  await propose.click();
}

/** The form a create or an edit opened: a page of its own on a routed list, a dialog elsewhere (T-2474). */
export async function openedForm(page: Page): Promise<Locator> {
  const form = page.getByTestId("form-page").or(page.getByRole("dialog")).first();
  await expect(form).toBeVisible({ timeout: 30_000 });
  return form;
}

/** A reference field is a select when the project has something to offer, and a text box when not. */
export async function reference(field: Locator, fallback: string): Promise<void> {
  if ((await field.evaluate((element) => element.tagName)) === "SELECT") {
    await pickFirst(field);
  } else {
    await field.fill(fallback);
  }
}

/** Chooses the first real option of a select: the one a person would take when any will do. */
export async function pickFirst(select: Locator): Promise<string> {
  await expect(select).toBeVisible();
  const value = await select.evaluate((element) => {
    const options = [...(element as HTMLSelectElement).options];
    return options.find((option) => option.value !== "" && !option.disabled)?.value ?? "";
  });
  expect(value, "the select offers something to choose").not.toBe("");
  await select.selectOption(value);
  return value;
}

/** The kind's row menu on its list, then one of its items (`ResourceRowActions`). */
export async function rowAction(page: Page, name: string, item: "Edit" | "Remove"): Promise<void> {
  await page.getByRole("button", { name: `More actions for ${name}` }).first().click();
  await page.getByRole("menuitem", { name: item }).click();
}

/**
 * Changes a resource through its YAML editor, the only editor some kinds have (EditResourceDialog):
 * the stored manifest is read, `edit` changes its spec, and the whole of it is written back as JSON,
 * which is YAML too, so nothing is retyped key by key into Monaco. `open` clicks whatever opens the
 * editor on the page the person is on.
 */
export async function editAsYaml(
  page: Page,
  plural: string,
  name: string,
  open: () => Promise<void>,
  edit: (spec: Record<string, unknown>) => void,
): Promise<void> {
  const stored = await page.request.get(`/api/v1/projects/${PROJECT}/${plural}/${name}`);
  expect(stored.ok(), `read ${plural}/${name}`).toBe(true);
  const { status: _status, ...manifest } = (await stored.json()) as {
    spec: Record<string, unknown>;
    status?: unknown;
  };
  edit(manifest.spec);
  await open();
  // A routed list draws the edit form as a page of its own, a region named like the dialog (T-2474).
  const dialog = page
    .getByRole("region", { name: `Edit ${name}` })
    .or(page.getByRole("dialog", { name: `Edit ${name}` }))
    .first();
  const editor = dialog.locator(".monaco-editor .view-lines").first();
  await expect(editor).toBeVisible({ timeout: 60_000 });
  // Pasted, as `parity` and `secrets` do and as a person brings a whole document in: typed with
  // `keyboard.insertText`, Monaco indented every line again and closed the first brace itself, so
  // the text ended in one `}` too many and did not parse (T-2624).
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.evaluate((text) => navigator.clipboard.writeText(text), JSON.stringify(manifest, null, 2));
  await editor.click();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.press("ControlOrMeta+V");
  // A form that holds a draft proposes what its check passed (PF-57): Check first when it asks.
  await proposeFrom(dialog);
}

/** Waits until the resource answers on its route with a phase that is neither pending nor an error. */
async function settled(page: Page, project: string, plural: string, name: string): Promise<void> {
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${project}/${plural}/${name}`);
        return answer.ok() ? String((await answer.json()).status?.phase ?? "no phase yet") : `http ${answer.status()}`;
      },
      { timeout: 180_000, message: `${plural}/${name} is live` },
    )
    .toMatch(/^(?!http |no phase yet|Pending|Error|Failed|Degraded)/);
}

/** Every change of the project, as the approvals route lists them. */
async function changes(page: Page, project: string): Promise<string> {
  const answer = await page.request.get(`/api/v1/projects/${project}/changes`);
  return answer.ok() ? JSON.stringify((await answer.json()).items ?? []) : "";
}

export function kindJourney(journey: KindJourney): void {
  const { task, kind, plural } = journey;
  const project = journey.organization ? "org" : PROJECT;
  test.setTimeout(900_000);
  // A kind whose page the installation hides has no page to journey through (T-2874).
  test.beforeEach(async ({ request }) => {
    test.skip(inHiddenSection(journey.page, await hiddenSections(request)), "the installation hides this section (T-2874)");
  });

  test(`${kind}: created, changed and removed through the page by a person`, async ({ browser }) => {
    const steward = await signIn(browser, STEWARD, `${journey.page}?lang=en`);
    let name = journeyName(task);
    let created = false;
    try {
      name = (await journey.create(steward.page, name)) || name;
      const change = await proposedChange(steward.page);
      created = true;
      const approver = await signIn(browser, APPROVER, `/projects/${project}/approvals?lang=en`);
      try {
        await approve(approver.page, project, change, name);
        await settled(steward.page, project, plural, name);

        const { change: edit } = journey;
        if (edit) {
          await steward.page.goto(`${journey.page}?lang=en`, { waitUntil: "load" });
          await edit.apply(steward.page, name);
          await approve(approver.page, project, await proposedChange(steward.page), name);
          await expect
            .poll(
              async () =>
                JSON.stringify(
                  await (await steward.page.request.get(`/api/v1/projects/${project}/${plural}/${name}`)).json(),
                ),
              { timeout: 120_000, message: `the change reached ${plural}/${name}` },
            )
            .toContain(edit.value(name));
        }
      } finally {
        await approver.context.close();
      }
    } finally {
      if (created) {
        await removeCompletely(steward, project, plural, name);
        await expect
          .poll(
            async () => (await steward.page.request.get(`/api/v1/projects/${project}/${plural}/${name}`)).status(),
            { timeout: 120_000, message: `${plural}/${name} is removed` },
          )
          .toBe(404);
      }
      await sweepDrafts(steward.context, steward.page, project, new RegExp(`^${task}-`));
      await steward.context.close();
    }
  });

  test(`${kind}: the assistant opens the kind's form and proposes nothing`, async ({ browser }) => {
    const steward = await signIn(browser, STEWARD, `${journey.page}?lang=en`);
    const name = journeyName(`${task}a`);
    try {
      await ask(steward.page, journey.assistant.ask(name));
      await expect(journey.assistant.opened(steward.page)).toBeVisible({ timeout: 180_000 });
      // AG-73: the assistant drafts and the person proposes. No change names what it drafted.
      expect(await changes(steward.page, project), "the assistant proposed nothing").not.toContain(name);
    } finally {
      await sweepDrafts(steward.context, steward.page, project, new RegExp(`^${task}a-`));
      await steward.context.close();
    }
  });

  test(`${kind}: a viewer finds every write control disabled with its reason, and the door answers 403`, async ({
    browser,
  }) => {
    const viewer = await signIn(browser, VIEWER, `${journey.page}?lang=en`);
    try {
      const controls = viewer.page
        .getByRole("main")
        .getByRole("button", { name: journey.writeControls ?? WRITE_CONTROLS });
      let seen = 0;
      for (const control of await controls.all()) {
        if (!(await control.isVisible())) {
          continue;
        }
        seen += 1;
        await expect(control, "a viewer may not write").toBeDisabled();
        const described = await control.getAttribute("aria-describedby");
        const reason = described
          ? ((await viewer.page.locator(`[id="${described}"]`).first().textContent()) ?? "")
          : ((await control.locator("xpath=ancestor-or-self::*[@title][1]").first().getAttribute("title")) ?? "");
        expect(reason, "the control names what the viewer's role lacks (UI-44)").toMatch(
          /permit|role|propose|delete|not allowed|no binding/i,
        );
      }
      expect(seen, `${journey.page} offers the viewer the kind's write controls, disabled`).toBeGreaterThan(0);
      const answer = await viewer.page.request.post(`/api/v1/projects/${project}/${plural}`, {
        headers: { "x-csrf-token": await csrf(viewer.context) },
        data: {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind,
          metadata: { name: journeyName(`${task}v`), namespace: project },
          spec: {},
        },
      });
      expect(answer.status(), `creating a ${kind} as a viewer: ${await answer.text()}`).toBe(403);
    } finally {
      await viewer.context.close();
    }
  });
}
