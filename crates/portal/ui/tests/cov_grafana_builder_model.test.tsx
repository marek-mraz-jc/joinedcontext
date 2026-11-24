// T-7589: coverage for builderModel missing title, leafWords, fieldId, and lineInWords branches
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { boardOf, fieldId, lineInWords, missing, newPanel } from "../src/pages/grafana/builderModel";
import type { Board } from "../src/pages/grafana/builderModel";

describe("builderModel additional coverage", () => {
  const t = (key: string, values?: Record<string, unknown>) => i18n.t(key, values);
  const sampleBoard: Board = boardOf({
    title: "Test Board",
    from: "now-6h",
    to: "now",
    refresh: "",
    variables: [
      { name: "station", label: "Station", endpoint: "ep", type: "T", attribute: "a" },
    ],
    panels: [
      {
        id: 1,
        kind: "timeseries",
        title: "Temperature",
        grid: { x: 0, y: 0, w: 12, h: 8 },
        endpoint: "ep",
        type: "T",
        attribute: "temp",
        aggregation: "avg",
        interval: "auto",
      },
    ],
  });

  it("checks missing title validation to cover line 114", () => {
    const emptyTitlePanel = newPanel(sampleBoard, "stat", "");
    expect(missing(emptyTitlePanel)).toContain("builder.need.title");

    const whitespaceTitlePanel = newPanel(sampleBoard, "stat", "    ");
    expect(missing(whitespaceTitlePanel)).toContain("builder.need.title");
  });

  it("covers fieldId branches for unlisted panel, filter subfields, grid, id, thresholds, variables, and board fields (lines 224, 226-228, 232-235)", () => {
    // Panel index not in board.panels -> id is undefined -> returns undefined (line 224)
    expect(fieldId("panels[999].title", sampleBoard)).toBeUndefined();

    // Filters: with specific subfield and without subfield (line 226)
    expect(fieldId("panels[0].filters[1].value", sampleBoard)).toBe("builder-filter-1-1-value");
    expect(fieldId("panels[0].filters[2]", sampleBoard)).toBe("builder-filter-1-2-attribute");

    // Grid and id (line 227)
    expect(fieldId("panels[0].grid", sampleBoard)).toBe("builder-panel-1-x");
    expect(fieldId("panels[0].id", sampleBoard)).toBe("builder-panel-1-x");

    // Thresholds (line 228)
    expect(fieldId("panels[0].thresholds", sampleBoard)).toBe("builder-panel-1-title");

    // Variables with and without subfield (line 232-233)
    expect(fieldId("variables[0].name", sampleBoard)).toBe("builder-var-0-name");
    expect(fieldId("variables[0]", sampleBoard)).toBe("builder-var-0-name");
    expect(fieldId("variables[0].label", sampleBoard)).toBe("builder-var-0-label");

    // Top-level board fields: title, description, from, refresh, and unlisted field (lines 234-235)
    expect(fieldId("title", sampleBoard)).toBe("builder-title");
    expect(fieldId("description", sampleBoard)).toBe("builder-description");
    expect(fieldId("from", sampleBoard)).toBe("builder-from");
    expect(fieldId("refresh", sampleBoard)).toBe("builder-refresh");
    expect(fieldId("nonexistent", sampleBoard)).toBeUndefined();
  });

  it("covers lineInWords for matching path with and without message, path 'board', and unmapped lines (line 241)", () => {
    // Matching top-level path with message
    const withMsg = lineInWords("title: title cannot be empty", sampleBoard, t);
    expect(withMsg).toBe("Title: title cannot be empty");

    // Matching top-level path without message (path only)
    const withoutMsg = lineInWords("title", sampleBoard, t);
    expect(withoutMsg).toBe("Title");

    // Matching panel path without message
    const panelWithoutMsg = lineInWords("panels[0].title", sampleBoard, t);
    expect(panelWithoutMsg).toBe("Panel “Temperature”, Title");

    // Path is 'board' -> returns line as is
    expect(lineInWords("board: unexpected error", sampleBoard, t)).toBe("board: unexpected error");

    // Arbitrary unmapped line -> returns line as is
    expect(lineInWords("an arbitrary error message", sampleBoard, t)).toBe("an arbitrary error message");
    expect(lineInWords("custom_field: error occurred", sampleBoard, t)).toBe("custom_field: error occurred");
  });
});
