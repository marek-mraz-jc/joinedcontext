/**
 * T-1853: "Try it on a sample" against the UI contract (UI-01, UI-15, UI-16, UI-32, UI-44, UI-48).
 *
 * The panel hand-made its file input, and greyed "Test mapping" for two different reasons while
 * writing only one of them on the screen — as a sentence beside the button that nothing pointed
 * at. A person with no sample chosen saw a dead button and no explanation at all. The input is
 * the shared `FilePicker`, and the button carries whichever reason applies.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { expectDenied, expectNoRawKeys, expectNoViolations } from "./checks";
import { PipelineTest } from "../src/pages/pipelines/PipelineTest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { sentTo } from "./requests";

const MAPPED: PipelineForm = {
  class: "auto",
  compute: { kind: "bloblang", bloblang: 'root.id = "urn:ngsi-ld:X:a:b:1"' },
  output: { type: "AirQualityObserved", mode: "upsert" },
};

const UNMAPPED: PipelineForm = { class: "auto", output: { type: "AirQualityObserved", mode: "upsert" } };

const TRACE = {
  input: { events: 1, bytes: 20 },
  mapping: [{ id: "urn:ngsi-ld:X:a:b:1" }],
  validation: [{ index: 0, ok: true, problems: [] }],
  errors: [],
};

function show(draft: PipelineForm | undefined = MAPPED) {
  const onChange = vi.fn();
  const view = render(
    <I18nextProvider i18n={i18n}>
      <PipelineTest
        project="helsinki"
        draft={draft}
        onChange={onChange}
        toManifest={(form) => form}
      />
    </I18nextProvider>,
  );
  return { onChange, container: view.container, user: userEvent.setup() };
}

function csv(): File {
  return new File(["id,pm10\n1,21.5\n"], "air.csv", { type: "text/csv" });
}

const runButton = () => screen.getByRole("button", { name: en.pipelines.test.run });

describe("the pipeline sample test against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-1; Secure; path=/";
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("says a sample is missing on the button that is refused for it", () => {
    show(MAPPED);
    expectDenied(runButton(), en.pipelines.test.noSample);
  });

  it("says a mapping is missing once a sample is there", async () => {
    show(UNMAPPED);
    await userEvent.upload(screen.getByLabelText(en.pipelines.test.chooseFile), csv());
    expect(await screen.findByText(/air\.csv/)).toBeInTheDocument();
    expectDenied(runButton(), en.pipelines.test.noMapping);
  });

  it("runs the mapping once both are there, and never before", async () => {
    // The parameters are declared although the body ignores them: `mock.lastCall[0]` below is
    // typed from this signature, and `vi.fn(() => …)` would make the call tuple empty.
    const fetchMock = vi.fn((_input: RequestInfo | URL, _init?: RequestInit) =>
      Promise.resolve(
        new Response(JSON.stringify(TRACE), { status: 200, headers: { "Content-Type": "application/json" } }),
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    show(MAPPED);

    // Refused: the click does nothing and nothing is sent.
    await userEvent.click(runButton());
    expect(fetchMock).not.toHaveBeenCalled();

    await userEvent.upload(screen.getByLabelText(en.pipelines.test.chooseFile), csv());
    await screen.findByText(/air\.csv/);
    await userEvent.click(runButton());
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(sentTo(fetchMock, "/pipelines/test")).toHaveLength(1);
  });

  it("keeps a typed mapping when a sample lands, and replaces it only when asked (T-3971)", async () => {
    const { onChange } = show(MAPPED);
    await userEvent.upload(screen.getByLabelText(en.pipelines.test.chooseFile), csv());
    await screen.findByText(/air\.csv/);
    const kept = onChange.mock.lastCall?.[0] as PipelineForm;
    expect(kept.compute).toEqual(MAPPED.compute);
    expect(kept.output?.type).toBe("AirQualityObserved");
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.test.replaceDrafted }));
    const replaced = onChange.mock.lastCall?.[0] as PipelineForm;
    expect(replaced.compute?.bloblang).toContain("# drafted from air.csv");
    expect(replaced.output?.type).toBe("Air");
    expect(screen.queryByRole("button", { name: en.pipelines.test.replaceDrafted })).toBeNull();
  });

  it("offers no replacement when the compute node was empty and took the draft", async () => {
    const { onChange } = show(UNMAPPED);
    await userEvent.upload(screen.getByLabelText(en.pipelines.test.chooseFile), csv());
    await screen.findByText(/air\.csv/);
    expect((onChange.mock.lastCall?.[0] as PipelineForm).compute?.bloblang).toContain("# drafted from air.csv");
    expect(screen.queryByRole("button", { name: en.pipelines.test.replaceDrafted })).toBeNull();
  });

  it("puts the file input in the tab order with its own name and the shared ring", () => {
    show(MAPPED);
    const input = screen.getByLabelText(en.pipelines.test.chooseFile);
    input.focus();
    expect(input).toHaveFocus();
    expect(input.closest("label")).toHaveClass("focus-ring-within");
  });

  it("has no axe violation, with and without a sample", async () => {
    const { container } = show(MAPPED);
    await expectNoViolations(container);
    await userEvent.upload(screen.getByLabelText(en.pipelines.test.chooseFile), csv());
    await screen.findByText(/air\.csv/);
    await expectNoViolations(container);
  });

  it("shows no raw translation key in any locale the organisation offers", async () => {
    for (const locale of SUPPORTED_LOCALES) {
      await i18n.changeLanguage(locale);
      const { container, unmount } = render(
        <I18nextProvider i18n={i18n}>
          <PipelineTest project="helsinki" draft={MAPPED} onChange={vi.fn()} toManifest={(form) => form} />
        </I18nextProvider>,
      );
      expectNoRawKeys(container);
      unmount();
    }
    await i18n.changeLanguage("en");
  });
});
