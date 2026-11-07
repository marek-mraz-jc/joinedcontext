import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { EmbeddedFrame, foreignHttps } from "./EmbeddedFrame";

const OSM = "https://www.openstreetmap.org/export/embed.html?bbox=24.9,60.15,25.0,60.2&layer=mapnik";

describe("EmbeddedFrame", () => {
  it("frames an https page of another site, titled, sandboxed, lazy and without a referrer", () => {
    render(<EmbeddedFrame src={OSM} title="Map of the stations" />);
    const frame = screen.getByTitle("Map of the stations");
    expect(frame.tagName).toBe("IFRAME");
    expect(frame).toHaveAttribute("src", OSM);
    expect(frame).toHaveAttribute("sandbox", "allow-scripts allow-same-origin allow-popups");
    expect(frame).toHaveAttribute("referrerpolicy", "no-referrer");
    expect(frame).toHaveAttribute("loading", "lazy");
  });

  it("refuses the App's own origin, which could lift its sandbox, and says why", () => {
    render(<EmbeddedFrame src={`${window.location.origin}/other.html`} title="Mine" />);
    expect(screen.queryByTitle("Mine")).not.toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("is not an https page of another site");
  });

  it("refuses anything that is not https", () => {
    for (const src of ["http://example.org/", "javascript:alert(1)", "data:text/html,<p>x</p>", "not a url", "", "https://user:pw@example.org/"]) {
      expect(foreignHttps(src, "https://app.example"), src).toBeUndefined();
    }
    expect(foreignHttps("https://example.org/a?b=1", "https://app.example")).toBe("https://example.org/a?b=1");
  });
});
