import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { AttributeHistory, CsvNotes, TimelinePanel, useWrite } from "../src/components/entities/history/AttributeHistory";
import { MAX_CSV_ROWS } from "../src/components/entities/history/historyApi";
import type { CsvProblem } from "../src/components/entities/history/historyApi";

const SEEN: Request[] = [];
const VERSIONS = [
  { type: "Property", value: 12, observedAt: "2026-10-18T07:00:00Z", instanceId: "urn:old", supersededAt: "2026-10-18T09:12:40Z", provenance: { channel: "pipeline", actor: "serviceAccount:pipeline-air", operation: "create", writtenAt: "2026-10-18T07:00:02Z" } },
  { type: "Property", value: 14, observedAt: "2026-10-18T07:00:00Z", instanceId: "urn:new", provenance: { channel: "ui", actor: "user:demo.steward@hel.fi", operation: "correct", reason: "Sensor checked on site", writtenAt: "2026-10-18T09:12:40Z" } },
];

function stub(read: { status: number; body?: unknown }, write = 204): void {
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const request = input as Request;
    SEEN.push(request);
    if (request.method === "GET") return new Response(JSON.stringify(read.body ?? {}), { status: read.status });
    return new Response(write === 204 ? null : JSON.stringify({ detail: "no grant" }), { status: write });
  });
}

function show(attributes = ["pm10"]) {
  render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <AttributeHistory slug="slug" entityId="urn:ngsi-ld:S:1" attributes={attributes} />
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

const panel = (name: string) => screen.getByRole("region", { name });
const writes = () => SEEN.filter((request) => request.method === "PATCH");

describe("AttributeHistory", () => {
  beforeEach(async () => {
    SEEN.length = 0;
    await i18n.changeLanguage("en");
  });
  afterEach(() => vi.restoreAllMocks());

  it("shows each version with the writer's name without its prefix, and the reason", async () => {
    stub({ status: 200, body: { pm10: VERSIONS } });
    show();
    expect(await screen.findByText(/Sensor checked on site/)).toBeTruthy();
    expect(screen.getByText(/demo\.steward@hel\.fi · ui correct/)).toBeTruthy();
    expect(screen.getByText(/^pipeline-air/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Revert" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Correct" })).toBeTruthy();
  });

  it("says when versions are withheld, when nothing is recorded, and offers a retry on a failed read", async () => {
    stub({ status: 404 });
    show();
    expect(await screen.findByText("No history yet")).toBeTruthy();
    vi.restoreAllMocks();
    document.body.innerHTML = "";
    stub({ status: 500, body: { detail: "broker down" } });
    show();
    expect(await screen.findByText(/broker down/)).toBeTruthy();
    expect(screen.getByRole("button", { name: i18n.t("app.error.retry") })).toBeTruthy();
  });

  it("asks for a reason before the current value is saved, and sends it", async () => {
    stub({ status: 200, body: { pm10: VERSIONS } });
    show();
    await screen.findByText(/Sensor checked on site/);
    const edit = panel("Edit current value");
    const save = within(edit).getByRole("button", { name: /Save/ });
    expect(save.getAttribute("aria-disabled") ?? save.hasAttribute("disabled")).toBeTruthy();
    await userEvent.clear(within(edit).getByRole("textbox", { name: /Value/ }));
    await userEvent.type(within(edit).getByRole("textbox", { name: /Value/ }), "15");
    await userEvent.type(within(edit).getByRole("textbox", { name: /Reason/ }), "Recalibrated");
    await userEvent.click(save);
    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0].headers.get("JC-Reason")).toBe("Recalibrated");
    expect((await writes()[0].json()).pm10.value).toBe(15);
  });

  it("adds an old value at the chosen instant and refuses a future one", async () => {
    stub({ status: 200, body: { pm10: VERSIONS } });
    show();
    await screen.findByText(/Sensor checked on site/);
    const old = panel("Add an old value");
    const when = within(old).getByLabelText(/Observed at/);
    await userEvent.type(when, "9999-01-01T00:00");
    expect(within(old).getByText(/not in the future/)).toBeTruthy();
    await userEvent.clear(when);
    await userEvent.type(when, "2026-03-03T14:00");
    await userEvent.type(within(old).getByRole("textbox", { name: /Value/ }), "9");
    await userEvent.type(within(old).getByRole("textbox", { name: /Reason/ }), "From the paper log");
    await userEvent.click(within(old).getByRole("button", { name: /Save/ }));
    await waitFor(() => expect(writes()).toHaveLength(1));
    expect((await writes()[0].json()).pm10).toMatchObject({ value: 9, observedAt: new Date(2026, 2, 3, 14, 0).toISOString() });
  });

  it("corrects the measurement inline at its own instant, and reverts to the older version", async () => {
    stub({ status: 200, body: { pm10: VERSIONS } });
    show();
    await screen.findByText(/Sensor checked on site/);
    await userEvent.click(screen.getByRole("button", { name: "Revert" }));
    const form = screen.getByRole("region", { name: /Revert to older version/ });
    expect(within(form).getByText(/Writes 12 again/)).toBeTruthy();
    await userEvent.type(within(form).getByRole("textbox", { name: /Reason/ }), "Wrong correction");
    await userEvent.click(within(form).getByRole("button", { name: /Revert/ }));
    await waitFor(() => expect(writes()).toHaveLength(1));
    expect((await writes()[0].json()).pm10).toMatchObject({ value: 12, observedAt: "2026-10-18T07:00:00Z" });
    await waitFor(() => expect(screen.queryByText(/Writes 12 again/)).toBeNull());
  });

  it("previews the counts of a CSV, sends nothing before the confirm, then imports", async () => {
    stub({ status: 200, body: { pm10: VERSIONS } });
    show();
    await screen.findByText(/Sensor checked on site/);
    const csv = panel("Import old values from CSV");
    const file = new File(["observedAt,value\n2026-10-18T07:00:00Z,14\n2026-10-18T07:00:00.500Z,6\n2026-03-03T14:00:00Z,many\n"], "old.csv", { type: "text/csv" });
    await userEvent.upload(within(csv).getByLabelText(/CSV file/), file);
    expect(await within(csv).findByText("New: 1 · Duplicates: 1 · Corrections: 0")).toBeTruthy();
    expect(within(csv).getByText(/Line 4/)).toBeTruthy();
    expect(writes()).toEqual([]);
    await userEvent.type(within(csv).getByRole("textbox", { name: /Reason/ }), "Paper log");
    await userEvent.click(within(csv).getByRole("button", { name: /Confirm import/ }));
    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(await within(csv).findByText("Values sent: 1")).toBeTruthy();
  });

  it("shows the gateway's refusal of a write", async () => {
    stub({ status: 200, body: { pm10: VERSIONS } }, 403);
    show();
    await screen.findByText(/Sensor checked on site/);
    const edit = panel("Edit current value");
    await userEvent.type(within(edit).getByRole("textbox", { name: /Reason/ }), "Why");
    await userEvent.click(within(edit).getByRole("button", { name: /Save/ }));
    expect(await within(edit).findByText(/no grant/)).toBeTruthy();
  });

  it("offers each writable attribute and reads the one chosen", async () => {
    stub({ status: 200, body: {} });
    show(["pm10", "no2"]);
    await userEvent.selectOptions(await screen.findByLabelText(/Attribute/), "no2");
    await waitFor(() => expect(SEEN.some((request) => request.url.includes("attrs=no2"))).toBe(true));
  });
});

describe("TimelinePanel", () => {
  const read = { isPending: false, isError: false, isSuccess: true, error: null, data: { versions: true }, refetch: vi.fn() };
  const panel = (props: Partial<Parameters<typeof TimelinePanel>[0]>) =>
    render(
      <I18nextProvider i18n={i18n}>
        <TimelinePanel history={read} measurements={[]} saved={false} open={() => undefined} {...props} />
      </I18nextProvider>,
    );

  beforeEach(() => i18n.changeLanguage("en"));

  it("offers paging only where it leads somewhere", async () => {
    const older = vi.fn();
    panel({ older });
    await userEvent.click(screen.getByRole("button", { name: "Older" }));
    expect(older).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "Newest" })).toBeNull();
  });

  it("confirms a save, shows the read in flight, and says when versions are withheld", () => {
    panel({ saved: true, history: { ...read, isPending: true, isSuccess: false, data: { versions: false } } });
    expect(screen.getByText("Saved. The change stands in the timeline.")).toBeTruthy();
    expect(screen.getByRole("status", { name: i18n.t("app.loading") })).toBeTruthy();
    expect(screen.getByText(/readVersions/)).toBeTruthy();
  });
});

describe("useWrite", () => {
  const wrapper = ({ children }: { children: ReactNode }) => <I18nextProvider i18n={i18n}>{children}</I18nextProvider>;
  const target = (written: () => Promise<void>) => ({ slug: "slug", entityId: "urn:ngsi-ld:S:1", attribute: "pm10", sample: 12, written });

  it("is busy while the write runs and reports the landed write to the page", async () => {
    const written = vi.fn(async () => undefined);
    let land = () => {};
    const { result } = renderHook(() => useWrite(target(written)), { wrapper });
    let done: Promise<boolean> = Promise.resolve(false);
    act(() => {
      done = result.current.run(() => new Promise((resolve) => (land = resolve)));
    });
    expect(result.current.busy).toBe(true);
    expect(written).not.toHaveBeenCalled();
    await act(async () => {
      land();
      expect(await done).toBe(true);
    });
    expect(written).toHaveBeenCalledTimes(1);
    expect(result.current).toMatchObject({ busy: false, error: null });
  });

  it("keeps the gateway's message on a failed write, tells the page nothing and clears on the next try", async () => {
    const written = vi.fn(async () => undefined);
    const { result } = renderHook(() => useWrite(target(written)), { wrapper });
    await act(async () => {
      expect(await result.current.run(() => Promise.reject(new Error("no grant")))).toBe(false);
    });
    expect(written).not.toHaveBeenCalled();
    expect(result.current).toMatchObject({ busy: false, error: "no grant" });
    await act(async () => {
      expect(await result.current.run(() => Promise.resolve())).toBe(true);
    });
    expect(result.current.error).toBeNull();
  });

  it("says something a person can read when the failure carries no message", async () => {
    const { result } = renderHook(() => useWrite(target(async () => undefined)), { wrapper });
    await act(async () => {
      await result.current.run(() => Promise.reject("gone"));
    });
    expect(result.current.error).toBe(i18n.t("app.error.generic"));
  });
});

describe("CsvNotes", () => {
  const notes = (problems: CsvProblem[], unchecked: number) =>
    render(
      <I18nextProvider i18n={i18n}>
        <CsvNotes problems={problems} unchecked={unchecked} />
      </I18nextProvider>,
    );

  it("says nothing about a clean file", () => {
    const { container } = notes([], 0);
    expect(container.textContent).toBe("");
  });

  it("counts the rows it could not check and names each refused line with its reason", () => {
    notes([{ line: 3, reason: "badInstant" }, { line: 9, reason: "tooManyRows" }], 2);
    expect(screen.getByText(i18n.t("attributeHistory.csvUnchecked", { count: 2 }))).toBeTruthy();
    const items = within(screen.getByRole("list")).getAllByRole("listitem").map((item) => item.textContent);
    expect(items).toEqual([
      i18n.t("attributeHistory.csvProblems", { line: 3, reason: i18n.t("attributeHistory.csvProblem.badInstant") }),
      i18n.t("attributeHistory.csvProblems", { line: 9, reason: i18n.t("attributeHistory.csvProblem.tooManyRows", { max: MAX_CSV_ROWS }) }),
    ]);
    expect(screen.getByText(i18n.t("attributeHistory.csvLeftOut"))).toBeTruthy();
  });
});
