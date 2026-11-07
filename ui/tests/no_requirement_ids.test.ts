/**
 * T-2756, UI-16: a person reads no requirement id. The locales carry none, and what the server
 * says (a refusal, a finding, an assistant's line) loses the "(AP-44)" it cites for engineers
 * before it reaches the screen; the server code keeps them, which the compliance matrix reads.
 */
import { describe, expect, it } from "vitest";
import { ApiError, forPeople, problemLines } from "../src/api/client";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";

function strings(node: unknown, at = ""): [string, string][] {
  if (typeof node === "string") return [[at, node]];
  if (node === null || typeof node !== "object") return [];
  return Object.entries(node).flatMap(([key, value]) => strings(value, at ? `${at}.${key}` : key));
}

describe("requirement ids on the screen (T-2756)", () => {
  it.each([
    ["en", en],
    ["sk", sk],
    ["cs", cs],
    ["de", de],
  ])("the %s locale names none", (_, bundle) => {
    const cited = strings(bundle).filter(([, text]) => /\([A-Z]{2,3}-\d+/.test(text));
    expect(cited).toEqual([]);
  });

  it("drops a parenthesis of references and keeps the sentence", () => {
    expect(forPeople("dataNeeds must not be empty (AP-44)")).toBe("dataNeeds must not be empty");
    expect(forPeople("refused (AG-69, AP-44); try another endpoint")).toBe("refused; try another endpoint");
    expect(forPeople("the lane's token (ADR-N-028 §5) expired")).toBe("the lane's token expired");
    expect(forPeople("moved past the Portal's copy (T-2674)")).toBe("moved past the Portal's copy");
    expect(forPeople("")).toBe("");
  });

  it("keeps a parenthesis that says more than a reference, and ones that are not references", () => {
    for (const kept of ["as the check says (see AP-44)", "a text file (UTF-8)", "a date (ISO-8601)", "one step (a)"]) {
      expect(forPeople(kept)).toBe(kept);
    }
  });

  it("cleans a refusal wherever a page reads it: the message, the detail and each error", () => {
    const error = new ApiError(403, "the agent profile does not grant read (AG-70)", {
      status: 403,
      title: "Forbidden (PF-49)",
      type: "about:blank",
      detail: "the agent profile does not grant read (AG-70)",
      errors: ["name is taken (MF-11)", "no such space"],
    });
    expect(error.message).toBe("the agent profile does not grant read");
    expect(error.problem?.detail).toBe("the agent profile does not grant read");
    expect(error.problem?.title).toBe("Forbidden");
    expect(error.problem?.errors).toEqual(["name is taken", "no such space"]);
    expect(new ApiError(500, "HTTP 500").problem).toBeUndefined();
  });
});

describe("field errors with their paths (T-5949, AG-146)", () => {
  it("reads errors[{path, message}] as the lines a form marks its fields with", () => {
    const detail = "the request names 3 problems";
    const error = new ApiError(400, detail, {
      status: 400,
      title: "Invalid Request",
      type: "https://joinedcontext.com/errors/invalid-request",
      detail,
      errors: [
        { path: "spec.allowedProjects", message: detail },
        { path: "types[0]", message: "names no type of the model (EP-12)" },
        { path: "", message: "the manifest is empty" },
      ],
    });
    expect(error.problem?.errors).toEqual(["spec.allowedProjects", "types[0]: names no type of the model", "the manifest is empty"]);
  });

  it("keeps a plain line and skips what is neither a line nor a member", () => {
    expect(problemLines({ errors: ["subject", 7, null, { path: 3 }] })).toEqual(["subject"]);
    expect(problemLines({ errors: "spec" })).toEqual([]);
    expect(problemLines(undefined)).toEqual([]);
  });
});

describe("a refusal the server wrote without every field (T-2756)", () => {
  it("keeps the problem as it came, with no field added and nothing thrown", () => {
    const partial = { status: 502, detail: "the forge is down" } as unknown as ConstructorParameters<typeof ApiError>[2];
    const error = new ApiError(502, "the forge is down", partial);
    expect(error.problem).toEqual({ status: 502, detail: "the forge is down" });
  });
});
