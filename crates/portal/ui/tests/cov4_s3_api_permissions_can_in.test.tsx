// T-7706: usePermissions answers canIn per space: a grant of one space opens that space and no other.
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { usePermissions } from "../src/api/permissions";

function Probe() {
  const permissions = usePermissions("helsinki");
  return (
    <p>
      {permissions.data ? `kamppi:${permissions.canIn("Endpoint", "propose", "kamppi")} other:${permissions.canIn("Endpoint", "propose", "other")}` : "waiting"}
    </p>
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("canIn", () => {
  it("allows the verb in the space a grant names and in no other", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(
          JSON.stringify({ project: "helsinki", bootstrap: false, grants: [{ role: "editor", binding: "b", scope: "project:helsinki", space: "kamppi", rule: { kinds: ["Endpoint"], verbs: ["propose"] } }] }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      ),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <Probe />
      </QueryClientProvider>,
    );
    expect(await screen.findByText(/kamppi:true other:false/)).toBeInTheDocument();
  });
});
