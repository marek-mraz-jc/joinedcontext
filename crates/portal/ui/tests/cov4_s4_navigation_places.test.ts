// T-7706: the places store when the preferences door fails: a star whose re-read fails, a refused save of preferences, a report that is refused.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { doorsAsRest } from "./odata";
import { remember, reportRecent, resetPlaces, savePreferences, toggleFavourite } from "../src/navigation/places";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const PLACE = { path: "/projects/helsinki/pipelines", title: "Pipelines" };

interface Stub {
  reads: number;
  puts: unknown[];
  reports: number;
}

function stub(options: { readsThatWork: number; putStatus?: number; reportStatus?: number }): Stub {
  const seen: Stub = { reads: 0, puts: [], reports: 0 };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      if (url.pathname === "/api/v1/preferences/recent") {
        seen.reports += 1;
        return options.reportStatus ? json([{ path: "", message: "refused" }], options.reportStatus) : json({});
      }
      if (url.pathname === "/api/v1/preferences") {
        if (request.method === "PUT") {
          if (options.putStatus) return json([{ path: "", message: "refused" }], options.putStatus);
          const body = (await request.json()) as object;
          seen.puts.push(body);
          return json(body);
        }
        seen.reads += 1;
        return seen.reads <= options.readsThatWork ? json({ recent: [], favourites: [] }) : json([{ path: "", message: "down" }], 500);
      }
      return json({});
    }),
  );
  doorsAsRest();
  return seen;
}

beforeEach(() => resetPlaces());
afterEach(() => vi.unstubAllGlobals());

describe("the places store when the server does not answer", () => {
  it("keeps a star for this visit and writes nothing when the stored preferences cannot be read again", async () => {
    const seen = stub({ readsThatWork: 1 });
    await toggleFavourite(PLACE);
    expect(seen.reads).toBe(2);
    expect(seen.puts).toEqual([]);
  });

  it("keeps a star for this visit when the save is refused", async () => {
    const seen = stub({ readsThatWork: 9, putStatus: 403 });
    await expect(toggleFavourite(PLACE)).resolves.toBeUndefined();
    expect(seen.puts).toEqual([]);
  });

  it("saves a preference patch, and a refused one rejects for the caller while the queue goes on", async () => {
    const seen = stub({ readsThatWork: 9 });
    await expect(savePreferences({ recent: [PLACE] })).resolves.toMatchObject({ recent: [PLACE] });
    expect(seen.puts).toHaveLength(1);
    const refused = stub({ readsThatWork: 9, putStatus: 403 });
    await expect(savePreferences({ recent: [] })).rejects.toBeInstanceOf(Error);
    expect(refused.puts).toEqual([]);
    // The queue is not stuck behind the refusal.
    const again = stub({ readsThatWork: 9 });
    await savePreferences({ recent: [PLACE] });
    expect(again.puts).toHaveLength(1);
  });

  it("drops the report of opened pages when the server refuses it, without throwing", async () => {
    const seen = stub({ readsThatWork: 9, reportStatus: 500 });
    await remember(PLACE);
    await expect(reportRecent()).resolves.toBeUndefined();
    expect(seen.reports).toBe(1);
    // What was reported is not reported twice.
    await reportRecent();
    expect(seen.reports).toBe(1);
  });
});

describe("the report of opened pages", () => {
  it("is sent once and settles quietly when the server takes it", async () => {
    const seen = stub({ readsThatWork: 9 });
    await remember(PLACE);
    await expect(reportRecent()).resolves.toBeUndefined();
    expect(seen.reports).toBe(1);
  });
});
