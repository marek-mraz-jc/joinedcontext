import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { AccessDocument, JcUser } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { RESIDENT_ACCESS, ROWS, SCHEMA, USER } from "./fixtures";

function renderForm({ user = USER, access = RESIDENT_ACCESS, refuseCreate = false }: { user?: JcUser | null; access?: AccessDocument; refuseCreate?: boolean } = {}) {
  const client = stubClient(
    {
      entities: ROWS,
      schema: SCHEMA,
      access,
      refuse: (request) =>
        refuseCreate && request.method === "POST" ? { status: 403, body: { title: "Forbidden", detail: "requests are closed for the night" } } : null,
    },
    { user },
  );
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

const fill = (label: string | RegExp, value: string) => fireEvent.change(screen.getByLabelText(label), { target: { value } });
const next = () => fireEvent.click(screen.getByRole("button", { name: "Next" }));

async function toReview() {
  await screen.findByRole("heading", { name: /Step 1 of 4/ });
  fill("Kind of problem", "pothole");
  fill("Title", "Pothole at the tram stop");
  fill("What is wrong", "A deep hole in the asphalt since Monday.");
  next();
  fill("Street address", "Hämeentie 5");
  next();
  next();
  await screen.findByRole("heading", { name: /Step 4 of 4/ });
}

describe("the form-first workflow", () => {
  it("names what is missing in a summary that links to each input", async () => {
    renderForm();
    await screen.findByRole("heading", { name: /Step 1 of 4: What is the problem/ });
    next();
    const summary = screen.getByRole("alert");
    expect(within(summary).getAllByRole("link").map((link) => link.textContent)).toEqual([
      "Kind of problem is required",
      "Title is required",
      "What is wrong is required",
    ]);
    await waitFor(() => expect(summary).toHaveFocus());
    expect(screen.getByLabelText("Title")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText("Title")).toHaveAccessibleDescription(/5 to 80 characters.*Title is required/);
    fireEvent.click(within(summary).getByRole("link", { name: "Title is required" }));
    expect(screen.getByLabelText("Title")).toHaveFocus();

    fill("Title", "Hole");
    fill("Kind of problem", "pothole");
    fill("What is wrong", "A deep hole in the asphalt since Monday.");
    next();
    expect(within(screen.getByRole("alert")).getByRole("link")).toHaveTextContent("Title does not match the expected format");
  });

  it("walks four steps, keeps the answers on the way back, and sends one request as the person", async () => {
    const client = renderForm();
    await toReview();
    expect(screen.getByText("Pothole at the tram stop")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Change Title" }));
    expect(screen.getByRole("heading", { name: /Step 1 of 4/ })).toHaveFocus();
    expect(screen.getByLabelText("Title")).toHaveValue("Pothole at the tram stop");
    next();
    next();
    next();

    fireEvent.click(await screen.findByRole("button", { name: "Send request" }));
    expect(await screen.findByRole("status")).toHaveTextContent(/Sent\. Your reference is [A-Za-z0-9-]+/);
    const post = client.transport.calls.find((call) => call.method === "POST");
    expect(post?.body).toMatchObject({
      type: "ServiceRequest",
      category: { type: "Property", value: "pothole" },
      address: { type: "Property", value: "Hämeentie 5" },
      submittedBy: { type: "Property", value: USER.id },
      status: { type: "Property", value: "received" },
    });
    expect(post?.body).not.toHaveProperty("district");
    expect(screen.getByLabelText("Title")).toHaveValue("");
    await waitFor(() => expect(within(screen.getByRole("article", { name: "Your requests" })).getAllByRole("listitem")).toHaveLength(ROWS.length + 1));
  });

  it("asks only for the person's own requests, newest first", async () => {
    const client = renderForm();
    const list = await screen.findByRole("article", { name: "Your requests" });
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(3));
    expect(within(list).getAllByRole("listitem")[0]).toHaveTextContent("Overflowing bin in Karhupuisto");
    const read = client.transport.calls.find((call) => call.method === "GET" && call.path.includes("type=ServiceRequest"));
    expect(new URL(read!.path, "http://x").searchParams.get("q")).toBe(`submittedBy=="${USER.id}"`);
  });

  it("keeps Send disabled with the reason for a person who is not signed in", async () => {
    renderForm({ user: null });
    await toReview();
    expect(screen.getByRole("note")).toHaveTextContent("Sign in to send a request");
    expect(screen.getByRole("button", { name: "Send request" })).toBeDisabled();
    expect(screen.getByText("Sign in to see the requests you sent.")).toBeInTheDocument();
  });

  it("keeps Send disabled with the endpoint's reason for a person who may only read", async () => {
    renderForm({ access: { permissions: [{ resource: { type: "ServiceRequest" }, actions: ["queryEntity"], attributes: "*" }], prohibitions: [] } });
    await toReview();
    await waitFor(() => expect(screen.getByRole("button", { name: "Send request" })).toBeDisabled());
    expect(screen.getByRole("note")).toHaveTextContent(/createEntity/);
  });

  it("shows the refusal and keeps every answer when the endpoint says no", async () => {
    renderForm({ refuseCreate: true });
    await toReview();
    fireEvent.click(screen.getByRole("button", { name: "Send request" }));
    expect(await screen.findByText(/requests are closed for the night/)).toBeInTheDocument();
    expect(screen.getByText("Pothole at the tram stop")).toBeInTheDocument();
  });
});
