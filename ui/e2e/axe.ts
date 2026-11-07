import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";

interface AxeViolation {
  id: string;
  impact: string | null;
  help: string;
  nodes: { target: string[] }[];
}

interface AxeResults {
  violations: AxeViolation[];
}

interface AxeWindow {
  axe: { run: (context: Document) => Promise<AxeResults> };
}

// axe-core is already a devDependency for the component tests; injecting its bundle keeps the
// e2e suite from carrying a second copy via @axe-core/playwright.
const axeSource = readFileSync(
  createRequire(import.meta.url).resolve("axe-core/axe.min.js"),
  "utf8",
);

/** Runs axe over the current page and returns a readable list of violations (empty when clean). */
export async function axeViolations(page: Page): Promise<string[]> {
  // Through the protocol, not a <script> tag: the preview sends the Portal's Content Security
  // Policy, which forbids an inline script but not what the test driver evaluates.
  await page.evaluate(axeSource);
  return page.evaluate(async () => {
    const { axe } = window as unknown as AxeWindow;
    const results = await axe.run(document);
    return results.violations.map(
      (violation) =>
        `${violation.id} (${violation.impact ?? "unknown"}): ${violation.help} — ` +
        violation.nodes.map((node) => node.target.join(" ")).join(", "),
    );
  });
}
