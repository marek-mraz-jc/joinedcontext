import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "./App";
import type { Note, NotesApi } from "./notes";

function fake(start: Note[]): NotesApi & { notes: Note[] } {
  const notes = [...start];
  return {
    notes,
    list: vi.fn(async () => [...notes]),
    add: vi.fn(async (body: string) => {
      const note = { id: notes.length + 1, body, file: null, created_at: "" };
      notes.unshift(note);
      return note;
    }),
    change: vi.fn(async () => undefined),
    remove: vi.fn(async (id: number) => {
      notes.splice(notes.findIndex((n) => n.id === id), 1);
    }),
    upload: vi.fn(async () => undefined),
    downloadUrl: vi.fn(async () => "https://store.example/x"),
  };
}

describe("the notes screen", () => {
  it("lists the notes, adds one and deletes one", async () => {
    const api = fake([{ id: 1, body: "first", file: null, created_at: "" }]);
    render(<App api={api} />);
    const list = await screen.findByRole("list", { name: "Notes" });
    expect(within(list).getByText("first")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("New note"), "second");
    await userEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(await screen.findByText("second")).toBeInTheDocument();
    expect(screen.getByLabelText("New note")).toHaveValue("");
    await userEvent.click(screen.getByRole("button", { name: "Delete note 1" }));
    expect(await screen.findByText("second")).toBeInTheDocument();
    expect(screen.queryByText("first")).toBeNull();
  });

  it("says what went wrong and offers the file once there is one", async () => {
    const api = fake([{ id: 1, body: "with a file", file: "notes/1/a.txt", created_at: "" }]);
    api.remove = vi.fn(async () => {
      throw new Error("the database is busy");
    });
    render(<App api={api} />);
    expect(await screen.findByRole("button", { name: "Open the file" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Delete note 1" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("the database is busy");
    await userEvent.upload(screen.getByLabelText("Replace the file"), new File(["x"], "b.txt"));
    expect(api.upload).toHaveBeenCalledWith(1, expect.any(File));
  });

  it("opens a note's file in a new tab, and says when its URL cannot be had", async () => {
    const api = fake([{ id: 1, body: "with a file", file: "notes/1/a.txt", created_at: "" }]);
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    render(<App api={api} />);
    await userEvent.click(await screen.findByRole("button", { name: "Open the file" }));
    expect(open).toHaveBeenCalledWith("https://store.example/x", "_blank", "noopener");
    api.downloadUrl = vi.fn(async () => {
      throw new Error("the link has expired");
    });
    await userEvent.click(screen.getByRole("button", { name: "Open the file" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("the link has expired");
    open.mockRestore();
  });

  it("attaches a first file, ignores a picker closed without one, and says when the list cannot be read", async () => {
    const api = fake([{ id: 2, body: "plain", file: null, created_at: "" }]);
    const { unmount } = render(<App api={api} />);
    const picker = await screen.findByLabelText("Attach a file");
    await userEvent.upload(picker, new File(["y"], "c.txt"));
    expect(api.upload).toHaveBeenCalledWith(2, expect.any(File));
    await userEvent.upload(picker, []);
    expect(api.upload).toHaveBeenCalledTimes(1);
    unmount();
    const broken = fake([]);
    broken.list = vi.fn(async () => {
      throw new Error("the server is not answering");
    });
    render(<App api={broken} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("the server is not answering");
  });

  it("says there is nothing yet, and adds nothing empty", async () => {
    render(<App api={fake([])} />);
    expect(await screen.findByText("No notes yet.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add" })).toBeDisabled();
  });

  it("sends nothing for a note of spaces, even by Enter", async () => {
    const api = fake([]);
    render(<App api={api} />);
    await screen.findByText("No notes yet.");
    const field = screen.getByLabelText("New note");
    await userEvent.type(field, "   ");
    (field.closest("form") as HTMLFormElement).requestSubmit();
    expect(api.add).not.toHaveBeenCalled();
  });
});
