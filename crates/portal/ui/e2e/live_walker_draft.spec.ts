import { expect, test } from "@playwright/test";
import type { Response } from "@playwright/test";
import { missingDraftRead } from "../tests/pageChecks";

// T-7119: the edit pages ask jc_draft_get for the person's draft; none yet is a 404 the walker
// excuses. Here the browser sends the OData address as it encodes it, and the walker's rule reads
// the Response the walker reads; a 404 of the draft list stays a finding.
test("the walker excuses the browser's missing draft read and nothing beside it", async ({ page }) => {
  await page.route("http://portal.test/**", (route) =>
    route.request().resourceType() === "document"
      ? route.fulfill({ body: "<main><h1>Edit endpoint</h1></main>", contentType: "text/html" })
      : route.fulfill({ status: 404, json: { status: 404, detail: "no draft" } }),
  );
  await page.goto("http://portal.test/projects/helsinki/endpoints/app-air-quality/edit");
  const seen: Response[] = [];
  page.on("response", (response) => seen.push(response));
  await page.evaluate(async () => {
    await fetch("/api/v1/projects('helsinki')/JoinedContext.Portal.jc_draft_get(kind=@kind,name=@name)?@kind='Endpoint'&@name='app-air-quality'");
    await fetch("/api/v1/projects('helsinki')/JoinedContext.Portal.jc_draft_list()");
  });
  await expect.poll(() => seen.length).toBe(2);
  const excused = seen.map((response) => [new URL(response.url()).pathname.split("/").pop(), missingDraftRead(response.status(), response.request().method(), response.url())]);
  expect(excused).toEqual([
    ["JoinedContext.Portal.jc_draft_get(kind=@kind,name=@name)", true],
    ["JoinedContext.Portal.jc_draft_list()", false],
  ]);
});
