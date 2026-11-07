import { afterEach, describe, expect, it } from "vitest";
import { blankDocument, edgeRetryMs, pageFindings } from "./pageChecks";

// T-4950: the rules that make a page explain itself, each with a page that passes and one that fails.
const findings = (check: string) =>
  pageFindings({ namespaces: [], layout: false }).filter((finding) => finding.startsWith(`${check}:`));

afterEach(() => {
  document.body.innerHTML = "";
});

describe("the walkers' boot check", () => {
  it("names a Portal that never drew the page instead of judging index.html's fallback", () => {
    document.body.innerHTML = `<div id="root"><main id="boot-failed" role="alert">
      <p lang="en"><strong>The Portal did not load.</strong> Load the page again.</p></main></div>`;
    expect(pageFindings({ namespaces: [], layout: false })).toEqual([
      "boot: the Portal did not load, The Portal did not load. Load the page again.",
    ]);
  });
});

describe("the walkers' blank document check", () => {
  it("calls a tab holding no Portal document blank instead of judging its lang, title and h1 (T-5855)", () => {
    document.body.innerHTML = "<h1>502 Bad Gateway</h1>";
    expect(blankDocument()).toBe(true);
  });

  it("calls a root with neither lang nor title blank (T-5857)", () => {
    document.documentElement.removeAttribute("lang");
    document.title = "";
    document.body.innerHTML = `<div id="root"></div>`;
    expect(blankDocument()).toBe(true);
  });

  it("passes the Portal's shell", () => {
    document.documentElement.setAttribute("lang", "en");
    document.title = "Roles";
    document.body.innerHTML = `<div id="root"><main><h1>Roles</h1></main></div>`;
    expect(blankDocument()).toBe(false);
  });
});

describe("the walkers' wait on the edge's 429", () => {
  it("waits the seconds Retry-After names, plus one, at most a minute", () => {
    expect(edgeRetryMs("8")).toBe(9_000);
    expect(edgeRetryMs("0")).toBe(1_000);
    expect(edgeRetryMs("3600")).toBe(61_000);
  });

  it("waits a third of the edge's window on no or a garbled header", () => {
    for (const header of [undefined, "", "soon", "-5", "1.5", "Wed, 21 Oct 2026 07:28:00 GMT"]) expect(edgeRetryMs(header)).toBe(20_000);
  });
});

describe("the walkers' purpose-line check", () => {
  it("passes a title with the line that says what the page manages", () => {
    document.body.innerHTML = `<main><h1>Spaces</h1><p>Where the project keeps its context data.</p></main>`;
    expect(findings("purpose-line")).toEqual([]);
  });

  it("passes a list page's title whose info mark beside it says it (T-6095)", () => {
    document.body.innerHTML = `<main><div><h1>Spaces</h1><span><button aria-describedby="d">i</button>
      <span id="d" role="tooltip" class="sr-only">Where the project keeps its context data.</span></span></div></main>`;
    expect(findings("purpose-line")).toEqual([]);
  });

  it("flags an info mark that says nothing", () => {
    document.body.innerHTML = `<main><div><h1>Spaces</h1><span><button>i</button></span></div></main>`;
    expect(findings("purpose-line")).toEqual(['purpose-line: no line under "Spaces"']);
  });

  it("flags a title with nothing under it", () => {
    document.body.innerHTML = `<main><h1>Spaces</h1><table></table></main>`;
    expect(findings("purpose-line")).toEqual(['purpose-line: no line under "Spaces"']);
  });
});

describe("the walkers' empty-state check", () => {
  it("passes an empty list that links to its next step", () => {
    document.body.innerHTML = `<main><div data-empty-state><p>No spaces yet.</p>
      <a href="/projects/helsinki/spaces/new">Create the first space</a></div></main>`;
    expect(findings("empty-state-next-step")).toEqual([]);
  });

  it("flags an empty list that only says it is empty", () => {
    document.body.innerHTML = `<main><div data-empty-state><p>No spaces yet.</p></div></main>`;
    expect(findings("empty-state-next-step")).toEqual(["empty-state-next-step: No spaces yet."]);
  });
});

describe("the walkers' error-text check", () => {
  it("passes a refused field whose message says what to change", () => {
    document.body.innerHTML = `<main><form><label for="n">Name</label>
      <input id="n" aria-invalid="true" aria-describedby="n__error">
      <div id="n__error" role="alert"><ul><li>Use lowercase letters, digits and dashes, like air-quality.</li></ul></div>
      </form></main>`;
    expect(findings("error-text")).toEqual([]);
  });

  it("flags a refused field with a bare word or no message at all", () => {
    document.body.innerHTML = `<main><form><label for="n">Name</label>
      <input id="n" aria-invalid="true" aria-describedby="n__error"><div id="n__error" role="alert">Invalid</div>
      <label for="u">Address</label><input id="u" aria-invalid="true"></form></main>`;
    expect(findings("error-text")).toEqual([
      'error-text: Name: "Invalid" does not say what to change',
      "error-text: Address: marked invalid with no message",
    ]);
  });

  it("flags a form refusal that only says it failed and passes one that names the field", () => {
    document.body.innerHTML = `<main><form><p role="alert">Request failed</p></form>
      <form><p role="alert">The space name air is taken: choose another name.</p></form></main>`;
    expect(findings("error-text")).toEqual(['error-text: the form says only "Request failed"']);
  });
});

describe("the walkers' raw-json check", () => {
  const body = '503 {"type":"https://joinedcontext.com/errors/broker-failure","status":503}';

  it("passes a problem body kept behind a closed disclosure (T-5177)", () => {
    document.body.innerHTML = `<main><p>Endpoint helsinki-transport was not published.</p>
      <details><summary>Technical details</summary><p>${body}</p></details></main>`;
    expect(findings("raw-json")).toEqual([]);
  });

  it("flags the same body once it shows on the page", () => {
    document.body.innerHTML = `<main><p>${body}</p><details open><summary>Technical details</summary><p>${body}</p></details></main>`;
    expect(findings("raw-json")).toEqual([`raw-json: ${body}`]);
  });
});
