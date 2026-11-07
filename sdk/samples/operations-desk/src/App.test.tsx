import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import type { AccessDocument } from "@joinedcontext/sdk";
import App, { queueOrder } from "./App";
import { EDITOR_ACCESS, ROWS, VIEWER_ACCESS } from "./fixtures";

function renderDesk(access: AccessDocument, refuse?: (path: string) => boolean) {
  const client = stubClient({
    entities: ROWS,
    access,
    refuse: (request) =>
      refuse?.(request.path) ? { status: 403, body: { title: "Forbidden", detail: "the status of this alert is locked" } } : null,
  });
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

function bodyRows(): HTMLElement[] {
  return within(screen.getByRole("table", { name: "Alerts" })).getAllByRole("row").slice(1);
}

describe("the operations desk", () => {
  it("orders the queue by severity, then newest first, with an unknown severity last", () => {
    const order = queueOrder([...ROWS, { id: "urn:x", type: "Alert", name: "Unrated" }]).map((row) => row.name);
    expect(order.slice(0, 3)).toEqual(["Water main burst on Mannerheimintie", "Power cut in three blocks", "Traffic lights out at Hakaniemi"]);
    expect(order.at(-1)).toBe("Unrated");
  });

  it("narrows the table by a filter and says how many of how many it shows", async () => {
    renderDesk(VIEWER_ACCESS);
    await screen.findByRole("table", { name: "Alerts" });
    expect(bodyRows()).toHaveLength(ROWS.length);
    fireEvent.change(screen.getByRole("combobox", { name: "Severity" }), { target: { value: "critical" } });
    expect(bodyRows()).toHaveLength(2);
    expect(screen.getByText(`2 of ${ROWS.length}`)).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "nothing like this" } });
    expect(screen.getByText("No alert matches these filters.")).toBeInTheDocument();
  });

  it("keeps the bulk actions disabled with the endpoint's reason for a person who may not change status", async () => {
    renderDesk(VIEWER_ACCESS);
    await screen.findByRole("table", { name: "Alerts" });
    await waitFor(() => expect(screen.getByRole("note")).toHaveTextContent(/may not updateAttrs Alert/));
    expect(screen.getByRole("button", { name: /Acknowledge/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Resolve/ })).toBeDisabled();
  });

  it("acknowledges the selected alerts, one request each, and says what changed", async () => {
    const client = renderDesk(EDITOR_ACCESS);
    await screen.findByRole("table", { name: "Alerts" });
    const acknowledge = screen.getByRole("button", { name: /Acknowledge/ });
    await waitFor(() => expect(acknowledge).toHaveAttribute("title", "Select one or more alerts first."));

    fireEvent.click(screen.getByRole("checkbox", { name: "Select Water main burst on Mannerheimintie" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Street lighting fault" }));
    expect(screen.getByRole("button", { name: "Acknowledge 2" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Acknowledge 2" }));

    expect(await screen.findByRole("status")).toHaveTextContent("2 alerts changed.");
    const patches = client.transport.calls.filter((call) => call.method === "PATCH");
    const acknowledged = { status: { type: "Property", value: "acknowledged" } };
    expect(patches.map((call) => call.body)).toEqual([acknowledged, acknowledged]);
    expect(patches.map((call) => call.path)).toEqual([
      expect.stringContaining("urn%3Angsi-ld%3AAlert%3Ahel.fi%3Aalerts%3Aa1"),
      expect.stringContaining("urn%3Angsi-ld%3AAlert%3Ahel.fi%3Aalerts%3Aa4"),
    ]);
  });

  it("names the alerts a refusal left unchanged", async () => {
    renderDesk(EDITOR_ACCESS, (path) => path.includes("a2") && path.endsWith("/attrs"));
    await screen.findByRole("table", { name: "Alerts" });
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Traffic lights out at Hakaniemi" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Fallen tree on cycle path" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Resolve 2" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Resolve 2" }));
    expect(await screen.findByRole("status")).toHaveTextContent("1 alert changed. Not changed: Traffic lights out at Hakaniemi.");
  });

  it("reads the chosen alert beside the table", async () => {
    renderDesk(VIEWER_ACCESS);
    await screen.findByRole("table", { name: "Alerts" });
    expect(screen.getByText("Choose an alert to read it here.")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Flooded underpass"));
    expect(screen.getByRole("heading", { name: "Flooded underpass" })).toBeInTheDocument();
  });
});
