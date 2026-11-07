import React, { useCallback, useEffect, useRef, useState } from "react";
import type { EntityGrid as EntityGridApi } from "./useEntityGrid";

const FALLBACK_ROW_HEIGHT = 36;
const FALLBACK_VIEWPORT_ROWS = 30;
/** Rows drawn beyond the window on each side, so a scroll shows rows rather than blank space. */
const OVERSCAN = 10;
/** The share of the loaded rows the window's end passes before the next page is asked for. */
const PREFETCH_AT = 0.6;

/** Scrolling instead of pages: which rows are drawn, how tall the box is, and when the next page is asked. */
export function useRowWindow(virtual: boolean, grid: EntityGridApi) {
  const { rows, state } = grid;
  const scrollRef = useRef<HTMLDivElement>(null);
  // A scrolling table ends at the window's bottom with its footer in sight, so the last row and the
  // totals never need a page scroll (owner 2026-10-30). Measured from the page top, so a scrolled
  // page keeps the height; at least 15rem where the window is short.
  const [fitHeight, setFitHeight] = useState<number>();
  useEffect(() => {
    const box = scrollRef.current;
    if (!virtual || !box) return;
    const fit = () => {
      const rect = box.getBoundingClientRect();
      const below = (box.parentElement?.getBoundingClientRect().bottom ?? rect.bottom) - rect.bottom;
      const top = rect.top + window.scrollY;
      setFitHeight(Math.max(240, window.innerHeight - top - below - 8));
    };
    fit();
    // Content above that settles after the first paint (a banner, a toolbar wrapping) moves the top.
    const moved = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(fit);
    moved?.observe(document.body);
    window.addEventListener("resize", fit);
    return () => {
      moved?.disconnect();
      window.removeEventListener("resize", fit);
    };
  }, [virtual]);
  const rowHeight = useRef(FALLBACK_ROW_HEIGHT);
  const [windowStart, setWindowStart] = useState(0);
  const [windowSize, setWindowSize] = useState(FALLBACK_VIEWPORT_ROWS);
  /**
   * The window of rows in sight, read off the scroll position. Once its end passes 60 % of the
   * loaded rows the next page is asked for, so it is there before the person reaches the end
   * (T-5623).
   */
  const onScroll = useCallback(() => {
    const box = scrollRef.current;
    if (!virtual || !box) return;
    const first = box.querySelector<HTMLTableRowElement>("tbody tr:not(.jc-grid-spacer)");
    if (first && first.offsetHeight > 0) rowHeight.current = first.offsetHeight;
    const height = rowHeight.current;
    const inSight = box.clientHeight > 0 ? Math.ceil(box.clientHeight / height) : FALLBACK_VIEWPORT_ROWS;
    const start = Math.max(0, Math.floor(box.scrollTop / height) - OVERSCAN);
    setWindowStart(start);
    setWindowSize(inSight + 2 * OVERSCAN);
    if (grid.more && start + inSight + 2 * OVERSCAN >= rows.length * PREFETCH_AT) {
      grid.loadMore();
    }
  }, [virtual, grid, rows.length]);
  // A page that lands while the window is still past the mark asks for the next one: a person who
  // scrolled to the end waits for no further scroll event, which a scroll box at its end never sends.
  React.useEffect(() => {
    if (virtual && !grid.loadingMore) onScroll();
  }, [virtual, grid.loadingMore, rows.length, onScroll]);
  // A key that moves the active cell out of the window scrolls to it; the scroll moves the window.
  // Only a move of the cell does: a person scrolling away from it is not pulled back (T-5623).
  const scrolledTo = useRef<number | undefined>(undefined);
  React.useEffect(() => {
    const row = state.activeCell?.row;
    const box = scrollRef.current;
    if (!virtual || row === undefined || !box || row === scrolledTo.current) return;
    scrolledTo.current = row;
    if (row < windowStart || row >= windowStart + windowSize) {
      box.scrollTop = row * rowHeight.current;
      onScroll();
    }
  }, [virtual, state.activeCell?.row, windowStart, windowSize, onScroll]);
  const shownFrom = virtual ? Math.min(windowStart, Math.max(0, rows.length - 1)) : 0;
  const shownRows = virtual ? rows.slice(shownFrom, shownFrom + windowSize) : rows;
  const spacerAbove = virtual ? shownFrom * rowHeight.current : 0;
  const spacerBelow = virtual ? Math.max(0, rows.length - shownFrom - shownRows.length) * rowHeight.current : 0;
  return { scrollRef, fitHeight, onScroll, shownFrom, shownRows, spacerAbove, spacerBelow };
}
