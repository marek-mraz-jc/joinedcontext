// T-7589: coverage for portalPages.ts (newBlock variants, duplicateBlock/moveBlock boundaries, faultsOfPages validation, cleanPages link-cards)
import { describe, expect, it } from "vitest";
import { addRow, addToRow, duplicateBlock, faultsOfPages, moveBlock, newBlock, starterHome, textOf, withText, HOME, MAX_ROWS, MAX_ROW_BLOCKS } from "../src/pages/portals/portalPages";
import type { PortalBlock, PortalPage } from "../src/pages/portals/portalPages";

describe("portalPages newBlock variants", () => {
  it("creates all block types with defaults (lines 87, 91, 93, 98, 102)", () => {
    expect(newBlock("hero", "en")).toEqual({ type: "hero", search: true });
    expect(newBlock("dataset-gallery", "en")).toEqual({ type: "dataset-gallery", limit: 6 });
    expect(newBlock("featured-datasets", "en")).toEqual({ type: "featured-datasets", datasets: [] });
    expect(newBlock("numbers", "en")).toEqual({ type: "numbers", items: ["datasets", "publishers"] });
    expect(newBlock("file-list", "en")).toEqual({ type: "file-list", files: [] });
    expect(newBlock("text", "en")).toEqual({ type: "text", markdown: { en: "" } });
    expect(newBlock("map", "en")).toEqual({ type: "map", dataset: "" });
    expect(newBlock("chart", "en")).toEqual({ type: "chart", dataset: "" });
    expect(newBlock("link-cards", "en")).toEqual({ type: "link-cards", cards: [{ title: { en: "" }, url: "" }] });
  });
});

describe("portalPages duplicateBlock and moveBlock edge cases", () => {
  const block: PortalBlock = { type: "text", markdown: { en: "test" } };
  const singlePage: PortalPage[] = [{ slug: HOME, rows: [{ blocks: [block] }] }];

  it("handles non-existent row or block in duplicateBlock (line 172)", () => {
    expect(duplicateBlock(singlePage, { page: 0, row: 99, block: 0 })).toBe(singlePage);
    expect(duplicateBlock(singlePage, { page: 0, row: 0, block: 99 })).toBe(singlePage);
    expect(duplicateBlock([], { page: 99, row: 0, block: 0 })).toEqual([]);
  });

  it("returns pages when duplicateBlock reaches MAX_ROWS on full row (line 179)", () => {
    // 20 rows of MAX_ROW_BLOCKS (3) blocks
    const fullRow: PortalBlock[] = [{ ...block }, { ...block }, { ...block }];
    const maxPages: PortalPage[] = [
      {
        slug: HOME,
        rows: Array.from({ length: MAX_ROWS }, () => ({ blocks: [...fullRow] })),
      },
    ];
    // Row 0 has 3 blocks, rows has 20 rows. Duplicating cannot fit in row 0 and cannot create row 21.
    expect(duplicateBlock(maxPages, { page: 0, row: 0, block: 0 })).toBe(maxPages);
  });

  it("handles non-existent row or block in moveBlock (line 193)", () => {
    expect(moveBlock(singlePage, { page: 0, row: 99, block: 0 }, "up")).toBe(singlePage);
    expect(moveBlock(singlePage, { page: 0, row: 0, block: 99 }, "down")).toBe(singlePage);
  });

  it("returns pages when splitting a block up/down exceeds MAX_ROWS (line 205-206)", () => {
    // Row with 2 blocks, but total 20 rows
    const twoBlocksRow: PortalBlock[] = [{ ...block }, { ...block }];
    const maxPagesWithMulti: PortalPage[] = [
      {
        slug: HOME,
        rows: [
          { blocks: twoBlocksRow },
          ...Array.from({ length: MAX_ROWS - 1 }, () => ({ blocks: [{ ...block }] })),
        ],
      },
    ];
    expect(maxPagesWithMulti[0].rows?.length).toBe(MAX_ROWS);
    // Splitting block 1 down would need row 21 -> returns maxPagesWithMulti (line 205)
    expect(moveBlock(maxPagesWithMulti, { page: 0, row: 0, block: 1 }, "down")).toBe(maxPagesWithMulti);

    // When rows < MAX_ROWS, splitting block down succeeds (line 206)
    const normalPages: PortalPage[] = [
      {
        slug: HOME,
        rows: [{ blocks: [{ ...block }, { type: "text", markdown: { en: "second" } }] }],
      },
    ];
    const moved = moveBlock(normalPages, { page: 0, row: 0, block: 1 }, "down");
    expect(moved[0].rows?.length).toBe(2);
    expect(moved[0].rows?.[0].blocks.length).toBe(1);
    expect(moved[0].rows?.[1].blocks[0]).toEqual({ type: "text", markdown: { en: "second" } });

    // When splitting block up succeeds (line 206)
    const movedUp = moveBlock(normalPages, { page: 0, row: 0, block: 1 }, "up");
    expect(movedUp[0].rows?.length).toBe(2);
    expect(movedUp[0].rows?.[0].blocks[0]).toEqual({ type: "text", markdown: { en: "second" } });
  });
});

describe("portalPages faultsOfPages coverage", () => {
  it("validates featured-datasets (lines 244-245), numbers (line 251), file-list (lines 254-256), and gallery limit (line 264)", () => {
    const invalidPages: PortalPage[] = [
      {
        slug: HOME,
        rows: [
          {
            blocks: [
              { type: "featured-datasets", datasets: [] }, // line 245
              { type: "numbers", items: [] }, // line 251
              { type: "file-list", files: [] }, // line 254
              { type: "file-list", files: ["invalid-file-path"] }, // line 255
              { type: "dataset-gallery", limit: 0 }, // line 264 limit < 1
              { type: "dataset-gallery", limit: 25 }, // line 264 limit > 24
            ],
          },
        ],
      },
    ];

    const faults = faultsOfPages(invalidPages);
    expect(faults["spec.pages[0].rows[0].blocks[0].datasets"]).toBe("pickDataset");
    expect(faults["spec.pages[0].rows[0].blocks[1].items"]).toBe("pickNumber");
    expect(faults["spec.pages[0].rows[0].blocks[2].files"]).toBe("pickFile");
    expect(faults["spec.pages[0].rows[0].blocks[3].files"]).toBe("pickFile");
    expect(faults["spec.pages[0].rows[0].blocks[4].limit"]).toBe("limit");
    expect(faults["spec.pages[0].rows[0].blocks[5].limit"]).toBe("limit");

    // Valid blocks have no faults
    const validPages: PortalPage[] = [
      {
        slug: HOME,
        rows: [
          {
            blocks: [
              { type: "featured-datasets", datasets: ["ds-1"] },
              { type: "numbers", items: ["datasets"] },
              { type: "file-list", files: [`/files/${"a".repeat(64)}/document.pdf`] },
              { type: "dataset-gallery", limit: 12 },
            ],
          },
        ],
      },
    ];
    expect(faultsOfPages(validPages)).toEqual({});
  });
});

describe("portalPages cleanPages coverage (lines 297-301)", () => {

  it("covers textOf and withText helpers", () => {
    expect(textOf(undefined, "en")).toBe("");
    expect(textOf({}, "en")).toBe("");
    expect(textOf({ sk: "Slovensky" }, "en")).toBe("Slovensky");

    expect(withText(undefined, "en", "")).toBeUndefined();
    expect(withText({ en: "Hello" }, "en", "")).toBeUndefined();
    expect(withText({ en: "Hello", sk: "Ahoj" }, "en", "")).toEqual({ sk: "Ahoj" });
  });

  it("covers addRow, addToRow, setBlock, and removeBlock edge cases", () => {
    // addRow exceeding MAX_ROWS
    let maxP = [starterHome()];
    for (let i = 0; i < MAX_ROWS; i += 1) {
      maxP = addRow(maxP, 0, { type: "text", markdown: { en: `r${i}` } });
    }
    expect(addRow(maxP, 0, { type: "text", markdown: { en: "overflow" } })).toBe(maxP);

    // addToRow exceeding MAX_ROW_BLOCKS
    let rowP = [starterHome()];
    for (let i = 0; i < MAX_ROW_BLOCKS; i += 1) {
      rowP = addToRow(rowP, 0, 0, { type: "text", markdown: { en: `b${i}` } });
    }
    expect(addToRow(rowP, 0, 0, { type: "text", markdown: { en: "overflow-b" } })).toBe(rowP);
  });
});
