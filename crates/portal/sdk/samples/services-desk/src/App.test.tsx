import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import type { Fixture } from "@joinedcontext/sdk/testing";
import { requireRole } from "@joinedcontext/sdk/server";
import App, { folderOf, keyOf } from "./App";
import { FUNCTIONS, ROWS, SERVICES, USER } from "./fixtures";

type Services = NonNullable<Fixture["services"]>;

const off = (layer: string) => ({
  status: 403,
  body: { type: "https://joinedcontext.com/errors/service-off", title: "Service Off", status: 403, layer },
});
const spent = (quota: string, resetAt: string) => ({
  status: 429,
  body: { type: "https://joinedcontext.com/errors/quota", title: "Quota Used Up", status: 429, quota, resetAt },
});

function renderDesk({ services = {}, roles = ["steward"] }: { services?: Services; roles?: string[] } = {}) {
  const client = stubClient(
    {
      entities: ROWS,
      services: { ...SERVICES, ...services },
      functions: FUNCTIONS,
      // the function's own check, on the user the stub was given
      refuse: (request) => {
        if (!request.path.startsWith("/functions/")) return null;
        const refused = requireRole({ user: { ...USER, roles } }, "steward");
        return refused ? { status: refused.status ?? 403, body: refused.body } : null;
      },
    },
    { user: { ...USER, roles } },
  );
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

const report = () => screen.findByRole("heading", { name: "Loose kerb stone at Hakaniemi market" });
const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));

describe("the services desk", () => {
  it("keys files under the report's own folder, whatever the id or file name holds", () => {
    expect(folderOf(ROWS[0])).toBe("reports/d1/");
    expect(folderOf({ id: "urn:ngsi-ld:DefectReport:x:../../a b", type: "DefectReport" })).toBe("reports/..-..-a-b/");
    expect(keyOf("my photo/1.jpg")).toBe("my-photo-1.jpg");
    expect(keyOf("")).toBe("photo");
    expect(keyOf("x".repeat(300))).toHaveLength(100);
  });

  it("lists a report's photos and stores a new one under its folder", async () => {
    const client = renderDesk();
    await report();
    const photos = screen.getByRole("region", { name: "Photos" });
    await within(photos).findByText(/kerb-north\.jpg · 179 KiB/);
    const file = new File(["jpeg"], "night shot.jpg", { type: "image/jpeg" });
    fireEvent.change(within(photos).getByLabelText("Add a photo"), { target: { files: [file] } });
    expect(await within(photos).findByRole("status")).toHaveTextContent("night shot.jpg is stored.");
    const put = client.transport.calls.find((call) => call.method === "PUT");
    expect(put?.path).toBe("/api/services/files/reports/d1/night-shot.jpg");
    expect(client.transport.calls.some((call) => call.path === "/api/services/files?prefix=reports%2Fd1%2F")).toBe(true);
  });

  it("rates the report with the model under a schema, emails it to the person and closes it", async () => {
    const client = renderDesk();
    await report();
    click("Rate severity");
    expect(await screen.findByText("Severity 4 of 5, as the model rates it.")).toBeInTheDocument();
    const asked = client.transport.calls.find((call) => call.path.endsWith("/services/ai/complete"));
    expect(asked?.body).toMatchObject({ maxTokens: 20, schema: { required: ["severity"] } });

    click("Email me this report");
    expect(await screen.findByText("Sent to your address.")).toBeInTheDocument();
    const mail = client.transport.calls.find((call) => call.path.endsWith("/services/email/send"));
    expect(mail?.body).toMatchObject({ to: "me", subject: "Defect report: Loose kerb stone at Hakaniemi market" });

    click("Close report");
    expect(await screen.findByText("Closed.")).toBeInTheDocument();
    expect(client.transport.calls.find((call) => call.path === "/functions/close-report")?.body).toEqual({ id: ROWS[0].id });
  });

  it("shows the App's jobs with their next run and a failed last run", async () => {
    renderDesk();
    expect(await screen.findByText("nightly-digest")).toBeInTheDocument();
    expect(screen.getByText(/last failed .*: files quota reached/)).toBeInTheDocument();
  });

  it("names the layer that switched each service off", async () => {
    renderDesk({
      services: { files: () => off("organization"), ai: () => off("project"), email: () => off("app"), jobs: () => off("project") },
    });
    await report();
    expect(await within(screen.getByRole("region", { name: "Photos" })).findByRole("alert")).toHaveTextContent(
      "Files is switched off for this organization.",
    );
    click("Rate severity");
    expect(await screen.findByText("The model is switched off for this project.")).toBeInTheDocument();
    click("Email me this report");
    expect(await screen.findByText("Email is switched off for this App.")).toBeInTheDocument();
    expect(await screen.findByText("Scheduled jobs is switched off for this project.")).toBeInTheDocument();
  });

  it("names the quota that is used up and when it comes back", async () => {
    renderDesk({ services: { ai: () => spent("ai.tokensPerDay", "2026-09-25T00:00:00Z"), files: () => spent("files.mib", "") } });
    await report();
    expect(await screen.findByText("Files: the files.mib quota is used up.")).toBeInTheDocument();
    click("Rate severity");
    expect(await screen.findByText("The model: the ai.tokensPerDay quota is used up until 2026-09-25T00:00:00Z.")).toBeInTheDocument();
  });

  it("shows the function's role refusal to a person without the steward role, and the report stays open", async () => {
    const client = renderDesk({ roles: ["clerk"] });
    await report();
    click("Close report");
    expect(await screen.findByRole("alert")).toHaveTextContent("this needs the App's role 'steward'");
    expect(client.transport.rows().find((row) => row.id === ROWS[0].id)?.status).toBe("open");
  });

  it("shows a plain problem for an answer that is no refusal, and keeps the button usable", async () => {
    renderDesk({ services: { email: () => ({ status: 502, body: { title: "Bad Gateway", detail: "the mail relay did not answer" } }) } });
    await report();
    click("Email me this report");
    expect(await screen.findByText("the mail relay did not answer")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "Email me this report" })).toBeEnabled());
  });
});
