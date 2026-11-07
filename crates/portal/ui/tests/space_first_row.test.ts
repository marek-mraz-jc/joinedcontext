import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { preloadSpacePage } from "../src/pages/spaces/preload";
import { parseModel } from "../src/pages/models/linkml";

/** T-5598: on Slow 4G every read the grid waits for in turn is a 0.6 s round trip before its first row. */
describe("a space's page opened from its address", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function stub() {
    const asked: string[] = [];
    // No read answers: what was asked before any answer is what ran side by side.
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      asked.push(new URL(input instanceof Request ? input.url : String(input), window.location.origin).pathname);
      return new Promise<Response>(() => undefined);
    });
    vi.stubGlobal("fetch", fetchMock);
    return asked;
  }

  it("starts the space and the project's models together, before the session check answers", async () => {
    const asked = stub();
    preloadSpacePage(new QueryClient(), "/projects/helsinki/spaces/helsinki");
    await vi.waitFor(() => expect(asked).toHaveLength(2));
    expect(asked.sort()).toEqual(["/api/v1/projects/helsinki/datamodels", "/api/v1/projects/helsinki/spaces/helsinki"]);
  });

  it("reads nothing ahead for an address that is no space's page", async () => {
    const asked = stub();
    const client = new QueryClient();
    for (const path of ["/projects/helsinki/spaces", "/projects/helsinki/spaces/new", "/projects/helsinki/spaces/upload", "/projects/helsinki/spaces/complete", "/projects/helsinki/spaces/a%2Fb", "/projects/helsinki/spaces/Air/edit", "/projects/helsinki/endpoints/air"]) {
      preloadSpacePage(client, path);
    }
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(asked).toEqual([]);
  });
});

describe("a model's source parsed for the grid", () => {
  const source = "id: https://example.org/air\nname: air\nclasses:\n  AirQualityObserved:\n    slots: [pm10]\nslots:\n  pm10:\n    range: float\n";

  it("is parsed once for every reader of the same text", () => {
    const first = parseModel(source);
    expect(first.classes.map((c) => c.name)).toEqual(["AirQualityObserved"]);
    expect(parseModel(source)).toBe(first);
  });

  it("is parsed again when the text changes", () => {
    const before = parseModel(source);
    const changed = parseModel(source.replace("AirQualityObserved", "AirQualityStation"));
    expect(changed).not.toBe(before);
    expect(changed.classes.map((c) => c.name)).toEqual(["AirQualityStation"]);
  });
});
