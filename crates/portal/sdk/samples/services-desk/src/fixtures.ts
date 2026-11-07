import type { FileInfo, JcUser, JobStatus, Row } from "@joinedcontext/sdk";
import type { Fixture } from "@joinedcontext/sdk/testing";

/** A steward: the one role the close-report function lets through. */
export const USER: JcUser = { id: "8c1f2a90-5d3e-4b7a-9e21-6f0d4c3b2a18", name: "Eero Laine", roles: ["steward"] };

const report = (local: string, name: string, district: string, status: string, day: number): Row => ({
  id: `urn:ngsi-ld:DefectReport:hel.fi:defects:${local}`,
  type: "DefectReport",
  name,
  description: `${name}, reported by a resident through the city's form.`,
  district,
  status,
  dateReported: `2026-09-${String(day).padStart(2, "0")}T09:10:00Z`,
});

/** The reports as the endpoint answers them, newest first. */
export const ROWS: Row[] = [
  report("d1", "Loose kerb stone at Hakaniemi market", "Kallio", "open", 24),
  report("d2", "Broken bench in Sibelius Park", "Töölö", "open", 23),
  report("d3", "Faded zebra crossing on Mannerheimintie", "Kamppi", "open", 21),
  report("d4", "Leaning lamp post on Teollisuuskatu", "Vallila", "closed", 18),
];

/** The photos already in the App's files, under each report's own prefix. */
export const PHOTOS: FileInfo[] = [
  { path: "reports/d1/kerb-north.jpg", size: 182_344, contentType: "image/jpeg", modifiedAt: "2026-09-24T09:12:00Z" },
  { path: "reports/d1/kerb-close.jpg", size: 96_120, contentType: "image/jpeg", modifiedAt: "2026-09-24T09:13:00Z" },
];

export const JOBS: JobStatus[] = [
  { name: "nightly-digest", schedule: "0 6 * * *", nextRun: "2026-09-25T06:00:00Z", lastRun: { at: "2026-09-24T06:00:00Z", ok: true } },
  { name: "photo-sweep", schedule: "30 2 * * 0", nextRun: "2026-09-27T02:30:00Z", lastRun: { at: "2026-09-20T02:30:00Z", ok: false, message: "files quota reached" } },
];

/** The platform services as the Portal answers them when every layer leaves them on. */
export const SERVICES: NonNullable<Fixture["services"]> = {
  files: (request) => {
    if (request.method === "PUT") {
      const path = decodeURIComponent(request.path.replace(/^\/api\/services\/files\//, ""));
      return { status: 201, body: { path, size: 1, contentType: "image/jpeg", modifiedAt: "2026-09-24T10:00:00Z" } };
    }
    const prefix = new URLSearchParams(request.path.split("?")[1] ?? "").get("prefix") ?? "";
    return { status: 200, body: PHOTOS.filter((photo) => photo.path.startsWith(prefix)) };
  },
  email: () => ({ status: 200, body: { id: "msg-1" } }),
  jobs: () => ({ status: 200, body: JOBS }),
  ai: () => ({ status: 200, body: { text: '{"severity":4}', json: { severity: 4 }, tokens: { in: 64, out: 8 } } }),
};

/** The function as the demo answers it; `functions/close-report.ts` is the real one. */
export const FUNCTIONS = {
  "close-report": (body: unknown) => ({ id: (body as { id: string }).id, status: "closed" }),
};
