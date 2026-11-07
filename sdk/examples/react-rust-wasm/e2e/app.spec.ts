import { expect, test } from "@playwright/test";
import { BASE, STATIC_HOST_CSP, serve } from "./serve";

// T-3327, AP-142: the Rust module answers in a Web Worker under the static host's own policy.
test("the WebAssembly worker summarizes what the SDK fetched", async ({ page }) => {
  const problems = await serve(page);
  await page.goto(BASE);
  const summary = page.getByTestId("summary");
  await expect(summary).toContainText("Entities with a value4");
  await expect(summary).toContainText("min1");
  await expect(summary).toContainText("max10");
  await expect(summary).toContainText("mean5.5");
  expect(problems).toEqual([]);
});

// The policy is what lets the module compile: without 'wasm-unsafe-eval' the browser refuses it.
test("without 'wasm-unsafe-eval' the browser refuses to compile the module", async ({ page }) => {
  await serve(page, STATIC_HOST_CSP.replace(" 'wasm-unsafe-eval'", ""));
  await page.goto(BASE);
  await expect(page.getByRole("alert")).toContainText("WebAssembly");
  await expect(page.getByTestId("summary")).toHaveCount(0);
});
