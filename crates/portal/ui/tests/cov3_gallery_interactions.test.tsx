// T-7589: the component gallery as a reader uses it: tick the checkbox, open a row's menu, show a
// hidden grid column, retry the failed read, confirm a removal, choose and drop files.
import { cleanup, createEvent, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { Gallery } from "../src/pages/gallery/Gallery";
import { json, renderPage } from "./page_contract";

function show() {
  return renderPage(<Gallery />, {
    answer: (url) =>
      url.pathname.endsWith("/permissions/me")
        ? json({ grants: [{ rule: { kinds: ["Endpoint"], verbs: ["propose"] } }], bootstrap: false })
        : undefined,
    path: "/__gallery",
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => cleanup());

describe("the component gallery, used", () => {
  it("ticks and unticks the publish checkbox", async () => {
    const user = userEvent.setup();
    show();
    const box = await screen.findByRole("checkbox", { name: /Publish this endpoint/ });
    expect(box).toBeChecked();
    await user.click(box);
    expect(box).not.toBeChecked();
    await user.click(box);
    expect(box).toBeChecked();
  });

  it("opens a row's menu and picks Edit, leaving the refused Remove unselectable", async () => {
    const user = userEvent.setup();
    show();
    const more = (await screen.findAllByRole("button", { name: /^More actions for/ }))[0];
    await user.click(more);
    const remove = await screen.findByRole("menuitem", { name: /Remove/ });
    expect(remove).toHaveAttribute("aria-disabled", "true");
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    await waitFor(() => expect(screen.queryByRole("menuitem", { name: "Edit" })).toBeNull());
  });

  it("shows the hidden Context Space column of the endpoints grid once it is ticked in Arrange columns", async () => {
    const user = userEvent.setup();
    show();
    await screen.findByRole("table", { name: "Endpoints of the project" });
    const arrange = screen.getAllByRole("button", { name: en.entityGrid.arrangeColumns })[0];
    await user.click(arrange);
    const panel = within(await screen.findByRole("dialog", { name: en.entityGrid.arrangeColumns }));
    await user.click(panel.getByRole("checkbox", { name: `${en.entityGrid.showColumn}: Context Space` }));
    await waitFor(() => expect(screen.getAllByRole("columnheader", { name: /Context Space/ }).length).toBeGreaterThan(0));
  });

  it("retries a failed read from the failure panel", async () => {
    const user = userEvent.setup();
    show();
    const retry = await screen.findAllByRole("button", { name: en.app.error.retry });
    await user.click(retry[0]);
    expect(screen.getAllByRole("button", { name: en.app.error.retry }).length).toBeGreaterThan(0);
  });

  it("asks before removing the endpoint and closes once the removal is proposed", async () => {
    const user = userEvent.setup();
    show();
    await user.click(await screen.findByRole("button", { name: "Remove the endpoint" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove air-quality?" });
    await user.click(within(dialog).getByRole("button", { name: "Propose removal" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("takes a chosen file and dropped files without error", async () => {
    const user = userEvent.setup();
    show();
    const picker = await screen.findByLabelText("Choose a model file");
    await user.upload(picker, new File(["a: 1"], "model.yaml", { type: "text/yaml" }));
    const zone = screen.getByTestId("file-drop-zone");
    const drop = createEvent.drop(zone);
    Object.defineProperty(drop, "dataTransfer", { value: { files: [new File(["x"], "a.csv"), new File(["y"], "b.json")] } });
    fireEvent(zone, drop);
    const chosen = await screen.findByLabelText("Files to read");
    await user.upload(chosen, [new File(["x"], "a.csv"), new File(["y"], "b.json")]);
    expect(screen.getByRole("heading", { level: 1, name: "Component gallery" })).toBeInTheDocument();
  });
});
