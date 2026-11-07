/** The research screen over what the public endpoint of `zilina-uniza` answers (T-3140). */
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App, { MOST } from "./App";
import { answer } from "./fixtures/works";
import { LOCALES } from "./locales";

const s = LOCALES.sk;

function show(respond?: (url: URL) => Response, withEndpoint = true, language = "sk") {
  const pages: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const url = new URL(path, "http://portal.test");
      pages.push(url.searchParams.get("offset") ?? "0");
      if (respond) return respond(url);
      const body = answer(url.searchParams.get("type"), Number(url.searchParams.get("offset") ?? 0), Number(url.searchParams.get("limit") ?? 1000));
      return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
    }),
  );
  // The list reads through `fetch` above; the entity panel reads one work through the client (SDK-40).
  const client = stubClient({ entities: answer("Article", 0, 1000) as Row[] }, {
    portal: "https://portal.zilina.sk/projects/zilina",
    slug: withEndpoint ? "os4quyrmvhijr3x7x4fno2jicej4ifv5" : "",
    orgDomain: "zilina.sk",
    space: withEndpoint ? "zilina-uniza" : "elsewhere",
    transport: "origin",
    appName: "zilina-vyskum",
    language,
  });
  return {
    ...render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    ),
    pages,
  };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const list = () => screen.getByRole("region", { name: s.list });

/** Opens every work the list shows and follows its DREPO link, the way a reader goes down it. */
function touchList() {
  // The handle opens in a new tab; the test keeps jsdom from navigating.
  const stay = (event: Event) => event.preventDefault();
  document.addEventListener("click", stay);
  for (const item of within(list()).queryAllByRole("listitem")) {
    fireEvent.click(within(item).getByRole("button"));
    const link = within(item).queryByRole("link");
    if (link) fireEvent.click(link);
  }
  document.removeEventListener("click", stay);
}

describe("the research screen", () => {
  it("reads every page and totals the works, their years and their journals", async () => {
    const { pages } = show();
    expect(await screen.findByText(s.works(251))).toBeInTheDocument();
    expect(pages).toEqual(["0", "200"]);
    const table = screen.getByRole("table", { name: s.perYearTable });
    expect(within(table).getByRole("row", { name: "2023 65" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: s.topSeries })).toHaveTextContent("Práce a štúdie");
  });

  it("lists the newest first, 25 at a time, and narrows by kind, year and search", async () => {
    show();
    await screen.findByText(s.works(251));
    expect(within(list()).getAllByRole("listitem")).toHaveLength(25);
    await userEvent.click(within(list()).getByRole("button", { name: s.more }));
    expect(within(list()).getAllByRole("listitem")).toHaveLength(50);
    touchList();
    await userEvent.click(screen.getByRole("radio", { name: /^Zborník/ }));
    expect(screen.getByText(s.shown(25, 33))).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.pickYear }), "2023");
    touchList();
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "neexistujuce slovo");
    expect(within(list()).getByText(s.none)).toBeInTheDocument();
  }, 20_000); // 251 works paged and filtered: slow under coverage on CI (948fcf9 timed out at 5 s)

  it("links each work to its handle and shows no author", async () => {
    const { container } = show();
    await screen.findByText(s.works(251));
    const link = within(list()).getAllByRole("link")[0];
    expect(link.getAttribute("href")).toMatch(/^http:\/\/drepo\.uniza\.sk\/handle\/hdluniza\/\d+$/);
    expect(container.textContent).not.toContain("Kardoš");
    expect(screen.getByText(s.noAuthors)).toBeInTheDocument();
  });

  // SDK-40, AP-140: a public App opens a work in the panel, which links to the Portal and edits nothing.
  it("opens a work in the entity panel, with a Portal link and no Edit", async () => {
    show();
    await screen.findByText(s.works(251));
    const first = within(list()).getAllByRole("button", { pressed: false })[0];
    await userEvent.click(first);
    const panel = await screen.findByRole("dialog");
    expect(first).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(await within(panel).findByRole("link", { name: "Otvoriť v Portáli" }));
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    await userEvent.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(first).toHaveAttribute("aria-pressed", "false");
  });

  it("opens each work of each kind and links it to DREPO", async () => {
    show();
    await screen.findByText(s.works(251));
    touchList();
    for (const radio of screen.getAllByRole("radio")) {
      fireEvent.click(radio);
      touchList();
    }
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    // Some 150 opens under the coverage run's controls recorder.
  }, 30_000);

  it("speaks English to an English reader, every control in English", async () => {
    const en = LOCALES.en;
    show(undefined, true, "en");
    expect(await screen.findByText(en.works(251))).toBeInTheDocument();
    const list = () => screen.getByRole("region", { name: en.list });
    const touch = () => {
      const stay = (event: Event) => event.preventDefault();
      document.addEventListener("click", stay);
      for (const item of within(list()).queryAllByRole("listitem")) {
        fireEvent.click(within(item).getByRole("button"));
        const link = within(item).queryByRole("link");
        if (link) fireEvent.click(link);
      }
      document.removeEventListener("click", stay);
    };
    touch();
    for (const radio of screen.getAllByRole("radio")) {
      fireEvent.click(radio);
      touch();
    }
    fireEvent.click(screen.getByRole("radio", { name: new RegExp(`^${en.allKinds}`) }));
    await userEvent.click(within(list()).getByRole("button", { name: en.more }));
    touch();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: en.pickYear }), "2023");
    touch();
    const panel = await screen.findByRole("dialog");
    await userEvent.click(await within(panel).findByRole("link", { name: "Open in the Portal" }));
    await userEvent.click(within(panel).getByRole("button", { name: "Close" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: en.pickYear }), "");
    // The whole search at once: typed letter by letter, each letter would list works of its own.
    fireEvent.change(screen.getByRole("searchbox", { name: en.search }), { target: { value: "nothing like this" } });
    expect(within(list()).getByText(en.none)).toBeInTheDocument();
  }, 30_000);

  it("lists a work the repository says little about, in Slovak when the reader's language is unknown", async () => {
    const P = (value: unknown) => ({ type: "Property", value });
    const rows = [
      { id: "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:bare", type: "Article" },
      {
        id: "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:odd",
        type: "Article",
        name: { type: "LanguageProperty", languageMap: { la: "Opus" } },
        workType: P("Thesis"),
        yearPublished: P(2020),
        url: P("javascript:alert(1)"),
      },
    ];
    show(() => new Response(JSON.stringify(rows), { status: 200, headers: { "content-type": "application/json" } }), true, undefined as unknown as string);
    expect(await screen.findByText(s.works(2))).toBeInTheDocument();
    const items = within(list()).getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("OpusThesis · 2020 · la");
    expect(items[1]).toHaveTextContent(`${s.noTitle}${s.noYear}`);
    // A link that is not http or https is not shown at all.
    expect(within(list()).queryAllByRole("link")).toEqual([]);
    touchList();
    fireEvent.click(screen.getByRole("radio", { name: "Thesis (1)" }));
    fireEvent.click(screen.getByRole("radio", { name: `${s.allKinds} (2)` }));
  });

  it("says how many it shows when the repository holds more than the screen reads", async () => {
    const full = answer("Article", 0, 200);
    show(() => new Response(JSON.stringify(full), { status: 200, headers: { "content-type": "application/json" } }));
    expect(await screen.findByText(s.truncated(MOST))).toBeInTheDocument();
    touchList();
  });

  it("says a failure that is not even an error as it came, and keeps quiet once the page is gone", async () => {
    const { unmount } = show(() => {
      throw "the proxy hung up";
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(s.refused("the proxy hung up"));
    unmount();
    // Gone before the read answers: the answer lands nowhere.
    show().unmount();
    await new Promise((done) => setTimeout(done, 0));
  });

  it("says why the endpoint refused, and that it has nothing to read without one", async () => {
    show(() => new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "no policy" }), { status: 403, headers: { "content-type": "application/problem+json" } }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no policy");
  });

  it("has nothing to read without an endpoint of the university's space", () => {
    show(undefined, false);
    expect(screen.getAllByRole("status")[0]).toHaveTextContent(s.noEndpoint);
  });

  it("names its source and has nothing axe finds", async () => {
    const { container } = show();
    await screen.findByText(s.works(251));
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
