// T-7706: TestPublish.tsx: the broker watch switches itself off when its minute is over.
import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { MqttBrokerPage } from "../src/pages/mqttbrokers/MqttBrokerPage";
import { WATCH_MS } from "../src/pages/mqttbrokers/TestPublish";
import { json, renderPage } from "./page_contract";

const PROJECT = "espoo";
const BROKER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "MqttBroker",
  metadata: { name: "cameras", namespace: PROJECT },
  spec: { roles: [{ name: "cams", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/#"] }] },
  status: { phase: "Live" },
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the broker watch", () => {
  it("is off again after its minute, and the person can switch it on once more", async () => {
    renderPage(<MqttBrokerPage project={PROJECT} name="cameras" />, {
      path: `/projects/${PROJECT}/mqttbrokers/cameras`,
      answer: async (url) => {
        if (url.pathname.endsWith("/watch")) return json({ observedAt: "2026-10-24T10:00:05Z", refusals: [], refusalsRead: true, connected: 1 });
        return url.pathname.endsWith("/mqttbrokers/cameras") ? json(BROKER) : undefined;
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: en.mqttbrokers.test.open }));
    const dialog = await screen.findByRole("dialog");
    const timers: { fn: () => void; ms: number }[] = [];
    const real = window.setTimeout.bind(window);
    vi.spyOn(window, "setTimeout").mockImplementation(((fn: () => void, ms?: number, ...rest: unknown[]) => {
      timers.push({ fn, ms: ms ?? 0 });
      return real(fn, ms, ...rest);
    }) as typeof window.setTimeout);
    const watch = within(dialog).getByRole("switch", { name: en.mqttbrokers.test.watch });
    fireEvent.click(watch);
    expect(watch).toBeChecked();
    const minute = await waitFor(() => {
      const found = timers.find((timer) => timer.ms === WATCH_MS);
      expect(found).toBeDefined();
      return found as { fn: () => void; ms: number };
    });
    act(() => minute.fn());
    await waitFor(() => expect(within(dialog).getByRole("switch", { name: en.mqttbrokers.test.watch })).not.toBeChecked());
    fireEvent.click(within(dialog).getByRole("switch", { name: en.mqttbrokers.test.watch }));
    expect(within(dialog).getByRole("switch", { name: en.mqttbrokers.test.watch })).toBeChecked();
  });
});
