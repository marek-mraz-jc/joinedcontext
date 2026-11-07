import axe from "axe-core";
import { expect, vi } from "vitest";
import { digestOf } from "../src/api/drafts";

type Fetch = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

/**
 * Every page checks a manifest before it proposes it (PF-57, T-0956), so a test's fetch stub
 * sees a `?dryRun=All` request before each proposal. This answers those checks green and records
 * them (`METHOD /path`), and hands every other request to the test's own stub unchanged, so what
 * a test asserts about its proposals still counts proposals only.
 */
export function answeringChecks(inner: Fetch): Fetch & { checks: string[] } {
  const checks: string[] = [];
  const wrapped = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const url = new URL(request ? request.url : String(input), window.location.origin);
    if (url.searchParams.get("dryRun") === "All") {
      checks.push(`${request?.method ?? init?.method ?? "GET"} ${url.pathname}`);
      // A verdict fresh for what was checked, so a form that holds a draft reads it green.
      let inputDigest: string | undefined;
      try {
        const body = (await (request ? request.clone().text() : Promise.resolve(String(init?.body ?? "")))) || "{}";
        const manifest = JSON.parse(body) as Record<string, unknown>;
        delete manifest.draft;
        inputDigest = digestOf(manifest);
      } catch {
        inputDigest = undefined;
      }
      const verdict = { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest };
      return new Response(JSON.stringify({ valid: true, verdict }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    return inner(input, init);
  });
  return Object.assign(wrapped, { checks });
}

/** The checks the current `fetch` stub answered, when it was wrapped by `answeringChecks`. */
export function checksSoFar(): string[] {
  return (globalThis.fetch as unknown as { checks?: string[] }).checks ?? [];
}

/**
 * A control the caller may not use (UI-44): refused, reachable, and carrying its reason.
 *
 * `PermissionGuard` and `Button`'s `disabledReason` make such a control `aria-disabled` rather
 * than `disabled`, because a hard-disabled button leaves the tab order and the reason written
 * for it can then never be read (T-1743). Every test that used to assert `toBeDisabled()` on a
 * guarded control asserts this instead.
 */
export function expectDenied(control: HTMLElement, reason?: string | RegExp): void {
  expect(control, "a denied control is aria-disabled, not disabled").toHaveAttribute(
    "aria-disabled",
    "true",
  );
  expect(control, "and it keeps its place in the tab order").not.toBeDisabled();
  if (reason !== undefined) {
    expect(control).toHaveAccessibleDescription(reason);
  }
}

/**
 * A control that is open for use: neither hard-disabled nor refused.
 *
 * The counterpart of {@link expectDenied}. `toBeEnabled()` alone stopped meaning this once a
 * gated submit became `aria-disabled` instead of `disabled` (UI-44, T-1743): the attribute it
 * looks at is no longer the one that closes the button.
 */
export function expectOpen(control: HTMLElement): void {
  expect(control, "an open control is not disabled").not.toBeDisabled();
  expect(control, "and it is not refused either").not.toHaveAttribute("aria-disabled", "true");
}

/**
 * axe over one rendered subtree (UI-16).
 *
 * The form tasks of the `ui-forms` group each run this on their own page, so a violation names
 * the rule and the element instead of failing somewhere inside the whole-app run of `a11y`.
 */
export async function expectNoViolations(
  container: HTMLElement,
  /**
   * Selectors left out of the run, for a violation that belongs to a file of its own task: the
   * caller names the task in a comment, so nothing is quietly excluded for ever.
   */
  exclude: string[] = [],
): Promise<void> {
  const context =
    exclude.length === 0
      ? (container as unknown as axe.ElementContext)
      : ({ include: [container], exclude: exclude.map((selector) => [selector]) } as unknown as axe.ElementContext);
  const results = await axe.run(context);
  const summary = results.violations
    .map((violation) => {
      const where = violation.nodes.map((node) => node.html).join("; ");
      return `${violation.id}: ${violation.description} (${where})`;
    })
    .join("\n");
  expect(results.violations, summary).toEqual([]);
}

const FOCUSABLE = [
  "a[href]",
  // A disclosure's own summary is a stop of its own, as the browser and jsdom tab to it (T-3220
  // put the form help in a <details> on every edit dialog).
  "details > summary:first-of-type",
  "button:not([disabled])",
  "input:not([disabled]):not([type=hidden])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

/**
 * The controls Tab walks in a subtree, in DOM order (UI-15).
 *
 * `aria-disabled` controls stay in the list: a control the caller may not use keeps its place in
 * the tab order so the reason written for it can be read (T-1743, `expectDenied`).
 */
export function focusables(container: HTMLElement): HTMLElement[] {
  // jsdom has no layout, so `offsetParent` is null for everything here; what is hidden from the
  // keyboard is hidden in the markup instead — `hidden`, `aria-hidden`, or a closed `<details>`.
  // A tab list is one stop: the selected tab holds it and the others are `tabindex="-1"`, reached
  // by the arrow keys (the roving tabindex of `Tabs`). Only an unselected tab is exempt, so a
  // button someone took out of the tab order by mistake still fails.
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (element) =>
      element.closest("[hidden],[aria-hidden=true]") === null &&
      !(element.getAttribute("role") === "tab" && element.getAttribute("tabindex") === "-1"),
  );
}

/**
 * Tab from the start of a subtree and assert focus visits every control in the order the DOM
 * has them, which is the order they are read in (UI-15). A control that is not reachable, or one
 * that is reached out of turn, fails with the element that broke the order.
 */
export async function expectTabOrder(
  user: { tab: () => Promise<void> },
  container: HTMLElement,
): Promise<void> {
  const expected = focusables(container);
  expect(expected.length, "a form with no reachable control is a form nobody can fill").toBeGreaterThan(0);
  const start = expected[0];
  start.focus();
  expect(document.activeElement, "the first control takes focus").toBe(start);
  for (const control of expected.slice(1)) {
    await user.tab();
    expect(
      document.activeElement,
      `Tab should reach ${control.outerHTML.slice(0, 120)}`,
    ).toBe(control);
  }
}

/**
 * The dotted path of a translation key, as it looks on screen when the string is missing.
 *
 * Two segments are enough (`quota.title`), because that is what most of the bundle looks like;
 * a word with a full stop after it is not a key, so the last segment may not end the sentence.
 */
const RAW_KEY = /^[a-z][A-Za-z0-9]*(\.[A-Za-z0-9_]+)+$/;

/**
 * No visible text is a raw translation key (UI-48): a key that reached the screen is a string
 * missing from the bundle of the locale under test, in every locale the organisation offers.
 *
 * `exclude` names the subtrees that render a dotted *identifier* rather than a sentence — an
 * activity kind (`access.denied`), an entity type, a JSON path — which no bundle translates and
 * which is shaped exactly like a key. As with {@link expectNoViolations}, the caller says in a
 * comment what the subtree holds, so nothing is quietly exempted.
 */
export function expectNoRawKeys(container: HTMLElement, exclude: string[] = []): void {
  const exempt = exclude.flatMap((selector) =>
    Array.from(container.querySelectorAll<HTMLElement>(selector)),
  );
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  const leaked: string[] = [];
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    const text = (node.textContent ?? "").trim();
    if (RAW_KEY.test(text) && !exempt.some((element) => element.contains(node))) {
      leaked.push(text);
    }
  }
  expect(leaked, "a translation key on the screen is a missing string").toEqual([]);
}
