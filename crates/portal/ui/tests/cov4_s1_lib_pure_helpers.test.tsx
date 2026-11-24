// T-7706: small helpers with unrun branches: lifetime ends, role presets, copy into a preview, explainAttribute, decision errors, access words, panel specs, representation outputs, titles, colours, history values, paging words, assistant notes.
import { renderToString } from "react-dom/server";
import { render, renderHook, screen, waitFor, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { asKind, problemOf, sweepIso } from "../src/pages/pipelines/lifetime";
import { matchRolePreset, presetOfRules } from "../src/components/forms/widgets/accessPresets";
import { MAX_BYTES, copyIntoPreview } from "../src/pages/workspaces/copyIntoPreview";
import { explainAttribute } from "../src/api/access";
import { useDecision } from "../src/api/decision";
import { useIsPhone } from "../src/phone";
import { RulesInWords, areaOf } from "../src/components/AccessWords";
import { seriesColors, stateTimelineSpec } from "../src/components/panels/specs";
import { chartTheme } from "../src/components/panels/VegaChart";
import type { Frame, Series } from "../src/components/panels/frames";
import { newOutput, withText } from "../src/components/forms/widgets/representationOutputs";
import { changesOf } from "../src/pages/pipelines/spaceChanges";
import { onPageTitle, resetDocumentTitle } from "../src/documentTitle";
import { plainColourOf } from "../src/components/dashboards/mapColours";
import { valueOf } from "../src/components/entities/history/historyApi";
import { pagingWords } from "../src/pages/datasources/templates";
import { runUiAction } from "../src/assistant/uiActions";
import { doorsAsRest } from "./odata";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("lifetime ends", () => {
  const now = new Date("2026-10-01T10:00:30Z");
  const t = (key: string) => key;

  it("turns another kind of end into a from-end that keeps the span and the cut-off", () => {
    expect(asKind({ rolling: "PT1H", notAfter: "2027-01-01T00:00:00Z" }, "from", now)).toEqual({ from: "observedAt", plus: "PT1H", notAfter: "2027-01-01T00:00:00Z" });
    expect(asKind({ from: "observedAt", plus: "PT2H", notAfter: "2027-01-01T00:00:00Z" }, "from", now)).toEqual({
      from: "observedAt",
      plus: "PT2H",
      notAfter: "2027-01-01T00:00:00Z",
    });
    expect(asKind({}, "from", now)).toEqual({ from: "observedAt", plus: "PT10M" });
  });

  it("names a date that is not a date, a date in the past, and a missing span", () => {
    expect(problemOf({ at: "not a date" }, t as never, now)).toBe("pipelines.lifetime.problem.date");
    expect(problemOf({ at: "2020-01-01T00:00:00Z" }, t as never, now)).toBe("pipelines.lifetime.problem.past");
    expect(problemOf({ at: "2030-01-01T00:00:00Z" }, t as never, now)).toBeUndefined();
    expect(problemOf({}, t as never, now)).toBe("pipelines.lifetime.problem.span");
    expect(problemOf({ rolling: "PT5M" }, t as never, now)).toBeUndefined();
  });

  it("reads a sweep as an ISO duration only when it is a whole number of hours or days", () => {
    expect(sweepIso("14d")).toBe("P14D");
    expect(sweepIso("6h")).toBe("PT6H");
    expect(sweepIso("0d")).toBeUndefined();
    expect(sweepIso("soon")).toBeUndefined();
    expect(sweepIso(undefined)).toBeUndefined();
  });
});

describe("role presets", () => {
  it("reads no preset from a role without rules", () => {
    expect(matchRolePreset(undefined, ["Entity"], ["read"])).toBeNull();
    expect(matchRolePreset([], ["Entity"], ["read"])).toBeNull();
    expect(presetOfRules(undefined)).toBeNull();
  });

  it("reads a list's role as a preset only when it has one read rule and at most one other", () => {
    const read = { kinds: ["Entity"], verbs: ["read"] };
    expect(presetOfRules([read])).toBe("reader");
    expect(presetOfRules([read, { kinds: ["Entity"], verbs: ["propose"] }, { kinds: ["Pipeline"], verbs: ["propose"] }])).toBeNull();
    expect(presetOfRules([read, { kinds: [], verbs: ["propose"] }])).toBeNull();
    expect(presetOfRules([{ kinds: [], verbs: ["read"] }])).toBeNull();
  });
});

describe("copying real data into a preview", () => {
  const ok = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

  it("stops with the refusal of a page it could not read", async () => {
    const fetcher = vi.fn(async (input: string) => {
      if (input.endsWith("/types")) return ok({ typeList: ["Event"] });
      return new Response(JSON.stringify({ title: "Forbidden", detail: "the endpoint refuses Event" }), { status: 403 });
    });
    expect(await copyIntoPreview("origin", "preview", fetcher)).toEqual({ copied: {}, stopped: "the endpoint refuses Event" });
  });

  it("copies nothing from a type with no entities", async () => {
    const fetcher = vi.fn(async (input: string) => (input.endsWith("/types") ? ok({ typeList: ["Event"] }) : ok([])));
    expect(await copyIntoPreview("origin", "preview", fetcher)).toEqual({ copied: {} });
    expect(fetcher.mock.calls.some(([input]) => input.includes("entityOperations"))).toBe(false);
  });

  it("stops at the byte bound before writing the page that crosses it", async () => {
    const fetcher = vi.fn(async (input: string) => {
      if (input.endsWith("/types")) return ok({ typeList: ["Event"] });
      return ok([{ id: "urn:ngsi-ld:Event:1", pad: "x".repeat(MAX_BYTES + 1) }]);
    });
    const result = await copyIntoPreview("origin", "preview", fetcher);
    expect(result.stopped).toBe("the copy stops at 10 MiB");
    expect(fetcher.mock.calls.some(([input]) => input.includes("entityOperations"))).toBe(false);
  });
});

describe("explaining a refused attribute", () => {
  it("names the policy behind the refusal, or none when the caller may not read it", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
        if (url.pathname.endsWith("/access/attributes")) {
          return new Response(JSON.stringify({ policy: "readers", policyTitle: "Readers", policyHidden: false }), { status: 200, headers: { "Content-Type": "application/json" } });
        }
        return new Response("{}", { status: 404 });
      }),
    );
    doorsAsRest();
    expect(await explainAttribute("helsinki", "air", "Event", "name", "read")).toEqual({ policy: "readers", policyTitle: "Readers", policyHidden: false });
  });
});

describe("a decision that fails for a reason that is not the API's", () => {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={new QueryClient()}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </QueryClientProvider>
  );

  it("says a network error in its own words, and an unknown rejection in the generic ones", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("network down"))));
    const { result } = renderHook(() => useDecision("helsinki", "chg-1"), { wrapper });
    act(() => result.current.reject.mutate("no"));
    await waitFor(() => expect(result.current.error).not.toBeNull());
    expect(result.current.refused).toBe(false);

    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject("boom")));
    const second = renderHook(() => useDecision("helsinki", "chg-2"), { wrapper });
    act(() => second.result.current.reject.mutate("no"));
    await waitFor(() => expect(second.result.current.error).not.toBeNull());
  });
});

describe("a page rendered without a window", () => {
  it("is not a phone on the server", () => {
    function Probe() {
      return <p>{useIsPhone() ? "phone" : "wide"}</p>;
    }
    expect(renderToString(<Probe />)).toContain("wide");
  });
});

describe("access words", () => {
  it("puts a source registration operation in its own area", () => {
    expect(areaOf("createCSR")).toBe("registrations");
    expect(areaOf("retrieveCSourceRegistration")).toBe("registrations");
  });

  it("says a role with no rules holds nothing", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <RulesInWords rules={[]} />
      </I18nextProvider>,
    );
    expect(screen.getByText(en.access.words.nothing)).toBeInTheDocument();
  });
});

describe("panel specs", () => {
  it("colours a series by the fixed colour its field config names", () => {
    const series: Series[] = [{ name: "a", field: { name: "a", type: "number", values: [1] }, points: [{ t: 1, v: 1 }] }];
    expect(seriesColors(series, { defaults: { color: { mode: "fixed", fixedColor: "#112233" } }, overrides: [] })).toEqual(["#112233"]);
  });

  it("skips a frame with no time column in a state timeline", () => {
    const frames: Frame[] = [{ fields: [{ name: "state", type: "string", values: ["ok"] }] }];
    const built = stateTimelineSpec(frames, { options: {}, locale: "en", theme: chartTheme("light"), height: 100, from: 0, to: 10 });
    expect(built.states).toEqual([]);
  });
});

describe("representation outputs", () => {
  it("starts a widget over a type with a name of its own, and drops an emptied text", () => {
    expect(newOutput("widget", ["mapa"], "Place")).toEqual({ name: "mapa-2", sources: [{ type: "Place" }] });
    expect(withText({ en: "Hello" }, "en", "   ")).toBeUndefined();
    expect(withText({ en: "Hello", sk: "Ahoj" }, "en", "")).toEqual({ sk: "Ahoj" });
  });
});

describe("what a record batch changes", () => {
  it("skips anything that is not a record", () => {
    expect(changesOf([null, "text", ["list"], 3], new Map())).toEqual([]);
  });
});

describe("the document title", () => {
  it("tells a listener when the title is forgotten", () => {
    const listener = vi.fn();
    const stop = onPageTitle(listener);
    resetDocumentTitle();
    expect(listener).toHaveBeenCalledTimes(1);
    stop();
  });
});

describe("map colours", () => {
  it("answers the fallback where there is no document", () => {
    vi.stubGlobal("document", undefined);
    expect(plainColourOf("color-mix(in srgb, red, blue)", "rgb(1, 2, 3)")).toBe("rgb(1, 2, 3)");
  });
});

describe("a history value typed by a person", () => {
  it("reads false as a boolean, and nothing else as one", () => {
    expect(valueOf("False", true)).toBe(false);
    expect(valueOf("false", undefined)).toBe(false);
    expect(valueOf("maybe", true)).toBeUndefined();
  });
});

describe("a paging guess in words", () => {
  it("says a paging whose end is unknown walks the pages", () => {
    expect(pagingWords({ paging: {}, more: false })).toMatchObject({ key: "walk" });
    expect(pagingWords({ paging: {}, more: true })).toMatchObject({ key: "more" });
  });
});

describe("the assistant's note", () => {
  it("leaves no description behind on a field pointed at before when the note moves", async () => {
    render(
      <>
        <input id="root_name" aria-label="Name" />
        <input id="root_title" aria-label="Title" />
      </>,
    );
    await runUiAction({ action: "highlight", field: "spec.name", message: "one" }, 0);
    await runUiAction({ action: "highlight", field: "spec.title", message: "two" }, 0);
    expect(screen.getByLabelText("Name").hasAttribute("aria-describedby")).toBe(false);
  });
});
