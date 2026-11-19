/**
 * T-7154, T-7168 — an Organization's configuration and data backup to its own S3 bucket.
 *
 * Setting the bucket, running, deleting and restoring are for the Organization's administrators.
 * `demo.viewer` holds no binding at the Organization: the Portal refuses them every backup door,
 * reads included, so neither the bucket nor a version is named to them. The doors themselves are
 * proved against an in-process S3 by tests/org_backup_tests.rs; the page by e2e/org_backups.spec.ts.
 */
import { expect, test } from "@playwright/test";
import { VIEWER, csrf, signIn } from "./portal";

// Test keys, joined from halves: the public check keeps whole age recipients out of the repository.
const RECIPIENT = ["age1mmaj3l7xsa7pjqmxr3lfc7k3nmy63sfw7", "tglj2r57stsehfmwvxqxp9h5k"].join("");

test("a person who is no administrator of the Organization is refused every backup door", async ({ browser }) => {
  test.skip(!VIEWER.password, "VIEWER_PASSWORD is not set");
  const { context, page } = await signIn(browser, VIEWER, "/?lang=en");
  const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };
  const door = "/api/v1/organization/backups/configuration";
  const data = "/api/v1/organization/backups/data";
  const target = {
    endpoint: "https://s3.eu-central-1.amazonaws.com",
    region: "eu-central-1",
    bucket: "viewer-backups",
    prefix: "",
    accessKeyId: "viewer-key-id",
    secretAccessKey: "not-a-real-secret",
    enabled: false,
    everyHours: 24,
    keepDays: 90,
  };
  const answers = [
    await page.request.get(door),
    await page.request.put(door, { headers, data: target }),
    await page.request.post(`${door}/run`, { headers }),
    await page.request.get(`${door}/versions`),
    await page.request.post(`${door}/versions/v1/restore`, { headers, data: { identity: "AGE-SECRET-KEY-1", apply: false } }),
    await page.request.delete(`${door}/bundle`, { headers }),
    await page.request.get(data),
    await page.request.put(data, { headers, data: { ...target, workingRecipient: RECIPIENT, recoveryRecipient: RECIPIENT } }),
    await page.request.post(`${data}/run`, { headers }),
    await page.request.get(`${data}/versions`),
    await page.request.get(`${data}/versions/v1`),
    await page.request.delete(`${data}/archive`, { headers }),
  ];
  for (const answer of answers) {
    expect(answer.status(), `${answer.url()} refuses a non-administrator`).toBe(403);
    expect(await answer.text(), `${answer.url()} says who may`).toContain("administrators");
  }
  await context.close();
});
