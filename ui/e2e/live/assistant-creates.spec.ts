/**
 * The assistant opens each create form, filled from one sentence, on dev (T-1597; AG-45, UI-45, AG-77).
 *
 * Two of the six creates were already driven by the assistant and proved:
 * roles in `change.spec.ts` and endpoints in
 * `joinedcontext-conformance/e2e/journeys/17-assistant-navigation.spec.ts`. The four here — a context
 * space, a data source, a pipeline and a dashboard — were driven by nobody: `load.spec.ts` and
 * `analyse.spec.ts` create a source and two pipelines through the *forms*, and `change.spec.ts` asks
 * the assistant for edits and a removal, never a create.
 *
 * What each case holds: one sentence in the dock lands on the kind's own page with its form open,
 * the form carries what the sentence said (the name at least, and whatever else the sentence named),
 * and the person's Propose opens a change whose plan still holds those values — nothing the assistant
 * wrote is lost between the dock, the form and the change.
 *
 * Every proposal is rejected again, so dev keeps what it had: this journey creates nothing. A
 * rejection is also the honest end, because a create the assistant drafted is exactly the change a
 * person is supposed to decide (PF-58).
 *
 * What the first run measured on dev (2026-09-19, portal 4f03b8c): only the dashboard case was
 * green. The other three had no create path in the dock at all — it reached for a space propose tool
 * and filed a Change nobody had read, against AG-77 — which is T-2246; with that fix on dev
 * (portal c602705) all four are played here with nothing softened.
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { APPROVER, STEWARD, ask, hiddenSections, inHiddenSection, proposedChange, reject, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = new Date().toISOString().slice(11, 19).replace(/:/g, "");

test.setTimeout(1_800_000);

/**
 * The plan of a change, as the approvals route reads it: every file it would write with its fields.
 * The whole body, because a create of one kind writes more than one file — a dashboard carries its
 * layers — and `planFields` holds the fields of the first file alone.
 */
async function planOf(page: Page, change: string): Promise<string> {
  const detail = await page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
  expect(detail.ok(), `read change ${change}`).toBe(true);
  return JSON.stringify(await detail.json());
}

/** Proposes from the form the assistant opened, running the check the strict gate wants (PF-57). */
async function proposeFrom(form: Locator, label = /^Propose/): Promise<void> {
  const propose = form.getByRole("button", { name: label });
  if (await propose.isDisabled()) {
    await form.getByRole("button", { name: "Check", exact: true }).click();
  }
  await expect(propose).toBeEnabled({ timeout: 180_000 });
  await propose.click();
}

interface Create {
  /** The kind, for the test name. */
  what: string;
  /** What a person types into the dock. */
  sentence: (name: string) => string;
  /** The page the request must land on. */
  route: RegExp;
  /** Another field the sentence named, by the label a person reads, and what it must hold. */
  filled?: (name: string) => [RegExp | string, string][];
  /** A value the change's plan must still carry, beyond the name. */
  carries?: string;
  /**
   * Why this case stops at the filled form instead of proposing from it. A pipeline's mapping is
   * written in the editor, with the editor's own test beside it: the platform runs the drafted
   * mapping on one fetch of the source and, when it is not green, opens the form with the finding
   * and no verdict, so the strict gate refuses a proposal until the person has finished it (PF-57).
   * Proposing here would measure whether the model guessed Bloblang right, which is not what the
   * person is promised (AG-45).
   */
  handOver?: string;
  /** The section the case works in, where the installation may hide it (T-2874). */
  section?: string;
}

const CREATES: Create[] = [
  {
    what: "a context space",
    sentence: (name) => `Create a context space called ${name} in the helsinki project`,
    route: /\/projects\/helsinki\/spaces/,
  },
  {
    what: "a data source",
    // A URL the cluster can actually fetch: the platform fetches a new source's address once
    // before the form opens, and a made-up host is refused with "no such host" — correctly, and
    // the journey then measured nothing (2026-09-19). The city's own news feed is already read by
    // `hel-news-rss`, so nothing new is asked of the network.
    sentence: (name) =>
      `Add an HTTP data source called ${name} that reads https://www.hel.fi/en/news/rss`,
    route: /\/projects\/helsinki\/datasources/,
    carries: "hel.fi/en/news/rss",
  },
  {
    what: "a pipeline",
    // Names a data source the project really has: asked for "the helsinki-bikes data source",
    // which is an endpoint and not a source, the assistant rightly asked which source to read
    // and the journey waited for an answer nobody was there to give (measured 2026-09-19).
    // A JSON source: asked for `hel-news-rss`, which is RSS, the model wrote three JSON mappings
    // and the platform's own run of each on one fetch refused all three ("invalid character '<'").
    // The refusals were right; the sentence was wrong (2026-09-19).
    sentence: (name) =>
      `Create a pipeline called ${name} that reads the hsl-citybikes-gbfs-info data source every 15 minutes and writes into the helsinki space through the helsinki-all endpoint`,
    route: /\/projects\/helsinki\/pipelines/,
    handOver: "the mapping is finished in the editor, with its own test (PF-57)",
  },
  {
    // T-3268: the fourth of the kinds a person asks the dock to fill; the conformance journey 17
    // asks for one too, but in a sentence that names no name, so it proves the navigation alone.
    what: "an endpoint",
    sentence: (name) =>
      `Create an endpoint called ${name} that serves the helsinki space as NGSI-LD to this project only`,
    route: /\/projects\/helsinki\/endpoints/,
  },
  {
    what: "a dashboard",
    sentence: (name) => `Make a dashboard called ${name} showing the bikes of the helsinki space`,
    route: /\/projects\/helsinki\/dashboards/,
    section: "dashboards",
  },
];

for (const create of CREATES) {
  test(`the assistant opens the form for ${create.what}, filled from one sentence`, async ({
    browser,
    request,
  }, info) => {
    const hidden = await hiddenSections(request);
    test.skip(
      create.section !== undefined && inHiddenSection(`/projects/${PROJECT}/${create.section}`, hidden),
      "the installation hides this section (T-2874)",
    );
    // Starts away from every route under test: a start page that is already the target would let a
    // case pass on a request the assistant ignored (the space case did, on the first run).
    const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/activity?lang=en`);
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    const name = `t1597-${create.what.replace(/[^a-z]/g, "")}-${SUFFIX}`.slice(0, 40);
    let change = "";
    try {
      await ask(steward.page, create.sentence(name));
      await expect(steward.page, "the request lands on the kind's own page").toHaveURL(create.route, {
        timeout: 300_000,
      });
      const form = steward.page.getByTestId("form-page");
      await expect(form, "with the form open, not on the list alone").toBeVisible({
        timeout: 60_000,
      });

      // What the sentence said is already in the form: an assistant that navigated and filled
      // nothing would leave the person to type it all again (AG-45). The name is read by its id,
      // not by its label: each kind labels it differently — a Pipeline's name field is labelled
      // "Pipeline" — and a label matched loosely found a heading instead of an input (2026-09-19).
      await expect(form.locator("#root_name"), "the name is filled").toHaveValue(
        new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "i"),
        { timeout: 60_000 },
      );
      for (const [label, value] of create.filled?.(name) ?? []) {
        await expect(form.getByLabel(label).first(), `${label} is filled`).toHaveValue(
          new RegExp(value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "i"),
          { timeout: 60_000 },
        );
      }

      // What the person sees before deciding anything: the draft, every field, nothing saved (T-3268).
      await info.attach(`draft-${create.what.replace(/[^a-z]/g, "")}.png`, {
        body: await steward.page.screenshot(),
        contentType: "image/png",
      });

      if (create.handOver) {
        // The form is open and filled, which is what the sentence promised; the rest of this
        // resource is the person's work in the editor.
        return;
      }

      await proposeFrom(form);
      change = await proposedChange(steward.page);
      expect(change).toMatch(/^chg-/);

      // And the change still holds it: nothing is lost between the form and the plan.
      const plan = await planOf(steward.page, change);
      expect(plan, `the plan carries the name`).toContain(name);
      if (create.carries) {
        expect(plan, `the plan carries what the sentence named`).toContain(create.carries);
      }
    } finally {
      if (change) {
        await reject(approver.page, PROJECT, change);
      }
      // Every draft the assistant kept for this journey goes with it: drafts left on dev are what
      // blew the assistant's own context window once (T-2248, T-2249).
      await sweepDrafts(steward.context, steward.page, PROJECT, /^t1597-/i);
      await steward.context.close();
      await approver.context.close();
    }
  });
}
