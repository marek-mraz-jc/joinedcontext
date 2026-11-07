/**
 * T-0793: the assistant hands a page off through the address, and a page reads what it was
 * handed only as it mounts. A second hand-off to a page already open must therefore mount it
 * again — for the explorer that means a change of the endpoint alone (AG-73, UI-46).
 */
import { act, render, screen } from "@testing-library/react";
import { useEffect, useState } from "react";
import type { JSX } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

let searchStr = "";
let mounts = 0;

vi.mock("@tanstack/react-router", () => ({
  useRouter: () => ({ history: { flush: () => {} } }),
  useRouterState: ({ select }: { select: (state: { location: { searchStr: string } }) => string }) =>
    select({ location: { searchStr } }),
}));

const { HandOff, HAND_OFF } = await import("../src/assistant/HandOff");
const { handPrefill, takePrefill } = await import("../src/assistant/state");
const { runTabAction } = await import("../src/connectors/pairedTab");

/** A page that takes what it was handed at the moment it mounted, as every hand-off page does. */
function Page(): JSX.Element {
  const [opened] = useState(() => new URLSearchParams(searchStr).get("endpoint") ?? "");
  useEffect(() => {
    mounts += 1;
  }, []);
  return <p>opened {opened}</p>;
}

const handedOff = () => (
  <HandOff>
    <Page />
  </HandOff>
);

describe("a hand-off from the assistant", () => {
  beforeEach(() => {
    mounts = 0;
  });

  it("mounts the page again when the address names another endpoint", async () => {
    searchStr = "?space=ovzdusie&endpoint=public-air";
    const { rerender } = render(handedOff());
    expect(await screen.findByText("opened public-air")).toBeInTheDocument();

    searchStr = "?space=ovzdusie&endpoint=internal-air";
    rerender(handedOff());

    expect(await screen.findByText("opened internal-air")).toBeInTheDocument();
    expect(mounts).toBe(2);
  });

  it("leaves the page alone when nothing of the hand-off changed", async () => {
    searchStr = "?endpoint=public-air";
    const { rerender } = render(handedOff());
    await screen.findByText("opened public-air");

    searchStr = "?endpoint=public-air&offset=20";
    rerender(handedOff());

    expect(mounts).toBe(1);
  });

  it("mounts the open form again when a newer plan is handed to the same address (JC2-11)", async () => {
    window.history.replaceState(null, "", "/projects/helsinki/models");
    searchStr = "?edit=air";
    function Editor(): JSX.Element {
      const [source] = useState(() => String(takePrefill(window.location.pathname)?.source ?? "none"));
      useEffect(() => {
        mounts += 1;
      }, []);
      return <p>source {source}</p>;
    }
    handPrefill("/projects/helsinki/models?edit=air", { source: "first plan" });
    render(
      <HandOff>
        <Editor />
      </HandOff>,
    );
    expect(await screen.findByText("source first plan")).toBeInTheDocument();

    // The paired tab runs the connector's second show_diff: same address, a newer source.
    const go = vi.fn();
    await act(async () => {
      await runTabAction(
        {
          frame: "navigate",
          payload: { route: "/projects/helsinki/models?edit=air", prefill: { source: "second plan" } },
          tool: "jc_ui_show_diff",
          from: { connector: "claude-code" },
        },
        go,
      );
    });
    expect(await screen.findByText("source second plan")).toBeInTheDocument();
    expect(mounts).toBe(2);
    expect(go).toHaveBeenCalledWith("/projects/helsinki/models?edit=air");
  });

  it("carries every name a page is handed", () => {
    expect([...HAND_OFF]).toEqual(["edit", "delete", "grant", "draft", "space", "endpoint"]);
  });
});
