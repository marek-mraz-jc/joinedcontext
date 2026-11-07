/**
 * A private dataset refused for who is asking (T-6376): a 401 is the signed-out state, a 403 the
 * no-access state, each designed, with no row and no second read.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { endpointSource } from "../src/grid/source";
import { DEFAULT_LABELS, refusalOf } from "../src/grid/useEntityGrid";
import { SourceError } from "../src/grid/source";
import type { Transport } from "../src/sdk/transport";

const config = parseGridConfig({
  source: { kind: "endpoint", slug: "private-aaaaaaaaaaaaaaaaaaaaaaaa" },
  type: "Station",
  columns: [{ attr: "name", label: "Name", pinned: true }],
  pageSize: 10,
}).config!;

afterEach(cleanup);

function answering(status: number): { transport: Transport; calls: () => number } {
  let calls = 0;
  // Only the row reads count: the grid also asks the Endpoint's per-attribute access once, and
  // the total by a read of its own with no rows (T-6541).
  const transport: Transport = async (request) => {
    if (request.path.includes("/entities?") && !request.path.includes("limit=0")) calls += 1;
    return { status, body: { type: "https://uri.etsi.org/ngsi-ld/errors/Unauthorized", title: "Unauthorized", detail: "sign in" } };
  };
  return { transport, calls: () => calls };
}

describe("a refused read of a private dataset", () => {
  it("maps 401 and 403 to the two states and every other status to none", () => {
    expect(refusalOf(new SourceError(401, "x"))).toBe("signedOut");
    expect(refusalOf(new SourceError(403, "x"))).toBe("noAccess");
    expect(refusalOf(new SourceError(404, "x"))).toBeNull();
    expect(refusalOf(new SourceError(500, "x"))).toBeNull();
    expect(refusalOf(new Error("x"))).toBeNull();
  });

  it("shows the signed-out state with the host's sign-in, no table and no retry", async () => {
    const { transport, calls } = answering(401);
    const onSignIn = vi.fn();
    render(<EntityGrid config={config} source={endpointSource("private-aaaaaaaaaaaaaaaaaaaaaaaa", transport)} onSignIn={onSignIn} />);
    expect(await screen.findByRole("heading", { name: DEFAULT_LABELS.signedOutTitle })).toBeTruthy();
    expect(screen.getByText(DEFAULT_LABELS.signedOutBody)).toBeTruthy();
    expect(screen.queryByRole("grid")).toBeNull();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByText(DEFAULT_LABELS.empty)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.signIn }));
    expect(onSignIn).toHaveBeenCalledOnce();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(calls()).toBe(1);
  });

  it("shows the no-access state for a 403, with no sign-in button", async () => {
    const { transport, calls } = answering(403);
    render(<EntityGrid config={config} source={endpointSource("private-aaaaaaaaaaaaaaaaaaaaaaaa", transport)} onSignIn={() => {}} />);
    expect(await screen.findByRole("heading", { name: DEFAULT_LABELS.noAccessTitle })).toBeTruthy();
    expect(screen.queryByRole("button", { name: DEFAULT_LABELS.signIn })).toBeNull();
    await waitFor(() => expect(calls()).toBe(1));
  });

  it("drops the rows of an earlier answer when the next read is refused", async () => {
    let status = 200;
    const transport: Transport = async () =>
      status === 200
        ? { status, body: [{ id: "urn:ngsi-ld:Station:1", type: "Station", name: { type: "Property", value: "Secret station" } }] }
        : { status, body: {} };
    const source = endpointSource("private-aaaaaaaaaaaaaaaaaaaaaaaa", transport);
    const { rerender } = render(<EntityGrid config={config} source={source} reloadKey={0} />);
    expect(await screen.findByText("Secret station")).toBeTruthy();
    status = 401;
    rerender(<EntityGrid config={config} source={source} reloadKey={1} />);
    expect(await screen.findByRole("heading", { name: DEFAULT_LABELS.signedOutTitle })).toBeTruthy();
    expect(screen.queryByText("Secret station")).toBeNull();
  });
});
