/**
 * A secret cannot be typed into the Portal, on dev (T-1590; MF-24, MF-35, PL-16).
 *
 * The task asked for "a secret typed into any field, refused and never echoed". Reading the forms
 * first changed the scenario: **no form offers a field for a secret's value.** A credential-bearing
 * field renders as the `secretRef` widget — a secret's name and the key inside it, two text boxes
 * and no third one — and the hint beside it says so. So the first property is an absence, and the
 * regression worth catching is a form that grew a value field.
 *
 * The other two roads a value can still travel are what the rest of this journey walks:
 * the YAML view of the same dialog, where a person can write anything, and the secret *name* box,
 * where a person pastes the token itself instead of naming the secret that holds it. Both are
 * refused, the refusal names the field, and neither the refusal nor anything the server keeps
 * carries the value: `find_literal_secret` (`src/api/mutate.rs`) names the key and never its
 * content, and the runner's own rule (`data_source.rs::validate_runner`) demands a `${VAR}`
 * interpolation in every field the input documents as a secret.
 *
 * Server-side each door has its own test (`workspace_api_tests.rs`, `ops_resource_tests.rs`,
 * `datasources_api_tests.rs`, `drafts_tests.rs::literal_secret_refused_at_put`); no browser test
 * walked any of it, which is what this adds.
 *
 * Nothing is approved, so nothing is left behind: the draft is discarded and the dialog closed.
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { STEWARD, csrf, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = new Date().toISOString().slice(11, 19).replace(/:/g, "");
/** The value that must never come back: distinctive enough to grep the whole answer for. */
const VALUE = `t1590-supersecret-${SUFFIX}`;

test.setTimeout(300_000);

/** Opens the new-data-source dialog for one type; the type is chosen first, as the page requires. */
async function openDataSource(page: Page, type: string, name: string): Promise<Locator> {
  await page.goto(`/projects/${PROJECT}/datasources?lang=en`, { waitUntil: "load" });
  await page.getByLabel("Type").selectOption(type);
  await page.getByRole("button", { name: "New data source" }).click();
  const dialog = page.getByTestId("form-page");
  await dialog.locator("#root_name").fill(name);
  return dialog;
}

/**
 * Every text box of a dialog that a credential could be typed into, by the name a person reads.
 * A box whose name says `name`, `key` or `reference` is the reference widget doing its job; one
 * that asks for the credential itself is the defect.
 */
async function credentialBoxes(dialog: Locator): Promise<string[]> {
  const boxes = dialog.locator("input:not([type='hidden']), textarea");
  const count = await boxes.count();
  const offending: string[] = [];
  for (let index = 0; index < count; index += 1) {
    const box = boxes.nth(index);
    const described = [
      await box.getAttribute("placeholder"),
      await box.getAttribute("aria-label"),
      await box.getAttribute("id"),
      await box.getAttribute("name"),
    ]
      .filter(Boolean)
      .join(" ");
    if (!/password|token|secret|credential|api_?key|passphrase/i.test(described)) {
      continue;
    }
    // A name, a key or a reference is the widget doing its job; a URL that has `token` in it
    // (`oauth2.token_url`) is an address and not a credential.
    if (!/name|key|ref|url|endpoint|user/i.test(described)) {
      offending.push(described);
    }
  }
  return offending;
}

/** Fills the first item of an array field, adding it only when the form starts with none. */
async function fillFirst(dialog: Locator, field: string, value: string): Promise<void> {
  const first = dialog.locator(`#root_${field}_0`);
  if ((await first.count()) === 0) {
    await dialog.locator(`#root_${field}__add`).click();
  }
  await first.fill(value);
}

test("no form offers a field for a secret's value, on any data source type", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
  const offending: string[] = [];
  try {
    // The four typed connections a person picks first, and one runner input whose catalog
    // documents a password (`http_client`: `basic_auth.password`, `bento_inputs::SECRET_FIELDS`).
    for (const type of ["mqtt", "http", "websocket", "gtfs-rt", "http_client"]) {
      const dialog = await openDataSource(page, type, `t1590-${type.replace(/_/g, "-")}-${SUFFIX}`);
      // The advanced fields carry the credentials on a runner input, so they are on screen too.
      const advanced = dialog.getByRole("button", { name: /advanced/i });
      if (await advanced.count()) {
        await advanced.first().click();
      }
      // A masked box is a value box: the platform has nothing to mask, because it takes no values.
      expect(await dialog.locator("input[type='password']").count(), `${type}: a masked input`).toBe(
        0,
      );
      offending.push(...(await credentialBoxes(dialog)).map((box) => `${type} → ${box}`));
      await page.keyboard.press("Escape");
    }

    // The hint is the promise the form makes to the person, so it is asserted where it is made.
    const mqtt = await openDataSource(page, "mqtt", `t1590-hint-${SUFFIX}`);
    await expect(mqtt.locator("#root_mqtt_passwordRef_name")).toBeVisible();
    await expect(mqtt.locator("#root_mqtt_passwordRef_key")).toBeVisible();
    expect(await mqtt.locator("#root_mqtt_password").count(), "a value box beside the name and key").toBe(0);
    await page.keyboard.press("Escape");

    expect(offending, "a credential box that asks for the value itself, not a reference").toEqual([]);
  } finally {
    // Every form this journey opened left a draft; dev keeps none of them (T-2249).
    await sweepDrafts(context, page, PROJECT, /^t1590-/i);
    await context.close();
  }
});

test("a secret written into the YAML view is refused by its field, and the value is never echoed", async ({
  browser,
}) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
  const name = `t1590-yaml-${SUFFIX}`;
  try {
    const dialog = await openDataSource(page, "http_client", name);
    // The two views are tabs, not buttons (`ResourceFormDialog`, `role="tablist"`).
    await dialog.getByRole("tab", { name: "YAML", exact: true }).click();
    const editor = dialog.locator(".monaco-editor").first();
    await expect(editor).toBeVisible({ timeout: 60_000 });
    // Pasted, not typed: the editor closes brackets and quotes as it goes, and a typed document
    // came back as YAML that does not parse — which made the test about the editor and not about
    // the refusal. A paste is also what a person does with a manifest.
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.evaluate(
      (text) => navigator.clipboard.writeText(text),
      JSON.stringify(
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "DataSource",
          metadata: { name, namespace: PROJECT },
          spec: {
            type: "http_client",
            input: {
              url: "https://example.org/feed.json",
              basic_auth: { enabled: true, username: "reader", password: VALUE },
            },
          },
        },
        null,
        1,
      ),
    );
    await editor.click();
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.press("ControlOrMeta+v");

    // The refusal comes from the server, whichever road runs first — the draft the dialog saves
    // (`literal_secret_refused_at_put`) or the check the person presses.
    await dialog.getByRole("tab", { name: "Form", exact: true }).click();
    const check = dialog.getByRole("button", { name: "Check", exact: true });
    if (await check.count()) {
      await check.first().click();
    }
    // Wherever the dialog puts it: the footer error, a field's alert or the verdict's findings.
    const refusal = dialog.locator("[data-testid=footer-error], [role=alert]");
    await expect(refusal.first(), "the literal secret is refused").toBeVisible({ timeout: 120_000 });
    const said = (await refusal.allInnerTexts()).join(" | ");
    // The field, so the person knows what to replace with a reference; never the value.
    expect(said, `the refusal names the field: ${said}`).toMatch(/password|secret/i);
    expect(said, "the refusal does not echo the value").not.toContain(VALUE);

    // And nothing the server keeps holds it: not the draft, not a change, not the activity feed.
    for (const route of [
      `/api/v1/projects/${PROJECT}/drafts`,
      `/api/v1/projects/${PROJECT}/changes`,
      `/api/v1/projects/${PROJECT}/activity`,
    ]) {
      const answer = await page.request.get(route);
      if (!answer.ok()) {
        continue;
      }
      expect(await answer.text(), `${route} holds the value`).not.toContain(VALUE);
    }
  } finally {
    await page.keyboard.press("Escape").catch(() => undefined);
    // A DELETE without the CSRF header answers 403 and cleans nothing, which is how this
    // journey's probe drafts stayed on dev (T-2249): the sweep carries the header and checks.
    await sweepDrafts(context, page, PROJECT, /^t1590-/i, [{ kind: "DataSource", name }]);
    await context.close();
  }
});

test("a token pasted into the secret name is refused by the name of the field", async ({
  browser,
}) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
  const name = `t1590-name-${SUFFIX}`;
  const token = `ghp_${"A".repeat(36)}`;
  try {
    // The door first, because it is the rule and the form's check is a courtesy: a name that is
    // not a DNS-1123 label cannot be committed, and this one *is* the credential itself.
    const answer = await page.request.post(`/api/v1/projects/${PROJECT}/datasources?dryRun=All`, {
      headers: { "x-csrf-token": await csrf(context) },
      data: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataSource",
        metadata: { name: `${name}-door`, namespace: PROJECT },
        spec: {
          type: "mqtt",
          mqtt: {
            urls: ["tcp://mqtt.example.org:1883"],
            topics: ["helsinki/#"],
            passwordRef: { name: token, key: "password" },
          },
        },
      },
    });
    // The check is the door here: a proposal without a fresh verdict is refused before anything is
    // read (PF-57), so a REST client asks for the dry run — the same call the form's Check makes.
    const said = await answer.text();
    const verdict = answer.ok() ? ((await answer.json()) as { valid?: boolean }) : undefined;
    expect(
      answer.status() === 400 || verdict?.valid === false,
      `the check took a token as a secret name: ${answer.status()} ${said}`,
    ).toBe(true);
    expect(said, "the refusal does not echo the token").not.toContain(token);

    const dialog = await openDataSource(page, "mqtt", name);
    await fillFirst(dialog, "mqtt_urls", "tcp://mqtt.example.org:1883");
    await fillFirst(dialog, "mqtt_topics", "helsinki/#");
    // The mistake: the token itself where the name of the secret holding it belongs.
    await dialog.locator("#root_mqtt_passwordRef_name").fill(token);
    await dialog.locator("#root_mqtt_passwordRef_key").fill("password");
    await dialog.getByRole("button", { name: "Check", exact: true }).click();

    // A DNS-1123 label is what a secret name is, so the refusal is about the name and says which
    // field: the person learns where the token goes instead, rather than that something is invalid.
    const refusal = dialog.locator("[data-testid=footer-error], [role=alert]");
    await expect(refusal.first(), "a token is not a secret name").toBeVisible({ timeout: 120_000 });
    const shown = (await refusal.allInnerTexts()).join(" | ");
    expect(shown, `the refusal names the field: ${shown}`).toMatch(/name|passwordRef|secret/i);
    // Even a refused name is a credential once it is a real token: it is not echoed either.
    expect(shown, "the refusal does not echo the token").not.toContain(token);

  } finally {
    await page.keyboard.press("Escape").catch(() => undefined);
    // A DELETE without the CSRF header answers 403 and cleans nothing, which is how this
    // journey's probe drafts stayed on dev (T-2249): the sweep carries the header and checks.
    await sweepDrafts(context, page, PROJECT, /^t1590-/i, [{ kind: "DataSource", name }]);
    await context.close();
  }
});
