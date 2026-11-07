import { describe, expect, it } from "vitest";
import {
  allowedUrl,
  nextAction,
  parseAction,
  redact,
} from "../e2e/agent/actions";

// T-5981: the user agent's action parser. A model answer reaches Playwright only as one of these.
describe("parseAction", () => {
  it("reads each action kind", () => {
    expect(
      parseAction('{"kind":"click","role":"button","name":"Search"}'),
    ).toEqual({ kind: "click", role: "button", name: "Search" });
    expect(
      parseAction(
        '```json\n{"kind":"fill","role":"searchbox","name":"Search","value":"ovzdušie"}\n```',
      ),
    ).toEqual({
      kind: "fill",
      role: "searchbox",
      name: "Search",
      value: "ovzdušie",
    });
    expect(
      parseAction(
        'The search box is first.\n{"kind":"click","role":"button","name":"Search"}',
      ),
    ).toEqual({ kind: "click", role: "button", name: "Search" });
    expect(parseAction('{"kind":"back"}')).toEqual({ kind: "back" });
    expect(parseAction('{"kind":"press","key":"Enter"}')).toEqual({
      kind: "press",
      key: "Enter",
    });
    expect(parseAction('{"kind":"navigate","url":"/catalogue"')).toEqual({
      kind: "navigate",
      url: "/catalogue",
    });
    expect(parseAction('{"kind":"done","answer":"PM10 is 21"}')).toEqual({
      kind: "done",
      answer: "PM10 is 21",
    });
  });

  it("refuses prose, unknown kinds, roles and keys", () => {
    expect(() => parseAction("I will click Search")).toThrow(
      "not one JSON object",
    );
    expect(() => parseAction('[{"kind":"done"}]')).toThrow(
      "not one JSON object",
    );
    expect(() => parseAction('{"kind":"eval","js":"x"}')).toThrow(
      '"kind" must be',
    );
    expect(() =>
      parseAction('{"kind":"click","role":"div","name":"x"}'),
    ).toThrow('role "div"');
    expect(() => parseAction('{"kind":"press","key":"Control+w"}')).toThrow(
      'key "Control+w"',
    );
    expect(() =>
      parseAction('{"kind":"click","role":"button","name":""}'),
    ).toThrow('"name"');
  });
});

describe("nextAction", () => {
  it("retries a bad answer once with the parser's message", async () => {
    const notes: (string | undefined)[] = [];
    const answers = ["nonsense", '{"kind":"press","key":"Tab"}'];
    const got = await nextAction(
      async (note) => (notes.push(note), answers.shift() ?? ""),
    );
    expect(got).toEqual({
      action: { kind: "press", key: "Tab" },
      retried: true,
    });
    expect(notes[0]).toBeUndefined();
    expect(notes[1]).toContain("not one JSON object");
  });

  it("ends as stuck after the second bad answer, asking no third time", async () => {
    let calls = 0;
    const got = await nextAction(async () => ((calls += 1), "still nonsense"));
    expect(calls).toBe(2);
    expect(got.action.kind).toBe("stuck");
    expect(got.retried).toBe(true);
  });
});

describe("allowedUrl and redact", () => {
  const start = "https://portal.dev.joinedcontext.com/";
  it("keeps paths on the start origin and refuses other hosts and schemes", () => {
    expect(allowedUrl("/projects", start, [])).toBe(
      "https://portal.dev.joinedcontext.com/projects",
    );
    expect(
      allowedUrl("https://opendata.dev.joinedcontext.com/dataset", start, [
        "opendata.dev.joinedcontext.com",
      ]),
    ).toContain("opendata.");
    expect(() =>
      allowedUrl("https://evil.example/", start, [
        "opendata.dev.joinedcontext.com",
      ]),
    ).toThrow("outside");
    expect(() => allowedUrl("javascript:alert(1)", start, [])).toThrow(
      "outside",
    );
  });
  it("masks credential-shaped text", () => {
    const jwt = ["eyJhbGciOiJSUzI1NiJ9", "eyJzdWIiOiJ4eHh4eHgifQ", "sig"].join(
      ".",
    );
    expect(redact(`token ${jwt} here`)).toEqual({
      text: "token [secret hidden] here",
      hidden: 1,
    });
  });
});
