import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider, ProblemError } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { AppShell, navigate } from "./AppShell";
import { reportError } from "@joinedcontext/sdk";
import { Problem } from "./states";
import { StatTiles } from "./StatTiles";
import { stubClient } from "@joinedcontext/sdk/testing";

describe("AppShell", () => {
  beforeEach(() => {
    window.location.hash = "";
  });

  it("renders the first page, button switches pages and marks aria-current", () => {
    const client = stubClient({}, { user: { id: "u-1", name: "Ada Lovelace" } });
    const pages = [
      { id: "p1", label: "Dashboard", render: () => <div>Dashboard View</div> },
      { id: "p2", label: "Settings", render: () => <div>Settings View</div> },
    ];

    render(
      <JcProvider client={client}>
        <AppShell title="Control Panel" pages={pages} />
      </JcProvider>,
    );

    expect(screen.getByRole("heading", { name: "Control Panel" })).toBeInTheDocument();
    expect(screen.getByText("Ada Lovelace")).toBeInTheDocument();
    expect(screen.getByText("Dashboard View")).toBeInTheDocument();
    expect(screen.queryByText("Settings View")).not.toBeInTheDocument();

    const p1Btn = screen.getByRole("button", { name: "Dashboard" });
    const p2Btn = screen.getByRole("button", { name: "Settings" });

    expect(p1Btn).toHaveAttribute("aria-current", "page");
    expect(p2Btn).not.toHaveAttribute("aria-current");

    fireEvent.click(p2Btn);

    expect(screen.getByText("Settings View")).toBeInTheDocument();
    expect(screen.queryByText("Dashboard View")).not.toBeInTheDocument();
    expect(p2Btn).toHaveAttribute("aria-current", "page");
    expect(p1Btn).not.toHaveAttribute("aria-current");
  });

  it("#/second hash selects the page on mount", () => {
    window.location.hash = "#/second";
    const client = stubClient();
    const pages = [
      { id: "first", label: "First", render: () => <div>First View</div> },
      { id: "second", label: "Second", render: () => <div>Second View</div> },
    ];

    render(
      <JcProvider client={client}>
        <AppShell title="App" pages={pages} />
      </JcProvider>,
    );

    expect(screen.getByText("Second View")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Second" })).toHaveAttribute("aria-current", "page");
  });

  it("navigate() opens a page through the address the shell follows", async () => {
    const client = stubClient();
    const pages = [
      { id: "home", label: "Home", render: () => <div>Home View</div> },
      { id: "detail", label: "Detail", render: () => <div>Detail View</div> },
    ];

    render(
      <JcProvider client={client}>
        <AppShell title="App" pages={pages} />
      </JcProvider>,
    );

    expect(screen.getByText("Home View")).toBeInTheDocument();
    act(() => {
      navigate("detail");
    });
    expect(await screen.findByText("Detail View")).toBeInTheDocument();
    expect(window.location.hash).toBe("#/detail");
  });

  it("a throwing page shows Problem with Retry and the other pages still work", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const client = stubClient();

    let shouldThrow = true;
    function ThrowingPage() {
      if (shouldThrow) {
        throw new Error("Broken page rendered");
      }
      return <div>Recovered Page</div>;
    }

    const pages = [
      { id: "good", label: "Good Page", render: () => <div>Good View</div> },
      { id: "bad", label: "Bad Page", render: () => <ThrowingPage /> },
    ];

    render(
      <JcProvider client={client}>
        <AppShell title="App" pages={pages} />
      </JcProvider>,
    );

    expect(screen.getByText("Good View")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Bad Page" }));

    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getByText("Broken page rendered")).toBeInTheDocument();

    // Switch back to good page
    fireEvent.click(screen.getByRole("button", { name: "Good Page" }));
    expect(screen.getByText("Good View")).toBeInTheDocument();

    // Back to the bad page, still broken; Retry renders it again once the cause is gone
    fireEvent.click(screen.getByRole("button", { name: "Bad Page" }));
    expect(screen.getByRole("alert")).toBeInTheDocument();
    shouldThrow = false;

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(screen.getByText("Recovered Page")).toBeInTheDocument();
  });

  it("renders Empty when pages array is empty", () => {
    const client = stubClient();
    render(
      <JcProvider client={client}>
        <AppShell title="App" pages={[]} />
      </JcProvider>,
    );
    expect(screen.getByText("No pages.")).toBeInTheDocument();
  });
});

describe("StatTiles", () => {
  const rows: Row[] = [
    { id: "1", type: "Sensor", temperature: 20.25 },
    { id: "2", type: "Sensor", temperature: 25.75 },
  ];

  it("renders count/avg/digits/unit, null -> '–', loading -> '…'", () => {
    const tiles = [
      { label: "Count", agg: "count" as const },
      { label: "Avg Temp", agg: "avg" as const, attr: "temperature", digits: 1, unit: "°C" },
      { label: "Missing", agg: "sum" as const, attr: "nonexistent" },
    ];

    const { rerender } = render(<StatTiles rows={rows} tiles={tiles} />);

    expect(screen.getByText("Count")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();

    expect(screen.getByText("Avg Temp")).toBeInTheDocument();
    expect(screen.getByText("23")).toBeInTheDocument();
    expect(screen.getByText("°C")).toBeInTheDocument();

    expect(screen.getByText("Missing")).toBeInTheDocument();
    expect(screen.getByText("–")).toBeInTheDocument();

    rerender(<StatTiles rows={rows} tiles={tiles} loading={true} />);

    const loadingEllipses = screen.getAllByText("…");
    expect(loadingEllipses).toHaveLength(3);
  });
});

describe("Problem and reportError", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("Problem shows ProblemError title+detail, plain Error title, and nothing for null", () => {
    const { container, rerender } = render(<Problem error={null} />);
    expect(container).toBeEmptyDOMElement();

    rerender(
      <Problem
        error={
          new ProblemError(403, {
            title: "Forbidden",
            detail: "User lacks permission to edit status",
          })
        }
      />,
    );
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getByText("Forbidden")).toBeInTheDocument();
    expect(screen.getByText("User lacks permission to edit status")).toBeInTheDocument();

    rerender(<Problem error={new Error("Standard network failure")} />);
    expect(screen.getByText("Standard network failure")).toBeInTheDocument();
  });

  it("reportError logs to console and posts jc-error to parent when framed", () => {
    const consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const postMessageMock = vi.fn();

    const originalParent = window.parent;
    try {
      Object.defineProperty(window, "parent", {
        value: { postMessage: postMessageMock },
        configurable: true,
        writable: true,
      });

      const err = new Error("Boom in test");
      reportError(err);

      expect(consoleSpy).toHaveBeenCalledWith("jc:", err);
      expect(postMessageMock).toHaveBeenCalledTimes(1);
      const [msg, targetOrigin] = postMessageMock.mock.calls[0];
      expect(targetOrigin).toBe("*");
      expect(msg.kind).toBe("jc-error");
      expect(msg.message).toBe("Boom in test");
    } finally {
      Object.defineProperty(window, "parent", {
        value: originalParent,
        configurable: true,
        writable: true,
      });
    }
  });
});
