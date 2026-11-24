// Appearance tab, remaining branches: legacy single font split, emptied appearance, text field, discard, stylesheet markup.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { jsonResponse, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk" }] };
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal", "Organization"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const CHANGE = { metadata: { name: "chg-77", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };
type Sent = { url: string; body: { spec: Record<string, unknown> } };

async function open(stored: Record<string, unknown>, sent: Sent[]) {
  await renderRoute({
    path: "/portals/bbsk-doprava",
    permissions: ADMIN,
    answer: (path, request) => {
      if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
      if (path === "/api/v1/projects/org/organizations") return jsonResponse({ kind: "List", items: [] });
      if (path === "/api/v1/projects/org/portals/bbsk-doprava" && request.method === "GET") {
        return jsonResponse({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Portal",
          metadata: { name: "bbsk-doprava", namespace: "org" },
          spec: { organizationRef: "bbsk", path: "doprava", title: { sk: "Doprava" }, languages: ["sk"], ...stored },
        });
      }
      if (path === "/api/v1/projects/org/portals/bbsk-doprava" && request.method === "PUT") {
        return request.json().then((body: Sent["body"]) => {
          sent.push({ url: request.url, body });
          return jsonResponse(request.url.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
        });
      }
      if (path.startsWith("/api/v1/projects/org/changes/history")) return jsonResponse({ apiVersion: "v1", kind: "List", items: [] });
      return undefined;
    },
  });
  fireEvent.click(await screen.findByRole("tab", { name: "Appearance" }));
}

describe("the appearance tab, more branches", () => {
  it("splits a legacy single font into the two fonts when one is chosen", async () => {
    const sent: Sent[] = [];
    await open({ appearance: { font: "serif" } }, sent);
    const heading = await screen.findByLabelText(/^Heading font/);
    fireEvent.change(heading, { target: { value: "mono" } });
    fireEvent.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.appearance).toEqual({ headingFont: "mono", bodyFont: "serif" });
    expect(await screen.findByText(/chg-77/)).toBeInTheDocument();
  });

  it("resets one font of a legacy pair and keeps the other", async () => {
    const sent: Sent[] = [];
    await open({ appearance: { font: "serif" } }, sent);
    fireEvent.click(await screen.findByRole("button", { name: "Reset Heading font to inherited" }));
    fireEvent.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.appearance).toEqual({ bodyFont: "serif" });
  });

  it("proposes a portal without an appearance member once every field is reset", async () => {
    const sent: Sent[] = [];
    await open({ appearance: { radius: 4 } }, sent);
    fireEvent.click(await screen.findByRole("button", { name: "Reset Corner radius (px) to inherited" }));
    fireEvent.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect("appearance" in (sent[1]?.body.spec ?? {})).toBe(false);
  });

  it("types a plain text field and clears it again, then discards the edit", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await open({ appearance: { instanceName: "Doprava" } }, sent);
    const name = await screen.findByLabelText(/^Full name/);
    expect(screen.getByText("The appearance shows as published.")).toBeInTheDocument();
    await user.clear(name);
    expect(name).toHaveValue("");
    await user.type(name, "Doprava BBSK");
    expect(screen.getByText(/Unpublished changes/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Discard" }));
    expect(screen.getByLabelText(/^Full name/)).toHaveValue("Doprava");
    expect(screen.getByText("The appearance shows as published.")).toBeInTheDocument();
    expect(sent).toEqual([]);
  });

  it("refuses a stylesheet holding markup before sending", async () => {
    const sent: Sent[] = [];
    await open({}, sent);
    fireEvent.change(await screen.findByLabelText(/^Stylesheet/), { target: { value: "a { color: red }</style>" } });
    expect(await screen.findByText("Remove </: a stylesheet holds no markup.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Publish" }));
    expect(await screen.findByText("Some fields need a fix")).toBeInTheDocument();
    expect(sent).toEqual([]);
  });
});
