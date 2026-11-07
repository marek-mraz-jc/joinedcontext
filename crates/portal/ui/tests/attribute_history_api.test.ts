import { afterEach, describe, expect, it, vi } from "vitest";
import { instantOf, MAX_CSV_BYTES, MAX_CSV_ROWS, measurementsOf, parseCsv, planImport, readHistory, runImport, valueOf, writeMeasurement } from "../src/components/entities/history/historyApi";
import type { CsvRow, Measurement } from "../src/components/entities/history/historyApi";

const NOW = "2026-10-18T09:00:00.000Z";
const row = (observedAt: string, value: unknown, line = 2): CsvRow => ({ observedAt, value, line });
const held = (observedAt: string, value: unknown): Measurement => {
  const current = { value, observedAt };
  return { observedAt, current, versions: [current] };
};

function capture(status = 204): Request[] {
  const seen: Request[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    seen.push(input as Request);
    return new Response(status === 204 ? null : JSON.stringify({ detail: "no grant" }), { status });
  });
  return seen;
}

afterEach(() => vi.restoreAllMocks());

describe("measurementsOf", () => {
  it("groups the versions of an instant, oldest first, the live one current, newest measurement first", () => {
    const [newest, older] = measurementsOf(
      {
        level: [
          { value: 1, observedAt: "2026-01-01T00:00:00Z" },
          { value: 3, observedAt: "2026-01-02T00:00:00Z", provenance: { writtenAt: "2026-01-03T00:00:00Z" } },
          { value: 2, observedAt: "2026-01-02T00:00:00Z", supersededAt: "2026-01-03T00:00:00Z", provenance: { writtenAt: "2026-01-02T00:00:01Z" } },
        ],
      },
      "level",
    );
    expect(newest.versions.map((v) => v.value)).toEqual([2, 3]);
    expect(newest.current.value).toBe(3);
    expect(older.current.value).toBe(1);
  });

  it("reads one instance and the values form, and leaves out a named dataset and a row with no time", () => {
    expect(measurementsOf({ level: { value: 1, observedAt: NOW } }, "level")).toHaveLength(1);
    expect(measurementsOf({ level: { values: [{ value: 1, observedAt: NOW }] } }, "level")).toHaveLength(1);
    expect(measurementsOf({ level: [{ value: 1, observedAt: NOW, datasetId: "urn:a" }, { value: 2 }] }, "level")).toEqual([]);
    expect(measurementsOf(null, "level")).toEqual([]);
  });
});

describe("readHistory", () => {
  it("falls back to the plain history on a 403 for versions, and a 404 is nothing recorded", async () => {
    const urls: string[] = [];
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const { url } = input as Request;
      urls.push(url);
      return new Response(JSON.stringify({ level: [{ value: 1, observedAt: NOW }] }), { status: url.includes("versions=all") ? 403 : 200 });
    });
    const read = await readHistory("slug", "urn:ngsi-ld:S:1", "level");
    expect(read.versions).toBe(false);
    expect(read.measurements).toHaveLength(1);
    expect(urls).toHaveLength(2);

    vi.restoreAllMocks();
    capture(404);
    expect((await readHistory("slug", "urn:ngsi-ld:S:1", "level")).measurements).toEqual([]);
  });

  it("names a refused read by the gateway's words", async () => {
    capture(500);
    await expect(readHistory("slug", "urn:ngsi-ld:S:1", "level")).rejects.toThrow("no grant");
  });
});

describe("writeMeasurement", () => {
  it("is one PATCH of the attribute through the Endpoint, never a temporal route a view refuses", async () => {
    const seen = capture();
    await writeMeasurement("slug", "urn:ngsi-ld:S:1", "level", { value: 9, observedAt: NOW, unitCode: "CEL" }, "  sensor checked  ");
    const [request] = seen;
    expect(request.method).toBe("PATCH");
    expect(new URL(request.url).pathname).toBe(`/api/endpoint/slug/ngsi-ld/v1/entities/${encodeURIComponent("urn:ngsi-ld:S:1")}/attrs`);
    expect(request.headers.get("JC-Reason")).toBe("sensor%20checked");
    expect(request.headers.get("JC-Record")).toBe("version");
    expect(await request.json()).toEqual({ level: { type: "Property", value: 9, observedAt: NOW, unitCode: "CEL" } });
  });

  it("refuses a blank or an over-long reason before anything leaves", async () => {
    const seen = capture();
    await expect(writeMeasurement("slug", "urn:ngsi-ld:S:1", "level", { value: 1, observedAt: NOW }, "  ")).rejects.toThrow();
    await expect(writeMeasurement("slug", "urn:ngsi-ld:S:1", "level", { value: 1, observedAt: NOW }, "x".repeat(501))).rejects.toThrow();
    expect(seen).toEqual([]);
  });

  it("says what the gateway refused", async () => {
    capture(403);
    await expect(writeMeasurement("slug", "urn:ngsi-ld:S:1", "level", { value: 1, observedAt: NOW }, "why")).rejects.toThrow("no grant");
  });
});

describe("instantOf and valueOf", () => {
  it("takes a past local time as UTC and refuses a future or broken one", () => {
    expect(instantOf("2026-03-03T14:00")).toBe(new Date(2026, 2, 3, 14, 0).toISOString());
    expect(instantOf("9999-01-01T00:00")).toBeUndefined();
    expect(instantOf("yesterday")).toBeUndefined();
    expect(instantOf("")).toBeUndefined();
  });

  it("reads typed text as the type of the newest value", () => {
    expect(valueOf("21.5", 1)).toBe(21.5);
    expect(valueOf("many", 1)).toBeUndefined();
    expect(valueOf("", 1)).toBeUndefined();
    expect(valueOf("TRUE", false)).toBe(true);
    expect(valueOf("yes", false)).toBeUndefined();
    expect(valueOf('{"a":1}', {})).toEqual({ a: 1 });
    expect(valueOf("{", {})).toBeUndefined();
    expect(valueOf(" free ", "occupied")).toBe(" free ");
    expect(valueOf("true", undefined)).toBe(true);
    expect(valueOf("free", undefined)).toBe("free");
    expect(valueOf("", undefined)).toBeUndefined();
  });
});

describe("parseCsv", () => {
  it("reads the two columns in any order, typed, and names the line of a bad row", () => {
    const { rows, problems } = parseCsv("value,observedAt\n21.5,2026-03-03T14:00:00Z\nmany,2026-03-03T15:00:00Z\n1,not a time\n1,9999-01-01T00:00:00Z\n", 1);
    expect(rows).toEqual([{ observedAt: "2026-03-03T14:00:00.000Z", value: 21.5, line: 2 }]);
    expect(problems).toEqual([
      { line: 3, reason: "badValue" },
      { line: 4, reason: "badInstant" },
      { line: 5, reason: "futureInstant" },
    ]);
  });

  it("refuses an empty file, missing columns, and a file over its caps, sending nothing", () => {
    expect(parseCsv("").problems).toEqual([{ line: 1, reason: "emptyFile" }]);
    expect(parseCsv("when,what\n1,2\n").problems).toEqual([{ line: 1, reason: "columns" }]);
    expect(parseCsv("x".repeat(MAX_CSV_BYTES + 1)).problems).toEqual([{ line: 1, reason: "tooLarge" }]);
    const long = `observedAt,value\n${"2026-03-03T14:00:00Z,1\n".repeat(MAX_CSV_ROWS + 1)}`;
    expect(parseCsv(long)).toEqual({ rows: [], problems: [{ line: 1, reason: "tooManyRows" }] });
  });

  it("drops a byte-order mark", () => {
    expect(parseCsv("\uFEFFobservedAt,value\n2026-03-03T14:00:00Z,1\n", 1).rows).toHaveLength(1);
  });
});

describe("planImport", () => {
  const history = [held("2026-03-02T00:00:00Z", 2), held("2026-03-03T00:00:00Z", 3)];

  it("sorts rows into new, duplicate, correction and unchecked", () => {
    const plan = planImport(
      [row("2026-03-02T12:00:00.000Z", 9), row("2026-03-03T00:00:00.000Z", 3), row("2026-03-02T00:00:00.000Z", 7), row("2026-03-01T00:00:00.000Z", 1)],
      history,
    );
    expect(plan.added.map((r) => r.value)).toEqual([9]);
    expect(plan.duplicates.map((r) => r.value)).toEqual([3]);
    expect(plan.corrections.map((r) => r.value)).toEqual([7]);
    expect(plan.unchecked.map((r) => r.value)).toEqual([1]);
  });

  it("treats a row before the oldest as new when the whole history was read, and an empty history likewise", () => {
    expect(planImport([row("2026-03-01T00:00:00.000Z", 1)], history, true).added).toHaveLength(1);
    expect(planImport([row("2026-03-01T00:00:00.000Z", 1)], [], true).added).toHaveLength(1);
  });

  it("keeps the last row of an instant", () => {
    expect(planImport([row(NOW, 1, 2), row(NOW, 2, 3)], [], true).added).toEqual([row(NOW, 2, 3)]);
  });
});

describe("runImport", () => {
  const target = { slug: "slug", entityId: "urn:ngsi-ld:S:1", attribute: "level" };
  const plan = { added: [row("2026-03-02T00:00:00.000Z", 2, 3)], duplicates: [row(NOW, 9, 9)], corrections: [row("2026-03-03T00:00:00.000Z", 3, 4)], unchecked: [row("2026-03-01T00:00:00.000Z", 1, 2)] };

  it("sends every row but the duplicates oldest first and reports progress", async () => {
    const seen = capture();
    const progress: number[] = [];
    expect(await runImport(target, plan, "from the paper log", (each) => progress.push(each.sent))).toEqual({ sent: 3 });
    const bodies = await Promise.all(seen.map((request) => request.json() as Promise<{ level: { value: number } }>));
    expect(bodies.map((body) => body.level.value)).toEqual([1, 2, 3]);
    expect(progress).toEqual([1, 2, 3]);
  });

  it("stops at the first refusal and names its line", async () => {
    capture(403);
    expect(await runImport(target, plan, "why")).toEqual({ sent: 0, failed: { line: 2, message: "no grant" } });
  });
});
