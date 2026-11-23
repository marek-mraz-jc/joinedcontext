/**
 * T-3108, API/01 §33: a type of a space published as a public link — a public Endpoint narrowed to
 * the type and the attributes left ticked, proposed with its Policy as one Change — and the page at
 * `/v/{slug}` that reads that Endpoint anonymously.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/PublicView.tsx.
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApiError } from "../src/api/client";
import { PublicViewPage, failureText, inLanguage, retryAfterSeconds, retryPublic } from "../src/pages/spaces/PublicView";
import { SharePanel, publishRequest } from "../src/pages/spaces/SharePanel";
import { publicName } from "../src/pages/spaces/publicName";
import { expectNoViolations } from "./checks";
import { renderPage } from "./page_contract";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
  });

describe("what is published", () => {
  it("names the Endpoint after the space and the type, within 63 characters", () => {
    expect(publicName("bikes", "BikeHireDockingStation")).toBe("bikes-bikehiredockingstation-public");
    expect(publicName("x".repeat(70), "T").length).toBeLessThanOrEqual(63);
  });

  it("asks for this type alone, public, and hides every attribute not left ticked", () => {
    expect(publishRequest("bikes", "bikes-public", "Station", ["name", "free", "note"], ["name", "free"])).toEqual({
      contextSpace: "bikes",
      name: "bikes-public",
      audience: "public",
      entityTypes: ["Station"],
      hiddenAttributes: ["note"],
    });
  });
});

describe("publishing a type", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("proposes the rendered Endpoint with its Policy as one Change and gives the link", async () => {
    const sent: { path: string; body: unknown; dryRun: boolean }[] = [];
    renderPage(<SharePanel project="helsinki" space="bikes" type="Station" attributes={["name", "free", "note"]} />, {
      path: "/projects/helsinki/spaces/bikes",
      answer: async (url, request) => {
        if (request.method !== "POST") return undefined;
        const body = (await request.json()) as unknown;
        sent.push({ path: url.pathname, body, dryRun: url.searchParams.get("dryRun") === "All" });
        if (url.pathname.endsWith("/assistant/propose-endpoint")) {
          return json({
            lane: "red",
            slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
            endpoint: { apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "bikes-station-public" }, spec: {} },
            policies: [{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Policy", metadata: { name: "bikes-station-public-public" }, spec: {} }],
          });
        }
        return url.searchParams.get("dryRun") === "All"
          ? json({ valid: true })
          : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-0000aa01", namespace: "helsinki" }, status: { lane: "red", phase: "PendingApproval" } }, 202);
      },
    });
    const panel = await screen.findByTestId("view-share");
    await userEvent.click(within(panel).getByText(en.spaces.share.title));
    await userEvent.click(within(panel).getByRole("checkbox", { name: "note" }));
    await expectNoViolations(panel);
    await userEvent.click(within(panel).getByRole("button", { name: en.spaces.share.publish }));

    await waitFor(() => expect(sent.filter((s) => s.path.endsWith("/import") && !s.dryRun)).toHaveLength(1));
    expect(sent[0].body).toEqual({
      contextSpace: "bikes",
      name: "bikes-station-public",
      audience: "public",
      entityTypes: ["Station"],
      hiddenAttributes: ["note"],
    });
    const imported = sent.find((s) => s.path.endsWith("/import") && !s.dryRun)?.body as { manifests: { kind: string }[] };
    expect(imported.manifests.map((m) => m.kind)).toEqual(["Policy", "Endpoint"]);
    expect(await within(panel).findByTestId("share-link")).toHaveTextContent(
      `${window.location.origin}/v/k7m2qz4tv6xh3n5jb2ryd3wcfa`,
    );
  });

  it("says why the Endpoint could not be drawn up, in the API's words or the failure's, and proposes nothing", async () => {
    const sent: string[] = [];
    let calls = 0;
    renderPage(<SharePanel project="helsinki" space="bikes" type="Station" attributes={["name"]} />, {
      path: "/projects/helsinki/spaces/bikes",
      answer: async (url, request) => {
        if (request.method !== "POST") return undefined;
        sent.push(url.pathname);
        calls += 1;
        if (calls === 1) return json({ title: "Forbidden", status: 403, detail: "no right to publish bikes" }, 403);
        throw new TypeError("Failed to fetch");
      },
    });
    const panel = await screen.findByTestId("view-share");
    await userEvent.click(within(panel).getByText(en.spaces.share.title));
    await userEvent.click(within(panel).getByRole("button", { name: en.spaces.share.publish }));
    expect(await within(panel).findByText(/no right to publish bikes/)).toBeInTheDocument();
    await userEvent.click(within(panel).getByRole("button", { name: en.spaces.share.publish }));
    expect(await within(panel).findByText(/Failed to fetch/)).toBeInTheDocument();
    expect(sent.some((path) => path.endsWith("/import"))).toBe(false);
    expect(within(panel).queryByTestId("share-link")).toBeNull();
  });

  it("keeps the proposal closed with the reason while the name is no name", async () => {
    renderPage(<SharePanel project="helsinki" space="bikes" type="Station" attributes={["name"]} />, {
      path: "/projects/helsinki/spaces/bikes",
      answer: async () => undefined,
    });
    const panel = await screen.findByTestId("view-share");
    await userEvent.click(within(panel).getByText(en.spaces.share.title));
    const name = within(panel).getByLabelText(new RegExp(`^${en.spaces.share.name}`));
    await userEvent.clear(name);
    await userEvent.type(name, "Not A Name");
    expect(within(panel).getByRole("button", { name: en.spaces.share.publish })).toHaveAttribute("aria-disabled", "true");
    expect(name).toHaveAttribute("aria-invalid", "true");
  });
});

describe("the public link", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** The Endpoint `abc` answering with its catalogue entry, one type and two rows; `fail` answers every read. */
  const publicEndpoint = (reads: Request[], fail?: () => Response | Promise<Response>) => ({
    path: "/v/abc",
    answer: async (url: URL, request: Request) => {
      if (!url.pathname.startsWith("/api/endpoint/abc/")) return undefined;
      reads.push(request);
      if (fail) return fail();
      if (url.pathname === "/api/endpoint/abc/") {
        return json({ "dct:title": [{ "@language": "ca", "@value": "Estacions" }, { "@language": "en", "@value": "Bike stations" }] });
      }
      if (url.pathname.endsWith("/types")) return json({ typeList: ["BikeHireDockingStation"] });
      return json([
        { id: "urn:ngsi-ld:BikeHireDockingStation:hsl:1", type: "BikeHireDockingStation", name: "Kamppi", availableBikeNumber: 4 },
        { id: "urn:ngsi-ld:BikeHireDockingStation:hsl:2", type: "BikeHireDockingStation", name: "Töölö" },
      ]);
    },
  });

  it("reads the Endpoint anonymously and shows its entities under the catalogue's title and labelled heads", async () => {
    const reads: Request[] = [];
    renderPage(<PublicViewPage slug="abc" />, publicEndpoint(reads));
    const table = await screen.findByRole("table", { name: "Bike hire docking station" });
    expect(await screen.findByRole("heading", { level: 1, name: "Bike stations" })).toBeInTheDocument();
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent?.replace(/⋮$/, ""))).toEqual([en.entityGrid.id, "Name", "Available bike number"]);
    expect(within(table).getByText("Kamppi")).toBeInTheDocument();
    expect(within(table).getByText("1")).toHaveAttribute("title", "urn:ngsi-ld:BikeHireDockingStation:hsl:1");
    expect(table.textContent).not.toContain("urn:");
    expect(screen.getByTestId("public-view").textContent).not.toContain("abc");
    expect(screen.getByText("2 entities.")).toBeInTheDocument();
    // The rows are on the shared grid (T-6752): its search narrows what the read returned.
    fireEvent.change(within(screen.getByTestId("public-view")).getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "Töölö" } });
    expect(within(table).queryByText("Kamppi")).toBeNull();
    expect(within(table).getByText("Töölö")).toBeInTheDocument();
    expect(reads.every((request) => request.credentials === "omit")).toBe(true);
    const entities = reads.find((request) => new URL(request.url).pathname.endsWith("/entities"));
    expect(new URL(entities?.url ?? "http://x").searchParams.get("options")).toBe("keyValues");
    await expectNoViolations(screen.getByTestId("public-view"));
  });

  const DOWN = en.spaces.share.downUnnamed;
  const statuses: Array<[string, () => Response, string, number]> = [
    ["404 says the view is not published", () => json({ title: "Not Found" }, 404), en.spaces.share.notPublished, 1],
    ["401 says the view is not open to the public", () => json({ title: "Unauthorized" }, 401), en.spaces.share.closed, 1],
    ["403 says the view is not open to the public", () => json({ title: "Forbidden" }, 403), en.spaces.share.closed, 1],
    [
      "429 says how long to wait",
      () => new Response("{}", { status: 429, headers: { "Retry-After": "30", "Content-Type": "application/problem+json" } }),
      "Too many visits just now. Try again in 30 s.",
      1,
    ],
    ["503 says the service did not answer, after two retries", () => json({ title: "Service Unavailable" }, 503), DOWN, 3],
  ];

  it.each(statuses)("%s", async (_name, answer, message, tries) => {
    const reads: Request[] = [];
    renderPage(<PublicViewPage slug="abc" />, publicEndpoint(reads, answer));
    expect(await screen.findByText(message, undefined, { timeout: 4000 })).toBeInTheDocument();
    const typeReads = reads.filter((request) => new URL(request.url).pathname.endsWith("/types"));
    expect(typeReads).toHaveLength(tries);
    expect(screen.getByRole("heading", { level: 1, name: en.spaces.share.heading })).toBeInTheDocument();
    expect(screen.getByTestId("public-view").textContent).not.toContain("abc");
    // A refusal is final; only a failure (5xx, network) or a wait (429) offers to ask again.
    expect(screen.queryByRole("button", { name: en.app.error.retry }) !== null).toBe(tries > 1 || message.startsWith("Too many"));
    await expectNoViolations(screen.getByTestId("public-view"));
  });

  it("says the service did not answer when the network fails, and reads again on Retry", async () => {
    const reads: Request[] = [];
    let down = true;
    renderPage(
      <PublicViewPage slug="abc" />,
      publicEndpoint(reads, () => {
        if (down) throw new TypeError("Failed to fetch");
        return json({ typeList: [] });
      }),
    );
    expect(await screen.findByText(DOWN, undefined, { timeout: 4000 })).toBeInTheDocument();
    expect(reads.filter((request) => new URL(request.url).pathname.endsWith("/types"))).toHaveLength(3);
    down = false;
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(en.spaces.share.notPublished)).toBeInTheDocument();
  });
});

describe("the public link with several types", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("offers each type as a pressed button, reads the chosen one, and reads a failed type again on Retry", async () => {
    const reads: string[] = [];
    let parkingDown = true;
    renderPage(<PublicViewPage slug="two" />, {
      path: "/v/two",
      answer: async (url: URL) => {
        if (!url.pathname.startsWith("/api/endpoint/two/")) return undefined;
        reads.push(url.pathname + url.search);
        if (url.pathname === "/api/endpoint/two/") return json({});
        if (url.pathname.endsWith("/types")) return json(["BikeHireDockingStation", "OffStreetParking", 7]);
        if (url.searchParams.get("type") === "OffStreetParking") {
          if (parkingDown) return json({ title: "Service Unavailable" }, 503);
          return json([{ id: "urn:ngsi-ld:OffStreetParking:1", type: "OffStreetParking", name: "Kamppi P" }]);
        }
        return json([{ id: "urn:ngsi-ld:BikeHireDockingStation:1", type: "BikeHireDockingStation", name: "Kamppi" }]);
      },
    });
    const group = await screen.findByRole("group", { name: en.spaces.share.types });
    const buttons = within(group).getAllByRole("button");
    // A type name that is no string is no type.
    expect(buttons).toHaveLength(2);
    expect(buttons[0]).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByText("Kamppi")).toBeInTheDocument();
    // No catalogue title: the chosen type names the page.
    expect(screen.getByRole("heading", { level: 1, name: "Bike hire docking station" })).toBeInTheDocument();
    await userEvent.click(buttons[1]);
    expect(buttons[1]).toHaveAttribute("aria-pressed", "true");
    // A 5xx is read twice more, then offered to the reader to ask again.
    const retry = await screen.findByRole("button", { name: en.app.error.retry }, { timeout: 5000 });
    parkingDown = false;
    await userEvent.click(retry);
    expect(await screen.findByText("Kamppi P")).toBeInTheDocument();
    expect(reads.filter((read) => read.includes("type=OffStreetParking"))).toHaveLength(4);
  }, 15_000);
});

describe("the catalogue title", () => {
  const title = [
    { "@language": "sk", "@value": "Stanice" },
    { "@language": "en", "@value": "Stations" },
    { "@language": "de-AT", "@value": "Stationen (AT)" },
    { "@language": 5, "@value": 7 },
  ];

  it("is the page's language, then its base, then English, then the first", () => {
    expect(inLanguage(title, "de-AT")).toBe("Stationen (AT)");
    expect(inLanguage(title, "sk-SK")).toBe("Stanice");
    expect(inLanguage(title, "fi")).toBe("Stations");
    expect(inLanguage([{ "@language": "ca", "@value": "Estacions" }, "Plain"], "fi")).toBe("Estacions");
    expect(inLanguage("Plain", "fi")).toBe("Plain");
    expect(inLanguage({ "@value": "Untagged" }, "fi")).toBe("Untagged");
    expect(inLanguage(undefined, "fi")).toBeUndefined();
    expect(inLanguage([null, 3], "fi")).toBeUndefined();
  });
});

describe("reading a failure", () => {
  it("names the view and the next step when the service is down", () => {
    const t = (key: string, options?: Record<string, unknown>) => `${key} ${String(options?.name)}`;
    expect(failureText(new ApiError(503, "Service Unavailable"), t, "Air quality")).toBe("spaces.share.down Air quality");
    expect(failureText(new ApiError(503, "Service Unavailable"), t)).toBe("spaces.share.downUnnamed undefined");
    expect(en.spaces.share.down).toContain("{name}");
    expect(en.spaces.share.down).toContain("Try again in a minute");
  });

  it("retries a 5xx and a network failure twice, never a refusal", () => {
    expect(retryPublic(0, new ApiError(503, "x"))).toBe(true);
    expect(retryPublic(1, new TypeError("Failed to fetch"))).toBe(true);
    expect(retryPublic(2, new ApiError(500, "x"))).toBe(false);
    for (const status of [401, 403, 404, 429]) expect(retryPublic(0, new ApiError(status, "x"))).toBe(false);
  });

  it("reads Retry-After as seconds or as a date", () => {
    expect(retryAfterSeconds("12")).toBe(12);
    expect(retryAfterSeconds("Wed, 21 Oct 2026 07:28:30 GMT", Date.parse("Wed, 21 Oct 2026 07:28:00 GMT"))).toBe(30);
    expect(retryAfterSeconds("soon")).toBeUndefined();
    expect(retryAfterSeconds(null)).toBeUndefined();
  });
});
