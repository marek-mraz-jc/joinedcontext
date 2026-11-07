import { describe, expect, it } from "vitest";
import { label, markdown, sources } from "../src/pages/knowledge/answerFormat";

// T-3325, AG-117: the same reading of an answer as the public widget's (platform
// crates/assistant/widget/render.test.js).
const upTo = (max: number) => (n: number) => n >= 1 && n <= max;

describe("an answer's Markdown", () => {
  it("reads a pipe table, the rule making its first row the header, as the widget does (T-4649)", () => {
    expect(markdown("| Stanica | NO₂ |\n|:---|---:|\n| **Centrum** | 12 \\| 14 |\nend")).toEqual([
      {
        tag: "table",
        children: [
          { tag: "tr", children: [{ tag: "th", children: ["Stanica"] }, { tag: "th", children: ["NO₂"] }] },
          { tag: "tr", children: [{ tag: "td", children: [{ tag: "strong", children: ["Centrum"] }] }, { tag: "td", children: ["12 | 14"] }] },
        ],
      },
      { tag: "p", children: ["end"] },
    ]);
    expect(markdown("a | b")).toEqual([{ tag: "p", children: ["a | b"] }]);
  });

  it("keeps a fenced block's lines as code, markers and HTML as typed", () => {
    expect(markdown("```\nconst a = \"**x**\";\n\n<script>alert(1)</script>\n```\nafter")).toEqual([
      { tag: "pre", text: 'const a = "**x**";\n\n<script>alert(1)</script>' },
      { tag: "p", children: ["after"] },
    ]);
  });

  it("keeps a paragraph's breaks and starts the next at a blank line", () => {
    expect(markdown("one\ntwo\n\nthree")).toEqual([
      { tag: "p", children: ["one", { tag: "br" }, "two"] },
      { tag: "p", children: ["three"] },
    ]);
  });

  it("reads bullet and numbered lists, an indented line staying with its item", () => {
    expect(markdown("- a\n* b\n  more\n1. c\n2) d")).toEqual([
      { tag: "ul", children: [{ tag: "li", children: ["a"] }, { tag: "li", children: ["b", { tag: "br" }, "more"] }] },
      { tag: "ol", children: [{ tag: "li", children: ["c"] }, { tag: "li", children: ["d"] }] },
    ]);
  });

  it("reads bold, italic and code, a heading as bold, and leaves snake_case alone", () => {
    expect(markdown("## Events\n**Workshop for _Families_** at `10:00`, *noon*, query_entities_tool")).toEqual([
      { tag: "p", children: [{ tag: "strong", children: ["Events"] }] },
      {
        tag: "p",
        children: [
          { tag: "strong", children: ["Workshop for ", { tag: "em", children: ["Families"] }] },
          " at ",
          { tag: "code", children: ["10:00"] },
          ", ",
          { tag: "em", children: ["noon"] },
          ", query_entities_tool",
        ],
      },
    ]);
  });

  it("links http(s) only and keeps HTML as text", () => {
    expect(markdown("[city](https://hel.fi/x) [bad](javascript:alert(1)) <script>alert(1)</script>")).toEqual([
      {
        tag: "p",
        children: [{ tag: "a", href: "https://hel.fi/x", children: ["city"] }, " (hel.fi)", " [bad](javascript:alert(1)) <script>alert(1)</script>"],
      },
    ]);
  });

  it("shows where a link leads beside words that do not say it (F-R2UI3-002)", () => {
    expect(markdown("see [hel.fi](https://evil.example/login)")).toEqual([
      { tag: "p", children: ["see ", { tag: "a", href: "https://evil.example/login", children: ["hel.fi"] }, " (evil.example)"] },
    ]);
    expect(markdown("[hel.fi](https://el.fi/login)")).toEqual([
      { tag: "p", children: [{ tag: "a", href: "https://el.fi/login", children: ["hel.fi"] }, " (el.fi)"] },
    ]);
    expect(markdown("[Pay at mybank.com.example](https://bank.com)")).toEqual([
      { tag: "p", children: [{ tag: "a", href: "https://bank.com", children: ["Pay at mybank.com.example"] }, " (bank.com)"] },
    ]);
    expect(markdown("[Open HEL.FI events](https://hel.fi/x)")).toEqual([
      { tag: "p", children: [{ tag: "a", href: "https://hel.fi/x", children: ["Open HEL.FI events"] }] },
    ]);
    expect(markdown("[www.hel.fi](https://www.hel.fi/x)")).toEqual([
      { tag: "p", children: [{ tag: "a", href: "https://www.hel.fi/x", children: ["www.hel.fi"] }] },
    ]);
  });

  it("turns markers into citations and drops an unknown number", () => {
    expect(markdown("x [2, 3] y [9] z [1, 9]", upTo(3))).toEqual([
      { tag: "p", children: ["x ", { tag: "cite", numbers: [2, 3] }, " y ", " z ", { tag: "cite", numbers: [1] }] },
    ]);
  });
});

describe("an answer's sources", () => {
  it("lists one per address by title, and live data never by its tool", () => {
    const { list, position } = sources([
      { n: 1, url: "https://opendata.dev.joinedcontext.com/dataset/helsinki-events", title: "Helsinki events" },
      { n: 2, url: "https://opendata.dev.joinedcontext.com/dataset/helsinki-events" },
      { n: 3, tool: "query_entities", endpoint: "helsinki-events" },
      { n: 4, url: "javascript:alert(1)" },
    ]);
    expect(list).toEqual([
      { url: "https://opendata.dev.joinedcontext.com/dataset/helsinki-events", title: "Helsinki events", live: false, endpoint: null, domain: "opendata.dev.joinedcontext.com", numbers: [1, 2] },
      { url: null, title: null, live: true, endpoint: "helsinki-events", domain: "", numbers: [3] },
    ]);
    expect(position).toEqual({ 1: 1, 2: 1, 3: 2 });
    expect(label(list[1])).toBe("helsinki-events");
    expect(label({ url: "https://www.hel.fi/en/events/", title: null, endpoint: null })).toBe("www.hel.fi/en/events");
    expect(JSON.stringify(list)).not.toMatch(/query_entities/);
  });

  it("names an uploaded file it cites, without a link (T-4869)", () => {
    const { list, position } = sources([
      { n: 1, url: "upload:garden/kumpula-garden-rules.pdf" },
      { n: 2, url: "upload:garden/kumpula-garden-rules.pdf#page=2", title: "Kumpula garden rules" },
      { n: 3, url: "upload:garden/minutes.docx", title: "Committee minutes" },
    ]);
    expect(list).toEqual([
      { url: null, title: "kumpula-garden-rules.pdf", live: false, endpoint: null, domain: "", numbers: [1, 2] },
      { url: null, title: "Committee minutes", live: false, endpoint: null, domain: "", numbers: [3] },
    ]);
    expect(position).toEqual({ 1: 1, 2: 1, 3: 2 });
    expect(label(list[0])).toBe("kumpula-garden-rules.pdf");
  });
});
