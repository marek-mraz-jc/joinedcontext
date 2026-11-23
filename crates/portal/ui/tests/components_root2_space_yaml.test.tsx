/** The space usage panel closes when the window moves; yamlManifest on the shapes the view test leaves out. */
import { fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { SpaceUsageInfo, UsageInfo } from "../src/components/SpaceUsageInfo";
import type { SpaceUsage } from "../src/api/spaceUsage";
import { describe as describeField, pathAt } from "../src/components/yamlManifest";
import { renderPage } from "./page_contract";

const KEPT: SpaceUsage = {
  entities: 40_000,
  broker: "main",
  observedAt: "2026-10-31T16:00:00Z",
  measuredAt: "2026-10-31T16:00:00Z",
  estimated: false,
};

describe("the space usage panel", () => {
  it.each([
    ["resize", () => fireEvent(window, new Event("resize"))],
    ["scroll", () => fireEvent(window, new Event("scroll"))],
  ])("closes on a window %s", async (_name, move) => {
    renderPage(<SpaceUsageInfo project="helsinki" space="air" usage={KEPT} locale="en" />, {});
    await userEvent.click(await screen.findByTitle("When the size of air was measured"));
    expect(screen.getByTestId("space-measured")).toBeInTheDocument();
    move();
    expect(screen.queryByTestId("space-measured")).not.toBeInTheDocument();
  });
});

describe("the space usage panel, by keyboard", () => {
  it("closes on Escape and gives the focus back to the (i)", async () => {
    renderPage(<SpaceUsageInfo project="helsinki" space="air" usage={KEPT} locale="en" />, {});
    const open = await screen.findByTitle("When the size of air was measured");
    await userEvent.click(open);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByTestId("space-measured")).not.toBeInTheDocument();
    expect(open).toHaveFocus();
  });

  it("is shown only once the usage has come", async () => {
    const first = renderPage(<UsageInfo project="helsinki" space="air" usage={undefined} locale="en" />, {});
    expect(screen.queryByTitle("When the size of air was measured")).not.toBeInTheDocument();
    first.unmount();
    renderPage(<UsageInfo project="helsinki" space="air" usage={KEPT} locale="en" />, {});
    expect(await screen.findByTitle("When the size of air was measured")).toBeInTheDocument();
  });
});

describe("yamlManifest", () => {
  it("follows an array step to the schema's items", () => {
    const schema = { properties: { list: { items: { description: "one entry" } } } };
    expect(describeField(schema, ["list", 0])).toBe("one entry");
  });

  it("names no path inside a complex (non-scalar) key", () => {
    expect(pathAt("? [a, b]\n: 1\nname: x\n", 3)).toBeNull();
    expect(pathAt("? [a, b]\n: 1\nname: x\n", 20)).toEqual(["name"]);
  });

  it("describes a field found in an allOf / anyOf branch, following $ref, and stops at a missing node", () => {
    const schema = {
      definitions: { Spec: { description: "the spec", properties: { rules: { description: "the rules" } } } },
      allOf: [{ $ref: "#/definitions/Spec" }],
      properties: { spec: { anyOf: [{ properties: { other: { description: "other" } } }, { $ref: "#/definitions/Spec" }] } },
    };
    expect(describeField(schema, ["rules"])).toBe("the rules");
    expect(describeField(schema, ["spec", "rules"])).toBe("the rules");
    expect(describeField(schema, ["spec", "other"])).toBe("other");
    expect(describeField(schema, ["spec", "nothing", "deeper"])).toBeNull();
    expect(describeField(schema, ["nothing"])).toBeNull();
  });
});
