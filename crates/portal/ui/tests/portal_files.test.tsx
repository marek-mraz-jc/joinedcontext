// The Files tab of a portal (T-7417, T-7428): the shared grid over the files store, upload by
// drag and drop, rename, replace, delete with a confirm and refused while used, the limits; each
// state as a person meets it.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { sizeText, typeText } from "../src/pages/portals/PortalFiles";
import { expectAxeClean, jsonResponse, problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk" }] };
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const PORTAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Portal",
  metadata: { name: "bbsk-mobility", namespace: "org" },
  spec: { organizationRef: "bbsk", path: "mobility", title: { en: "Mobility" } },
};
const SHA = "a".repeat(64);
const HERO = {
  id: "f-1",
  name: "hero.png",
  contentType: "image/png",
  size: 2048,
  sha256: SHA,
  path: `/files/${SHA}/hero.png`,
  uploadedBy: "Ana",
  uploadedAt: "2026-11-19T19:00:00Z",
  usedBy: ["bbsk-home"],
};
const REPORT = { ...HERO, id: "f-2", name: "report.pdf", contentType: "application/pdf", size: 3 * 1024 * 1024, sha256: "b".repeat(64), usedBy: [] };
const LIMITS = { files: 2, bytes: 3 * 1024 * 1024 + 2048, maxFiles: 500, maxBytes: 200 * 1024 * 1024, maxFileBytes: 10 * 1024 * 1024 };
const FILES = "/api/v1/projects/org/portals/bbsk-mobility/files";

type Answer = (path: string, request: Request) => Response | Promise<Response> | undefined;

async function openFiles(answer: Answer) {
  const rendered = await renderRoute({
    path: "/portals/bbsk-mobility",
    permissions: ADMIN,
    answer: (path, request) => {
      if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
      if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "GET") return jsonResponse(PORTAL);
      return answer(path, request);
    },
  });
  fireEvent.click(await screen.findByRole("tab", { name: "Files" }));
  return rendered;
}

describe("file helpers", () => {
  it("names sizes and types as a person reads them", () => {
    expect([sizeText(12), sizeText(2048), sizeText(3 * 1024 * 1024)]).toEqual(["12 B", "2 kB", "3.0 MB"]);
    expect(typeText("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")).toBe("XLSX");
    expect(typeText("image/svg+xml")).toBe("SVG");
  });
});

describe("the files tab", () => {
  it("says what to do when the portal has no file yet", async () => {
    const { container } = await openFiles((path) => (path === FILES ? jsonResponse({ items: [], limits: { ...LIMITS, files: 0, bytes: 0 } }) : undefined));
    expect(await screen.findByText("No file yet")).toBeInTheDocument();
    expect(screen.getByText(/PNG, JPEG, WebP, SVG, PDF, CSV or XLSX, up to 10.0 MB each, for example hero.png/)).toBeInTheDocument();
    expect(screen.getByText("0 of 500 files, 0 B of 200.0 MB used by the Organization.")).toBeInTheDocument();
    await expectAxeClean(container);
  });

  it("lists each file with its type, size and where it is used, and refuses to delete one in use", async () => {
    const { container } = await openFiles((path) => (path === FILES ? jsonResponse({ items: [HERO, REPORT], limits: LIMITS }) : undefined));
    const grid = await screen.findByRole("table", { name: "Files" });
    expect(within(grid).getByText("hero.png")).toBeInTheDocument();
    expect(within(grid).getByText("PDF")).toBeInTheDocument();
    expect(within(grid).getByText("3.0 MB")).toBeInTheDocument();
    expect(within(grid).getByText("bbsk-home")).toBeInTheDocument();
    expect(within(grid).getByText("Not used")).toBeInTheDocument();
    const del = screen.getByRole("button", { name: "Delete hero.png" });
    expect(del).toBeDisabled();
    expect(del).toHaveAttribute("title", "Used by bbsk-home: take it out there first.");
    await expectAxeClean(container);
  });

  it("uploads dropped files one by one with their progress and lists them", async () => {
    const sent: { url: string; type: string | null; size: number }[] = [];
    let listed = [] as (typeof HERO)[];
    await openFiles(async (path, request) => {
      if (path === FILES && request.method === "POST") {
        const body = await request.arrayBuffer();
        sent.push({ url: request.url, type: request.headers.get("content-type"), size: body.byteLength });
        listed = [HERO];
        return jsonResponse(HERO, 201);
      }
      if (path === FILES) return jsonResponse({ items: listed, limits: LIMITS });
      return undefined;
    });
    const zone = await screen.findByTestId("file-drop-zone");
    const png = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "hero.png", { type: "image/png" });
    fireEvent.drop(zone, { dataTransfer: { files: [png] } });
    expect(await screen.findByText("1 files uploaded.")).toBeInTheDocument();
    expect(sent).toHaveLength(1);
    expect(sent[0].url).toContain("name=hero.png");
    expect(sent[0].type).toBe("application/octet-stream");
    expect(sent[0].size).toBe(4);
    expect(await screen.findByRole("table", { name: "Files" })).toBeInTheDocument();
  });

  it("shows the server's limit refusal for an upload over the cap", async () => {
    await openFiles((path, request) => {
      if (path === FILES && request.method === "POST") return problem(422, "file: the Organization's files would hold 210000000 bytes; its limit is 209715200 bytes (200 MiB)");
      if (path === FILES) return jsonResponse({ items: [], limits: LIMITS });
      return undefined;
    });
    fireEvent.drop(await screen.findByTestId("file-drop-zone"), { dataTransfer: { files: [new File(["a,b"], "big.csv")] } });
    expect(await screen.findByRole("alert")).toHaveTextContent("big.csv: file: the Organization's files would hold 210000000 bytes; its limit is 209715200 bytes (200 MiB)");
  });

  it("renames a file and deletes an unused one only after a confirm", async () => {
    const user = userEvent.setup();
    const calls: string[] = [];
    await openFiles(async (path, request) => {
      if (path === `${FILES}/f-2` && request.method === "PATCH") {
        calls.push(`PATCH ${JSON.stringify(await request.json())}`);
        return jsonResponse({ ...REPORT, name: "vyrocna.pdf" });
      }
      if (path === `${FILES}/f-2` && request.method === "DELETE") {
        calls.push("DELETE");
        return new Response(null, { status: 204 });
      }
      if (path === FILES) return jsonResponse({ items: [HERO, REPORT], limits: LIMITS });
      return undefined;
    });
    await user.click(await screen.findByRole("button", { name: "Rename report.pdf" }));
    const input = screen.getByLabelText(/^New name/);
    expect(screen.getByText(/at most 120 characters, for example Titulná fotka.png/)).toBeInTheDocument();
    await user.clear(input);
    await user.type(input, "vyrocna.pdf");
    await user.click(screen.getByRole("button", { name: "Rename" }));
    expect(await screen.findByText("The file was renamed.")).toBeInTheDocument();
    expect(calls).toEqual(['PATCH {"name":"vyrocna.pdf"}']);

    await user.click(screen.getByRole("button", { name: "Delete report.pdf" }));
    expect(screen.getByText("Delete report.pdf?")).toBeInTheDocument();
    expect(calls).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Delete the file" }));
    await waitFor(() => expect(calls).toContain("DELETE"));
    expect(await screen.findByText("report.pdf was deleted.")).toBeInTheDocument();
  });

  it("says when the installation stores no files", async () => {
    await openFiles((path) => (path === FILES ? problem(503, "this installation stores no open data portal files") : undefined));
    expect(await screen.findByText("No file store here")).toBeInTheDocument();
  });
});
