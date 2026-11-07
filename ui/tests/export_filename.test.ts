/** The export's file name from its Content-Disposition (T-6230): a malformed escape keeps the export. */
import { describe, expect, it } from "vitest";
import { filenameOf } from "../src/components/export/ExportModal";

const answer = (disposition: string) => new Response(null, { headers: { "content-disposition": disposition } });

describe("the exported file's name", () => {
  it("decodes the server's name and keeps a malformed one as it came", () => {
    expect(filenameOf(answer(`attachment; filename*=UTF-8''pr%C3%A1ca.zip`), "p", "zip", {})).toBe("práca.zip");
    expect(filenameOf(answer(`attachment; filename*=UTF-8''%E0%A4%A`), "p", "zip", {})).toBe("%E0%A4%A");
    expect(filenameOf(answer(""), "helsinki", "zip", {})).toBe("helsinki.zip");
  });
});
