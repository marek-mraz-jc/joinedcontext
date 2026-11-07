import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AccessProvider } from "../access";
import { memory, STEWARD, STOP } from "../fixtures/memory";
import { ServerProblem } from "../plans";
import type { Me } from "../plans";
import { problemText, SavedPlans } from "./SavedPlans";

const went: string[] = [];
vi.mock("../go", () => ({ go: (url: string) => went.push(url) }));

const CURRENT = { vanCapacity: 12, start: null, include: ["a"], exclude: ["b"] };

beforeEach(() => window.history.replaceState(null, "", "/"));
afterEach(() => window.history.replaceState(null, "", "/"));

// T-3346: a plan saved on the server comes back after a reload, for the van it was saved for.
describe("SavedPlans", () => {
  it("asks for the van or crew before it saves, then saves the plan on screen and lists it", async () => {
    const api = memory();
    render(<SavedPlans lang="en" api={api} current={CURRENT} onOpen={vi.fn()} />);
    expect(await screen.findByText("No plan is saved yet.")).toBeInTheDocument();
    const save = screen.getByRole("button", { name: "Save this plan" });
    expect(save).toBeDisabled();
    expect(screen.getByText("Name the van or crew the plan is for first.")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Van or crew"), { target: { value: "Van 2" } });
    fireEvent.click(save);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Plan saved: 1 stops for Van 2"));
    expect(api.save).toHaveBeenCalledWith({ ...CURRENT, operator: "Van 2" });
    const list = screen.getByRole("list", { name: "Saved plans" });
    expect(within(list).getByText(/^Van 2 ·/)).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("op")).toBe("Van 2");
  });

  it("opens a saved plan's choices, records a drive and deletes only after a second click", async () => {
    const api = memory();
    await api.save({ ...CURRENT, operator: "Van 2" });
    window.history.replaceState(null, "", "/?op=Van%202");
    const onOpen = vi.fn();
    render(<SavedPlans lang="en" api={api} current={CURRENT} onOpen={onOpen} />);
    const row = (await screen.findByText(/^Van 2 ·/)).closest("li") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: /^Open:/ }));
    await waitFor(() => expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ id: 1, include: ["a"], exclude: ["b"] })));
    fireEvent.click(within(row).getByRole("button", { name: /^Mark as driven:/ }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The drive is recorded."));
    expect(api.drive).toHaveBeenCalledWith(1, [STOP.id]);
    expect(await screen.findByText(/driven 1 times/)).toBeInTheDocument();

    fireEvent.click(within(screen.getByText(/^Van 2 ·/).closest("li") as HTMLElement).getByRole("button", { name: /^Route sheet \(CSV\):/ }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The route sheet is downloading."));
    expect(went).toEqual(["https://store.example/apps/s1/x/sheets/1.csv?X-Amz-Signature=x"]);

    const del = () => within(screen.getByText(/^Van 2 ·/).closest("li") as HTMLElement).getByRole("button", { name: /^Delete:/ });
    fireEvent.click(del());
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("button", { name: "Delete for good" })).toBeNull();
    fireEvent.click(del());
    expect(api.remove).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Delete for good" }));
    await waitFor(() => expect(screen.getByText("No plan is saved for Van 2.")).toBeInTheDocument());
    expect(api.saved).toHaveLength(0);
  });

  it("downloads a plan's route sheet and keeps the plan when the delete is cancelled", async () => {
    const api = memory();
    await api.save({ ...CURRENT, operator: "Van 2" });
    window.history.replaceState(null, "", "/?op=Van%202");
    render(<SavedPlans lang="en" api={api} current={CURRENT} onOpen={vi.fn()} />);
    const row = (await screen.findByText(/^Van 2 ·/)).closest("li") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: /^Route sheet \(CSV\):/ }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The route sheet is downloading."));
    expect(api.sheetUrl).toHaveBeenCalledWith(1);

    fireEvent.click(within(row).getByRole("button", { name: /^Delete:/ }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("button", { name: "Delete for good" })).not.toBeInTheDocument();
    expect(within(row).getByRole("button", { name: /^Delete:/ })).toBeInTheDocument();
    expect(api.remove).not.toHaveBeenCalled();
    expect(api.saved).toHaveLength(1);
  });

  it("says in the operator's language why the server refused", async () => {
    const api = memory();
    api.save = vi.fn(async () => {
      throw new ServerProblem(400, "the van carries 1 to 200 bikes");
    });
    render(<SavedPlans lang="fi" api={api} current={CURRENT} onOpen={vi.fn()} />);
    fireEvent.change(await screen.findByLabelText("Auto tai tiimi"), { target: { value: "Auto 1" } });
    fireEvent.click(screen.getByRole("button", { name: "Tallenna suunnitelma" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Ei tallennettu: the van carries 1 to 200 bikes"));
  });

  it("maps every refusal to words a person can act on", () => {
    expect(problemText("en", new ServerProblem(0, ""), "saveFailed")).toMatch(/could not be reached/);
    expect(problemText("en", new ServerProblem(401, ""), "saveFailed")).toMatch(/Sign in again/);
    expect(problemText("en", new ServerProblem(403, ""), "saveFailed")).toMatch(/may not do this/);
    expect(problemText("en", new ServerProblem(404, ""), "openFailed")).toMatch(/no longer there/);
    expect(problemText("en", new ServerProblem(507, ""), "saveFailed")).toMatch(/storage is full/);
    expect(problemText("en", new ServerProblem(502, ""), "saveFailed")).toMatch(/did not answer/);
    expect(problemText("en", new Error("x"), "saveFailed")).toMatch(/did not answer/);
  });

  it("hands a plan to another van at once for a steward, and says who now has it", async () => {
    const api = memory(STEWARD);
    await api.save({ ...CURRENT, operator: "Van 2" });
    renderAs(STEWARD, api);
    const row = (await screen.findByText(/^Van 2 ·/)).closest("li") as HTMLElement;
    fireEvent.click(await within(row).findByRole("button", { name: /^Hand over:/ }));
    const form = screen.getByRole("form", { name: "Hand over: Van 2" });
    fireEvent.change(within(form).getByLabelText("New van or crew"), { target: { value: " Van 7 " } });
    fireEvent.click(within(form).getByRole("button", { name: "Hand over" }));
    expect(await screen.findByText(/^Van 7 ·/)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The plan is handed to Van 7."));
    expect(api.handOver).toHaveBeenCalledWith(1, "Van 7");
    expect(api.saved[0].operator).toBe("Van 7");
  });

  it("puts the van back and says why when the server refuses the hand-over", async () => {
    const api = memory(STEWARD);
    await api.save({ ...CURRENT, operator: "Van 2" });
    api.handOver = vi.fn(async () => {
      throw new ServerProblem(403, "this needs the role steward");
    });
    renderAs(STEWARD, api);
    const row = (await screen.findByText(/^Van 2 ·/)).closest("li") as HTMLElement;
    fireEvent.click(await within(row).findByRole("button", { name: /^Hand over:/ }));
    fireEvent.change(screen.getByLabelText("New van or crew"), { target: { value: "Van 7" } });
    fireEvent.click(screen.getByRole("button", { name: "Hand over" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Your account may not do this."));
    expect(screen.getByText(/^Van 2 ·/)).toBeInTheDocument();
    expect(screen.queryByText(/^Van 7 ·/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByLabelText("New van or crew")).toBeNull();
  });

  it("marks a plan someone else saved, which a steward sees beside their own", async () => {
    const api = memory(STEWARD);
    await api.save({ ...CURRENT, operator: "Van 2" });
    api.saved[0].owner = "u-other";
    renderAs(STEWARD, api);
    expect(await screen.findByText(/saved by someone else/)).toBeInTheDocument();
  });

  it("closes the hand-over to a viewer and says who may", async () => {
    const api = memory();
    await api.save({ ...CURRENT, operator: "Van 2" });
    renderAs({ id: "u-viewer", name: "V", roles: ["viewer"] }, api);
    const row = (await screen.findByText(/^Van 2 ·/)).closest("li") as HTMLElement;
    const hand = within(row).getByRole("button", { name: /^Hand over:/ });
    expect(hand).toBeDisabled();
    expect(hand).toHaveAttribute("title", "Only a steward hands a plan to another van or crew.");
    expect(hand).toHaveAccessibleDescription("Only a steward hands a plan to another van or crew.");
  });

  it("offers no save to a reader who is signed out, and says why", async () => {
    const api = memory(null);
    renderAs(null, api);
    fireEvent.change(await screen.findByLabelText("Van or crew"), { target: { value: "Van 2" } });
    const save = screen.getByRole("button", { name: "Save this plan" });
    await waitFor(() => expect(save).toHaveAccessibleDescription("Sign in to save plans."));
    expect(save).toBeDisabled();
    fireEvent.submit(save.closest("form") as HTMLFormElement);
    expect(api.save).not.toHaveBeenCalled();
  });
});

function renderAs(me: Me | null, api: ReturnType<typeof memory>) {
  api.me = vi.fn(async () => me);
  render(
    <AccessProvider api={api}>
      <SavedPlans lang="en" api={api} current={CURRENT} onOpen={vi.fn()} />
    </AccessProvider>,
  );
}
