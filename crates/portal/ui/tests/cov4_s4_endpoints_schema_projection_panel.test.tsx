// T-7706: the projection panel when the schema cannot be read (Retry for the index and for the artifact), a refused submit, and a hidden chip that takes its attribute back.
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SchemaProjectionPanel } from "../src/pages/endpoints/SchemaProjectionPanel";

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const INDEX = { endpoint: SLUG, models: [{ name: "air", version: 2, semver: "2.1.0", types: ["Air"] }] };
const SCHEMA = { $defs: { Air: { properties: { pm10: { type: "number" } } } } };

interface Stub {
  index: number[];
  artifact: number[];
}

function mount(stub: Stub, props: { hidden?: string[]; disabledReason?: string; onHiddenChange?: (names: string[]) => void } = {}) {
  const calls = { index: 0, artifact: 0 };
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
      const reply = (statuses: number[], at: number, body: unknown) => {
        const status = statuses[Math.min(at, statuses.length - 1)];
        return Promise.resolve(
          new Response(JSON.stringify(status === 200 ? body : { title: "Bad Gateway", status }), {
            status,
            headers: { "Content-Type": status === 200 ? "application/json" : "application/problem+json" },
          }),
        );
      };
      if (url.pathname.endsWith("/schema/index.json")) return reply(stub.index, calls.index++, INDEX);
      if (url.pathname.endsWith("/json-schema")) return reply(stub.artifact, calls.artifact++, SCHEMA);
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );
  const onHiddenChange = props.onHiddenChange ?? vi.fn();
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaProjectionPanel slug={SLUG} hidden={props.hidden ?? []} onHiddenChange={onHiddenChange} disabledReason={props.disabledReason} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { calls, onHiddenChange };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the projection panel when the schema cannot be read", () => {
  it("offers to read the schema index again, and the published attributes appear when it answers", async () => {
    const { calls } = mount({ index: [502, 200], artifact: [200] });
    await userEvent.click(await screen.findByRole("button", { name: en.form.listRetry }));
    expect((await screen.findAllByText("pm10")).length).toBeGreaterThan(0);
    expect(calls.index).toBe(2);
  });

  it("offers to read the schema itself again when only that read failed", async () => {
    const { calls } = mount({ index: [200], artifact: [502, 200] });
    await userEvent.click(await screen.findByRole("button", { name: en.form.listRetry }));
    expect((await screen.findAllByText("pm10")).length).toBeGreaterThan(0);
    expect(calls.artifact).toBe(2);
  });
});

describe("hiding attributes", () => {
  it("adds nothing when the person may not change the projection, even by pressing Enter in the field", async () => {
    const { onHiddenChange } = mount({ index: [200], artifact: [200] }, { disabledReason: "You may only read this endpoint" });
    const field = await screen.findByLabelText(en.endpoints.projection.addHidden);
    await userEvent.type(field, "pm25");
    fireEvent.submit(field.closest("form") as HTMLFormElement);
    expect(onHiddenChange).not.toHaveBeenCalled();
    expect(screen.queryByText(en.endpoints.projection.addShape)).toBeNull();
  });

  it("takes a hidden attribute back when its chip is pressed", async () => {
    const { onHiddenChange } = mount({ index: [200], artifact: [200] }, { hidden: ["calibrationOffset", "pm25"] });
    await userEvent.click(await screen.findByRole("button", { name: new RegExp(`^calibrationOffset.*${en.endpoints.projection.unhide}`) }));
    await waitFor(() => expect(onHiddenChange).toHaveBeenCalledWith(["pm25"]));
  });
});
