/**
 * Unicode, over-long and hostile names on a create form, on dev (T-1591; MF-13, UI-04, R20).
 *
 * `ui/e2e/live/forms-checklist.spec.ts` submits every form *empty* by design, so no browser test
 * ever typed a name that is not a well-formed one. Server-side the shape is covered by
 * `src/resource/mod.rs::dns1123_validation_rules` (ASCII, the 63/64 boundary) and hostile input is
 * covered at the gateway's query surface
 * (`joinedcontext-conformance/tests/schemathesis/test_gateway_endpoints.py::test_ts09_hostile_query_parameters_never_crash`).
 * What nobody had tried on dev is a person typing these names into the Portal.
 *
 * The properties, for each name: it is accepted or refused **by the rule** — 63 characters is a
 * name and 64 is not — the refusal is tied to the field and says the rule in words, the value is
 * shown as text and never as markup, and the form stays inside 400 px with the longest of them in
 * it. The same names are then sent at the door, where a refusal must be a 400 naming the field and
 * never a 500.
 *
 * Nothing is proposed: the only button pressed inside a form is Check. A check does save a draft,
 * so every draft these names left is deleted and the deletion is *checked* — a cleanup that fails
 * quietly is how residue reaches dev (T-2236).
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { STEWARD, csrf, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = new Date().toISOString().slice(11, 19).replace(/:/g, "");

/** The rule a DNS-1123 label follows, in the words a form is allowed to use. */
const RULE = /lowercase|letters|digits|dash|hyphen|63|long|pattern|match|invalid|dns/i;

interface Name {
  /** What the case is, for the failure message. */
  what: string;
  value: string;
  /** Whether the platform's own rule accepts it. */
  accepted: boolean;
}

/** A 63-character label is the longest a name may be; 64 is one too many (RFC 1123). */
const AT_THE_LIMIT = `t1591-${SUFFIX}-`.padEnd(63, "a").slice(0, 63);
const OVER_THE_LIMIT = `${AT_THE_LIMIT}a`;

const NAMES: Name[] = [
  { what: "63 characters", value: AT_THE_LIMIT, accepted: true },
  { what: "64 characters", value: OVER_THE_LIMIT, accepted: false },
  { what: "Cyrillic", value: "охрана-данных", accepted: false },
  { what: "an emoji", value: "bikes-🚲", accepted: false },
  { what: "a trailing hyphen", value: `t1591-${SUFFIX}-`, accepted: false },
  { what: "a path", value: "../etc/passwd", accepted: false },
  { what: "a script tag", value: "<script>alert(1)</script>", accepted: false },
  { what: "a capital letter", value: `T1591-${SUFFIX}`, accepted: false },
];

test.setTimeout(1_800_000);

/** Opens the new-space dialog, which is the shortest create form with a name and a Check. */
async function openSpace(page: Page): Promise<Locator> {
  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: /^New (context )?space/i }).first().click();
  return page.getByTestId("form-page");
}

/** What the form says, from the places a person is told things: the field and the live regions. */
async function refusals(dialog: Locator): Promise<string> {
  const spoken = await dialog.locator("[role=alert], [role=status], [aria-live]").allInnerTexts();
  return spoken.join(" | ");
}

test("a name is accepted or refused by the rule, and the refusal is tied to the field", async ({
  browser,
}) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  // A name that is refused must never be executed: an `alert` would open a dialog, and a page
  // that opened one is a page that ran what was typed into it.
  const dialogs: string[] = [];
  page.on("dialog", async (opened) => {
    dialogs.push(opened.message());
    await opened.dismiss();
  });
  const wrong: string[] = [];
  try {
    for (const name of NAMES) {
      const dialog = await openSpace(page);
      const field = dialog.getByLabel(/^Name/);
      await field.fill(name.value);
      // The value really reached the form: a fill that silently did nothing would let every case
      // below pass on an empty form, which is the way a survey like this becomes decoration.
      expect(await field.inputValue(), `${name.what}: the form did not take the name`).toBe(
        name.value,
      );
      await dialog.locator("#root_dataModelRef").fill("helsinki").catch(() => undefined);
      const check = dialog.getByRole("button", { name: "Check", exact: true });
      await check.click();
      await page.waitForTimeout(2_000);

      const said = await refusals(dialog);
      const invalid = await dialog.locator("[aria-invalid=true]").count();
      if (name.accepted) {
        // Accepted means the rule let it through, and the check says so: a name at the limit is a
        // name, and a form that refused it would have made every case below meaningless.
        if (invalid > 0 && RULE.test(said)) {
          wrong.push(`${name.what}: refused although it is a name (${said.slice(0, 160)})`);
        }
        const verdict = dialog.getByTestId("draft-verdict");
        if ((await verdict.count()) > 0 && !/Checked/i.test((await verdict.innerText()) ?? "")) {
          wrong.push(`${name.what}: the check did not pass it (${await verdict.innerText()})`);
        }
      } else {
        if (invalid === 0) {
          wrong.push(`${name.what}: refused without marking the field (${said.slice(0, 160)})`);
        }
        if (!RULE.test(said)) {
          wrong.push(`${name.what}: the refusal does not say the rule (${said.slice(0, 160) || "it says nothing"})`);
        }
        // Shown as text: the value comes back in the words on screen, not as an element.
        if (name.value.includes("<script>")) {
          const injected = await page.locator("script:not([src])").filter({ hasText: "alert(1)" }).count();
          if (injected > 0) {
            wrong.push(`${name.what}: the value was rendered as markup`);
          }
        }
      }
      await page.keyboard.press("Escape");
    }

    expect(dialogs, "a typed script was executed by the page").toEqual([]);
    expect(wrong, "every name is accepted or refused by the rule, with the reason at the field").toEqual(
      [],
    );
  } finally {
    await sweepDrafts(
      context,
      page,
      PROJECT,
      /^t1591-/i,
      NAMES.map((one) => ({ kind: "ContextSpace", name: one.value })),
    );
    await context.close();
  }
});

test("the form holds the longest name inside 400 px", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  try {
    const dialog = await openSpace(page);
    await dialog.getByLabel(/^Name/).fill(OVER_THE_LIMIT);
    await dialog.getByRole("button", { name: "Check", exact: true }).click();
    await page.waitForTimeout(2_000);
    await page.setViewportSize({ width: 400, height: 900 });
    await page.waitForTimeout(500);

    // The refusal of a 64-character name is the longest thing the dialog ever has to fit.
    const overflow = await dialog.evaluate((element) => element.scrollWidth - element.clientWidth);
    expect(overflow, "the dialog overflows sideways with a long name and its refusal").toBeLessThanOrEqual(1);
    const pageOverflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(pageOverflow, "the page scrolls sideways at 400 px").toBeLessThanOrEqual(1);
    await page.keyboard.press("Escape");
  } finally {
    await sweepDrafts(
      context,
      page,
      PROJECT,
      /^t1591-/i,
      NAMES.map((one) => ({ kind: "ContextSpace", name: one.value })),
    );
    await context.close();
  }
});

test("the same names at the door are refused with the field named, never with a 500", async ({
  browser,
}) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const token = await csrf(context);
  const wrong: string[] = [];
  try {
    for (const name of NAMES.filter((candidate) => !candidate.accepted)) {
      const answer = await page.request.post(`/api/v1/projects/${PROJECT}/spaces`, {
        headers: { "x-csrf-token": token },
        data: {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "ContextSpace",
          metadata: { name: name.value, namespace: PROJECT },
          spec: { dataModelRef: "helsinki" },
        },
      });
      const said = await answer.text();
      if (answer.status() >= 500) {
        wrong.push(`${name.what}: the door answered ${answer.status()}`);
        continue;
      }
      if (answer.status() < 400) {
        wrong.push(`${name.what}: the door accepted it (${answer.status()})`);
        continue;
      }
      // Problem Details, and the field a person can go and fix — not "invalid manifest".
      if (!/name/i.test(said)) {
        wrong.push(`${name.what}: the refusal names no field (${said.slice(0, 200)})`);
      }
    }
    expect(wrong, "every hostile name is refused at the door by its field").toEqual([]);
  } finally {
    await context.close();
  }
});
