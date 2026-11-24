/**
 * The page preview: what the server answers is rebuilt as elements; a comment in it is dropped and
 * nothing outside the allowed elements survives but its text.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { previewElements } from "../src/pages/organization/pageMarkdown";

describe("previewElements", () => {
  it("drops an HTML comment and keeps the text around it", () => {
    const { container } = render(<div>{previewElements("<p>before<!-- hidden note -->after</p>")}</div>);
    expect(container.textContent).toBe("beforeafter");
    expect(container.innerHTML).not.toContain("hidden note");
  });

  it("keeps only the text of an element outside the list", () => {
    render(<div>{previewElements('<p>a <span style="color:red">kept</span> <script>alert(1)</script></p>')}</div>);
    expect(screen.getByText(/kept/).tagName).toBe("P");
    expect(document.querySelector("span")).toBeNull();
  });
});
