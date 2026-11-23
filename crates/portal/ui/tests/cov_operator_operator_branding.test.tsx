/**
 * T-7589: coverage for src/pages/operator/OperatorBranding.tsx.
 * Exercises shortName change handler, removeFile buttons for logoSvg and faviconSvg,
 * long instance name truncation in sign-in preview, and file upload error validations.
 */
import { cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { MAX_SVG_BYTES, OperatorBranding, svgRefusal } from "../src/pages/operator/OperatorBranding";
import type { InstallationLook } from "../src/pages/operator/operatorApi";
import { json, renderPage } from "./page_contract";

const LOGO = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="5"/></svg>';
const FAVICON = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16"/></svg>';

afterEach(() => {
  cleanup();
});

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("OperatorBranding coverage", () => {
  it("handles shortName input changes (line 209)", async () => {
    const look: InstallationLook = {
      instanceName: "Helsinki Context",
      shortName: "Hel",
    };

    renderPage(<OperatorBranding look={look} />, {
      path: "/organization/operator-installation",
      answer: () => undefined,
    });

    const shortInput = await screen.findByLabelText(en.operator.installation.branding.shortName);
    expect(shortInput).toHaveValue("Hel");

    fireEvent.change(shortInput, { target: { value: "HelsinkiNew" } });
    expect(shortInput).toHaveValue("HelsinkiNew");
  });

  it("removes logoSvg and faviconSvg via removeFile button (lines 245-246)", async () => {
    const look: InstallationLook = {
      instanceName: "Helsinki Context",
      logoSvg: LOGO,
      faviconSvg: FAVICON,
    };

    renderPage(<OperatorBranding look={look} />, {
      path: "/organization/operator-installation",
      answer: () => undefined,
    });

    // Locate both "Remove file" buttons for logo and favicon
    const removeButtons = await screen.findAllByRole("button", { name: en.operator.installation.branding.removeFile });
    expect(removeButtons).toHaveLength(2);

    // Click remove on logoSvg (runs lines 244-246 for logo)
    fireEvent.click(removeButtons[0]);
    // Click remove on faviconSvg (runs lines 244-246 for favicon)
    fireEvent.click(removeButtons[1]);

    // After removing both, the remove buttons disappear and "From file" indicators show
    expect(screen.queryByRole("button", { name: en.operator.installation.branding.removeFile })).toBeNull();
    expect(screen.getAllByText(en.operator.installation.branding.fromFile)).toHaveLength(2);
  });

  it("truncates names longer than 28 characters in sign-in preview (line 402)", async () => {
    const longName = "A Very Long Instance Name That Exceeds Twenty Eight Characters";
    const look: InstallationLook = {
      instanceName: longName,
    };

    renderPage(<OperatorBranding look={look} />, {
      path: "/organization/operator-installation",
      answer: () => undefined,
    });

    // In preview: name.length > 28 ? `${name.slice(0, 27)}…` : name
    const truncated = `${longName.slice(0, 27)}…`;
    expect((await screen.findAllByText(truncated)).length).toBeGreaterThan(0);
  });

  it("proposes change with all updated colours and handles server error messages", async () => {
    let proposedBody: unknown;
    const look: InstallationLook = {
      instanceName: "Region Context",
    };

    renderPage(<OperatorBranding look={look} />, {
      path: "/organization/operator-installation",
      answer: async (url, request) => {
        if (url.pathname === "/api/v1/operator/changes" && request.method === "POST") {
          proposedBody = await request.json();
          return json(
            {
              title: "Unprocessable",
              status: 422,
              detail: "branding.colours.secondary: invalid hex",
              errors: ["branding.colours.secondary: must be hex", "unrelated failure"],
            },
            422,
          );
        }
        return undefined;
      },
    });

    fireEvent.change(await screen.findByLabelText(en.operator.installation.branding.colour.secondary), {
      target: { value: "#123456" },
    });
    fireEvent.change(screen.getByLabelText(en.operator.installation.branding.colour.accent), {
      target: { value: "#abcdef" },
    });
    fireEvent.change(screen.getByLabelText(en.operator.installation.branding.colour.background), {
      target: { value: "#ffffff" },
    });
    fireEvent.change(screen.getByLabelText(en.operator.installation.branding.colour.text), {
      target: { value: "#000000" },
    });

    fireEvent.click(screen.getByRole("button", { name: en.operator.installation.branding.propose }));

    await screen.findByText("Must be hex");
    expect(proposedBody).toMatchObject({
      kind: "BrandingSet",
      branding: {
        instanceName: "Region Context",
        colours: {
          secondary: "#123456",
          accent: "#abcdef",
          background: "#ffffff",
          text: "#000000",
        },
      },
    });
  });

  it("validates svgRefusal utility function on invalid SVGs", () => {
    expect(svgRefusal("<svg><script>alert(1)</script></svg>", 30)).toBe("script");
    expect(svgRefusal("<svg>ok</svg>", MAX_SVG_BYTES + 10)).toBe("size");
    expect(svgRefusal("<html>not svg</html>", 10)).toBe("type");
    expect(svgRefusal('<svg xmlns="http://www.w3.org/2000/svg"/>', 40)).toBeUndefined();
  });
});
