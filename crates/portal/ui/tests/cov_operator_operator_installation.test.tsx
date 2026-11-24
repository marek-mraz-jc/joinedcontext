// T-7589: coverage for OperatorInstallation (onRetry refetch, port adjustments on TLS, InForce edge cases, test email success)
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OperatorInstallation } from "../src/pages/operator/OperatorInstallation";
import { json, renderPage } from "./page_contract";

const OPERATOR = { project: "org", bootstrap: true, grants: [] } as unknown as Effective;

function renderInstallation(
  answers: (path: string, method: string, body: string) => Response | undefined,
  permissions = OPERATOR,
) {
  renderPage(<OperatorInstallation />, {
    path: "/organization/operator-installation",
    answer: async (url, request) => {
      const body = request.method === "GET" ? "" : await request.clone().text();
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      return answers(url.pathname, request.method, body);
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => cleanup());

describe("OperatorInstallation coverage", () => {
  it("retries fetching installation when PageFailed retry button is clicked", async () => {
    let callCount = 0;
    renderInstallation((path) => {
      if (path === "/api/v1/operator/installation") {
        callCount += 1;
        if (callCount === 1) {
          return json({ title: "Internal Server Error", status: 500, detail: "DB error" }, 500);
        }
        return json({ smtp: null, smtpChange: null, branding: null, brandingChange: null });
      }
      return undefined;
    });

    expect(await screen.findByText("DB error")).toBeInTheDocument();
    const retryBtn = screen.getByRole("button", { name: /try again|retry/i });
    fireEvent.click(retryBtn);

    await waitFor(() => {
      expect(callCount).toBeGreaterThanOrEqual(2);
    });
  });

  it("adjusts port on TLS selection if port matches default or empty, but preserves custom port", async () => {
    const E = en.operator.installation.email;
    renderInstallation((path) => {
      if (path === "/api/v1/operator/installation") {
        return json({ smtp: null, smtpChange: null, branding: null, brandingChange: null });
      }
      return undefined;
    });

    const portInput = (await screen.findByLabelText(E.port, { exact: false })) as HTMLInputElement;
    const tlsSelect = screen.getByLabelText(E.tls) as HTMLSelectElement;

    expect(portInput.value).toBe("587");

    fireEvent.change(tlsSelect, { target: { value: "ssl" } });
    expect(portInput.value).toBe("465");

    fireEvent.change(tlsSelect, { target: { value: "starttls" } });
    expect(portInput.value).toBe("587");

    fireEvent.change(portInput, { target: { value: "2525" } });
    fireEvent.change(tlsSelect, { target: { value: "ssl" } });
    expect(portInput.value).toBe("2525");

    fireEvent.change(portInput, { target: { value: "" } });
    fireEvent.change(tlsSelect, { target: { value: "ssl" } });
    expect(portInput.value).toBe("465");
  });

  it("displays success alert when test e-mail was sent successfully", async () => {
    const E = en.operator.installation.email;
    const SMTP = { host: "smtp.example.org", port: 587, tls: "starttls", from: "noreply@example.org", user: "mailer", passwordSet: true };

    renderInstallation((path, method) => {
      if (path === "/api/v1/operator/installation") {
        return json({ smtp: SMTP, smtpChange: null, branding: null, brandingChange: null });
      }
      if (path === "/api/v1/operator/installation/test-email" && method === "POST") {
        return json({ sent: true, reason: null });
      }
      return undefined;
    });

    const testBtn = await screen.findByRole("button", { name: E.test });
    fireEvent.click(testBtn);

    expect(await screen.findByText(E.testSent)).toBeInTheDocument();
  });

  it("shows elsewhere alert when propose fails with generic unhandled error", async () => {
    const E = en.operator.installation.email;
    renderInstallation((path, method) => {
      if (path === "/api/v1/operator/installation") {
        return json({ smtp: null, smtpChange: null, branding: null, brandingChange: null });
      }
      if (path === "/api/v1/operator/changes" && method === "POST") {
        return json({ title: "Bad Request", status: 400, detail: "system configuration locked" }, 400);
      }
      return undefined;
    });

    fireEvent.change(await screen.findByLabelText(E.host, { exact: false }), { target: { value: "smtp.example.org" } });
    fireEvent.change(screen.getByLabelText(E.from, { exact: false }), { target: { value: "a@b.com" } });
    fireEvent.click(screen.getByRole("button", { name: E.propose }));

    expect(await screen.findByText("System configuration locked")).toBeInTheDocument();
  });
});
