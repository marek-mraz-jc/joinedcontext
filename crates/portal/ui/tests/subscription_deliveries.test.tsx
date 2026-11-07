/**
 * T-4866 (CC-72, UI-30): a Subscription row says whether its receiver gets what it watches.
 *
 * The reconciler keeps what the space reports of the deliveries in `status.notifications`; the
 * row shows it in words: nothing sent yet, delivered with the last time, or failing with the last
 * failure and what to check. The tone never carries the meaning alone.
 */
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { DeliveryCell } from "../src/routes/SubscriptionsPage";
import type { Deliveries } from "../src/routes/SubscriptionsPage";

function cell(deliveries: Deliveries | undefined, live = true) {
  return render(
    <I18nextProvider i18n={i18n}>
      <DeliveryCell deliveries={deliveries} live={live} />
    </I18nextProvider>,
  );
}

describe("the deliveries of a subscription", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("says nothing was sent yet while the space reports no notification", () => {
    cell(undefined);
    expect(screen.getByText(/No notification yet/)).toBeInTheDocument();
    cell({ timesSent: 0 });
    expect(screen.getAllByText(/No notification yet/)).toHaveLength(2);
  });

  it("names the last delivery and the counts when the receiver answers", () => {
    cell({ status: "ok", timesSent: 3, timesFailed: 0, lastSuccess: "2026-10-21T03:20:11Z", lastNotification: "2026-10-21T03:20:11Z" });
    expect(screen.getByText("Delivered")).toBeInTheDocument();
    expect(screen.getByText(/^Last delivered .*2026/)).toBeInTheDocument();
    expect(screen.getByText("3 sent, 0 failed")).toBeInTheDocument();
    expect(screen.queryByText(/did not accept/)).not.toBeInTheDocument();
  });

  it("says a failing receiver fails, since when, and what to check", () => {
    cell({ status: "failed", timesSent: 2, timesFailed: 2, lastFailure: "2026-10-21T03:21:00Z" });
    expect(screen.getByText("Failing")).toBeInTheDocument();
    expect(screen.getByText(/^Last failure .*2026/)).toBeInTheDocument();
    expect(screen.getByText("2 sent, 2 failed")).toBeInTheDocument();
    expect(screen.getByText(/The receiver did not accept the last notification/)).toBeInTheDocument();
  });

  it("shows nothing for a subscription the space does not hold", () => {
    const { container } = cell({ status: "ok", timesSent: 1 }, false);
    expect(container.textContent).toBe("—");
  });

  it("speaks Slovak in the Slovak Portal", async () => {
    await i18n.changeLanguage("sk");
    cell({ status: "failed", timesSent: 1, timesFailed: 1, lastFailure: "2026-10-21T03:21:00Z" });
    expect(screen.getByText("Zlyháva")).toBeInTheDocument();
    expect(screen.getByText("Odoslané: 1, zlyhalo: 1")).toBeInTheDocument();
  });
});
