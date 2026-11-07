// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/endpoints/snippets.ts.
/**
 * T-3264: an Endpoint's call as curl, Python or JavaScript; a closed endpoint's code names a token
 * and never carries one.
 */
import { describe, expect, it } from "vitest";
import { snippetsOf } from "../src/pages/endpoints/snippets";

describe("the call as code (T-3264)", () => {
  it("writes the call as code, naming a token only where the endpoint is closed", () => {
    const url = "https://dev.example/api/endpoint/x/ngsi-ld/v1/types";
    const closed = snippetsOf(url, false);
    expect(closed.curl).toContain('-H "Authorization: Bearer $TOKEN"');
    expect(closed.python).toContain('os.environ["TOKEN"]');
    expect(closed.javascript).toContain("process.env.TOKEN");
    const open = snippetsOf(url, true);
    for (const code of Object.values(open)) {
      expect(code).toContain(url);
      expect(code).not.toMatch(/TOKEN|Authorization/);
    }
    expect(snippetsOf("https://x/'; rm -rf /", true).python).toContain(JSON.stringify("https://x/'; rm -rf /"));
  });
});
