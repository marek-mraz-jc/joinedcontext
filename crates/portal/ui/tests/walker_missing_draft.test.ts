import { describe, expect, it } from "vitest";
import { missingDraftRead } from "./pageChecks";

// T-7119: the edit pages read the person's draft through jc_draft_get since b0c154f6; the walker
// excused the "no draft yet" 404 only at the REST address and flagged four edit pages.
describe("the walker's missing draft rule", () => {
  const odata = "http://x/api/v1/projects('helsinki')/JoinedContext.Portal.jc_draft_get(kind=@kind,name=@name)?@kind='Endpoint'&@name='app-air-quality'";

  it("excuses a GET 404 of the draft read at both addresses", () => {
    expect(missingDraftRead(404, "GET", odata)).toBe(true);
    expect(missingDraftRead(404, "GET", "http://x/api/v1/projects/helsinki/drafts/Endpoint/app-air-quality")).toBe(true);
  });

  it("keeps every other answer a finding", () => {
    expect(missingDraftRead(403, "GET", odata)).toBe(false);
    expect(missingDraftRead(500, "GET", odata)).toBe(false);
    expect(missingDraftRead(404, "POST", odata)).toBe(false);
    expect(missingDraftRead(404, "GET", "http://x/api/v1/projects('helsinki')/JoinedContext.Portal.jc_draft_list()")).toBe(false);
    expect(missingDraftRead(404, "GET", "http://x/api/v1/projects/helsinki/spaces/a")).toBe(false);
  });
});
