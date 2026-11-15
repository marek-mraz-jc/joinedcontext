import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { useNetwork } from "./network";

describe("reading HSL's network", () => {
  it("reads every page, past the 5000 rows the SDK's `all` would stop at", async () => {
    const stops = Array.from({ length: 2500 }, (_, i) => ({
      id: `urn:ngsi-ld:GtfsStop:hel.fi:helsinki:${1000000 + i}`,
      type: "GtfsStop",
      name: `Stop ${i}`,
      location: { type: "Point", coordinates: [24.9 + i / 1e5, 60.17] },
    })) as Row[];
    const line = { id: "urn:ngsi-ld:PublicTransportRoute:hel.fi:helsinki:1-1", type: "PublicTransportRoute", routeShortName: "1", stopSequence: "1000000, 1002499" } as Row;
    const client = stubClient(
      { entities: [...stops, line], access: { permissions: [{ resource: { type: "*" }, actions: ["queryEntity"], attributes: "*" }], prohibitions: [] } },
      { appName: "transit-reach" },
    );
    const wrapper = ({ children }: { children: ReactNode }) => <JcProvider client={client}>{children}</JcProvider>;
    const { result } = renderHook(() => useNetwork(), { wrapper });
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.error).toBeNull();
    expect(result.current.network.stops).toHaveLength(2500);
    expect(result.current.network.routes).toEqual([{ name: "1", stops: ["1000000", "1002499"] }]);
    // Three pages of stops (1000, 1000, 500) and one of lines.
    expect(client.transport.calls.filter((call) => call.path.includes("type=GtfsStop"))).toHaveLength(3);
  });
});
