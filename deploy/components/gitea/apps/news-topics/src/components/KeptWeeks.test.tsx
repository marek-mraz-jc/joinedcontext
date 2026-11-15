import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { KeptWeeks } from "./KeptWeeks";
import { Problem } from "../server";
import type { Kept, Server } from "../server";
import { memory } from "../testing/server";

const KEPT: Kept = {
  weeks: [
    {
      week: "2026-W42",
      articles: 3,
      computed_at: "2026-10-12T10:00:00Z",
      topics: [
        { topic: 0, share: 2 / 3, articles: 2, keywords: [{ term: "raitiotie", weight: 0.9 }, { term: "liikenne", weight: 0.7 }, { term: "ratikka", weight: 0.5 }, { term: "katu", weight: 0.1 }] },
        { topic: 1, share: 1 / 3, articles: 1, keywords: [{ term: "kirjasto", weight: 0.8 }] },
      ],
    },
    { week: "2026-W41", articles: 0, computed_at: "2026-10-12T10:00:00Z", topics: [] },
  ],
  stale: true,
};

function serverOf(kept: Kept | Error, corpus: string | Error = "https://store.test/corpus/2026-W42.json"): Server {
  return {
    ...memory(),
    weeks: async () => {
      if (kept instanceof Error) throw kept;
      return kept;
    },
    corpusUrl: async () => {
      if (corpus instanceof Error) throw corpus;
      return corpus;
    },
  };
}

describe("the weeks the server keeps", () => {
  afterEach(() => vi.restoreAllMocks());

  it("lists each week's topics by their keywords, the newest first, says they are as last kept, and opens a week's articles", async () => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    render(<KeptWeeks lang="en" server={serverOf(KEPT)} />);
    const table = await screen.findByRole("table", { name: "Topics week by week" });
    expect(within(table).getAllByRole("row").map((row) => row.textContent)).toEqual([
      "WeekArticlesTopics, the largest firstArticles as a file",
      "2026-W423raitiotie, liikenne, ratikka (67%); kirjasto (33%)Articles as a file",
      "2026-W410–Articles as a file",
    ]);
    expect(screen.getByText("The feed could not be read just now: these are the weeks as last kept.")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Topics week by week" })).toHaveAttribute("tabindex", "0");
    fireEvent.click(screen.getByRole("button", { name: "Download the articles of week 2026-W42" }));
    await waitFor(() => expect(open).toHaveBeenCalledWith("https://store.test/corpus/2026-W42.json", "_blank", "noopener"));
  });

  it("says in Finnish when a week's articles cannot be downloaded", async () => {
    render(<KeptWeeks lang="fi" server={serverOf(KEPT, new Problem(404, "the server keeps no such week"))} />);
    fireEvent.click(await screen.findByRole("button", { name: "Lataa viikon artikkelit 2026-W42" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Viikon artikkeleita ei voitu ladata: the server keeps no such week");
  });

  it("says when nothing is kept yet, and when the weeks cannot be read", async () => {
    const { unmount } = render(<KeptWeeks lang="en" server={serverOf({ weeks: [], stale: false })} />);
    expect(screen.getByText("Reading the kept weeks…")).toBeInTheDocument();
    expect(await screen.findByText("The server has not kept any week yet.")).toBeInTheDocument();
    unmount();
    render(<KeptWeeks lang="en" server={serverOf(new Problem(502, "the gateway could not answer right now; try again shortly (502)"))} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("The kept weeks could not be read: the gateway could not answer right now; try again shortly (502)");
  });
});
