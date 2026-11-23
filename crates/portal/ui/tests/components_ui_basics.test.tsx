/**
 * Small interface parts at their edges: the drop zone's hover state, a card's middle click, the
 * switch with its label, the compact header's info mark on a tap, a reference whose copy fails,
 * and the address helpers on input no browser would accept.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApiError } from "../src/api/client";
import { Card } from "../src/components/ui/Card";
import { FileDropZone } from "../src/components/ui/FileDropZone";
import { PageHeader } from "../src/components/ui/PageHeader";
import { PageFailed } from "../src/components/ui/PageState";
import { Switch } from "../src/components/ui/Switch";
import { recordCard } from "../src/components/ui/Table";
import { endpointView, hostToShow, safeHref, sameOriginPath, withoutUserinfo } from "../src/components/ui/safeHref";

const show = (node: React.ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => vi.unstubAllGlobals());

describe("FileDropZone", () => {
  it("highlights while a file is dragged over it and drops the highlight when it leaves", () => {
    show(<FileDropZone label="Choose a bundle" button="Choose" onFiles={() => {}} />);
    const zone = screen.getByTestId("file-drop-zone");
    expect(zone).toHaveClass("bg-surface-subtle");
    fireEvent.dragOver(zone);
    expect(zone).toHaveClass("bg-primary-soft");
    fireEvent.dragLeave(zone);
    expect(zone).toHaveClass("bg-surface-subtle");
  });

  it("hands over the first dropped file only, and ignores a drop of nothing", () => {
    const onFiles = vi.fn();
    show(<FileDropZone label="Choose a bundle" button="Choose" onFiles={onFiles} />);
    const zone = screen.getByTestId("file-drop-zone");
    fireEvent.drop(zone, { dataTransfer: { files: [] } });
    expect(onFiles).not.toHaveBeenCalled();
    const a = new File(["a"], "a.zip");
    const b = new File(["b"], "b.zip");
    fireEvent.drop(zone, { dataTransfer: { files: [a, b] } });
    expect(onFiles).toHaveBeenCalledWith([a]);
  });
});

describe("a record card's middle click", () => {
  function clicks(link: HTMLElement): MouseEvent[] {
    const seen: MouseEvent[] = [];
    link.addEventListener("click", (event) => {
      event.preventDefault();
      seen.push(event);
    });
    return seen;
  }

  it("opens the record link of a Card as a new tab, and leaves other buttons alone", () => {
    const onAuxClick = vi.fn();
    show(
      <Card onAuxClick={onAuxClick}>
        <a data-row-link href="#record">Record</a>
        <span>body</span>
      </Card>,
    );
    const seen = clicks(screen.getByRole("link", { name: "Record" }));
    fireEvent(screen.getByText("body"), new MouseEvent("auxclick", { bubbles: true, button: 2 }));
    expect(seen).toHaveLength(0);
    fireEvent(screen.getByText("body"), new MouseEvent("auxclick", { bubbles: true, button: 1 }));
    expect(onAuxClick).toHaveBeenCalledTimes(2);
    expect(seen).toHaveLength(1);
    expect(seen[0]?.ctrlKey).toBe(true);
  });

  it("does the same for an article that spreads recordCard", () => {
    show(
      <article {...recordCard}>
        <a data-row-link href="#record">Record</a>
        <span>body</span>
      </article>,
    );
    const seen = clicks(screen.getByRole("link", { name: "Record" }));
    fireEvent(screen.getByText("body"), new MouseEvent("auxclick", { bubbles: true, button: 0 }));
    expect(seen).toHaveLength(0);
    fireEvent(screen.getByText("body"), new MouseEvent("auxclick", { bubbles: true, button: 1 }));
    expect(seen).toHaveLength(1);
  });
});

describe("Switch with a label", () => {
  it("toggles when its label text is clicked", async () => {
    const onCheckedChange = vi.fn();
    show(<Switch checked={false} onCheckedChange={onCheckedChange} label="Notify me" />);
    await userEvent.click(screen.getByText("Notify me"));
    expect(onCheckedChange).toHaveBeenCalledWith(true);
    expect(screen.getByRole("switch", { name: "Notify me" })).toHaveAttribute("aria-checked", "false");
  });
});

describe("Switch without a label", () => {
  it("is the bare track, named by the caller, and toggles off when on", async () => {
    const onCheckedChange = vi.fn();
    const { container } = show(<Switch checked onCheckedChange={onCheckedChange} aria-label="Enabled" />);
    expect(container.querySelector("label")).toBeNull();
    await userEvent.click(screen.getByRole("switch", { name: "Enabled" }));
    expect(onCheckedChange).toHaveBeenCalledWith(false);
  });
});

describe("PageHeader compact info mark", () => {
  it("shows and hides the description on a tap", async () => {
    show(<PageHeader compact title="Endpoints" description="Where a space is published." />);
    const mark = screen.getByRole("button", { name: en.pageHeader.about });
    expect(mark).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(mark);
    expect(mark).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(mark);
    expect(mark).toHaveAttribute("aria-expanded", "false");
  });
});

describe("a failure's reference", () => {
  it("keeps the copy button as it was when the clipboard refuses", async () => {
    const writeText = vi.fn(() => Promise.reject(new Error("denied")));
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText }, onLine: true });
    show(<PageFailed error={new ApiError(500, "Server", { type: "about:blank", title: "Server", status: 500 }, "9d8c7b6a")} />);
    const copy = screen.getByRole("button", { name: "Copy the reference 9d8c7b6a" });
    await userEvent.click(copy);
    expect(writeText).toHaveBeenCalledWith("9d8c7b6a");
    expect(screen.getByRole("button", { name: "Copy the reference 9d8c7b6a" })).toHaveTextContent(en.app.error.copy);
    expect(screen.getByRole("status", { hidden: true })).toBeEmptyDOMElement();
  });
});

describe("safeHref helpers on input a browser would not accept", () => {
  it("refuses a path whose host cannot be parsed and an absolute URL that does not parse", () => {
    expect(sameOriginPath("//[bad")).toBeUndefined();
    expect(sameOriginPath("/ok/path")).toBe("/ok/path");
    expect(safeHref("http://[bad")).toBeUndefined();
    expect(safeHref("javascript:alert(1)")).toBeUndefined();
    expect(safeHref("https://example.org/x")).toBe("https://example.org/x");
  });

  it("leaves an address with a non-numeric port and no user part as it is", () => {
    expect(withoutUserinfo("https://host:abc/x")).toBe("https://host:abc/x");
    expect(withoutUserinfo("https://user:pw@host/x")).toBe("https://host/x");
    expect(endpointView("https://host:abc/x").shown).toBe("host:abc/x");
  });

  it("prints no host for a fragment or query link, which names none", () => {
    expect(hostToShow("Jump", "#section")).toBeUndefined();
    expect(hostToShow("Jump", "?page=2")).toBeUndefined();
    expect(hostToShow("Docs", "https://el.fi/login")).toBe("el.fi");
  });
});
