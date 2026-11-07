import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LifecycleBadge } from "../src/components/status/LifecycleBadge";

function renderBadges(children: React.ReactNode) {
  return render(<I18nextProvider i18n={i18n}>{children}</I18nextProvider>);
}

describe("lifecycle badge", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("labels every phase the reconciler reports", () => {
    renderBadges(
      <>
        <LifecycleBadge kind="phase" value="Draft" />
        <LifecycleBadge kind="phase" value="PendingApproval" />
        <LifecycleBadge kind="phase" value="Deploying" />
        <LifecycleBadge kind="phase" value="Live" />
        <LifecycleBadge kind="phase" value="Error" />
        <LifecycleBadge kind="phase" value="Drifted" />
      </>,
    );

    for (const label of [
      en.phase.draft,
      en.phase.pendingApproval,
      en.phase.deploying,
      en.phase.live,
      en.phase.error,
      en.phase.drifted,
    ]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });

  it("gives each variant its own colour scheme", () => {
    renderBadges(
      <>
        <LifecycleBadge kind="phase" value="Deploying" />
        <LifecycleBadge kind="phase" value="Live" />
        <LifecycleBadge kind="phase" value="Error" />
        <LifecycleBadge kind="phase" value="Drifted" />
      </>,
    );

    expect(screen.getByText(en.phase.deploying).className).toContain("info");
    expect(screen.getByText(en.phase.live).className).toContain("success");
    expect(screen.getByText(en.phase.error).className).toContain("danger");
    // The `accent` token, not Tailwind's `purple-500`, which followed neither the dark theme
    // nor an installation's brand (T-1734).
    expect(screen.getByText(en.phase.drifted).className).toContain("accent");
  });

  it("explains each variant in a tooltip, so colour is never the only carrier", () => {
    renderBadges(
      <>
        <LifecycleBadge kind="phase" value="Deploying" />
        <LifecycleBadge kind="lane" value="red" />
      </>,
    );

    expect(screen.getByText(en.phase.deploying)).toHaveAttribute("title", en.phase.deployingHelp);
    expect(screen.getByText(en.lane.red)).toHaveAttribute("title", en.lane.redHelp);
  });

  it("colours the three risk lanes green, amber and red", () => {
    renderBadges(
      <>
        <LifecycleBadge kind="lane" value="green" />
        <LifecycleBadge kind="lane" value="yellow" />
        <LifecycleBadge kind="lane" value="red" />
      </>,
    );

    expect(screen.getByText(en.lane.green).className).toContain("success");
    expect(screen.getByText(en.lane.yellow).className).toContain("warning");
    expect(screen.getByText(en.lane.red).className).toContain("danger");
  });

  it("translates the label with the locale", async () => {
    await i18n.changeLanguage("sk");
    renderBadges(<LifecycleBadge kind="phase" value="Deploying" />);
    expect(screen.getByText("Nasadzuje sa")).toBeInTheDocument();
  });

  it("never says a merged resource the reconciler has not deployed is waiting for an approver", () => {
    // T-2873: ten praha pipelines over their resident quota read "Pending approval" while the
    // Approvals page, rightly, had no open change. Only a Change waits for an approver (CC-33).
    renderBadges(<LifecycleBadge kind="phase" value="Pending" />);
    const chip = screen.getByText(en.phase.notDeployed);
    expect(chip).toHaveAttribute("title", en.phase.notDeployedHelp);
    expect(screen.queryByText(en.phase.pendingApproval)).not.toBeInTheDocument();
  });

  it("says pending approval only for a phase that has a proposal in Approvals", () => {
    // Every phase a resource reports (jc-core `Phase`) against the one a Change reports while
    // its proposal is open: only the latter may send a person to the Approvals page.
    for (const phase of ["Draft", "Pending", "Deploying", "Live", "Error", "Drifted"]) {
      const { unmount } = renderBadges(<LifecycleBadge kind="phase" value={phase} />);
      expect(screen.queryByText(en.phase.pendingApproval), phase).not.toBeInTheDocument();
      unmount();
    }
    renderBadges(<LifecycleBadge kind="phase" value="PendingApproval" />);
    expect(screen.getByText(en.phase.pendingApproval)).toBeInTheDocument();
  });

  it("shows an unknown status verbatim instead of an empty chip", () => {
    renderBadges(<LifecycleBadge kind="phase" value="Hibernating" />);
    const chip = screen.getByText("Hibernating");
    expect(chip).not.toHaveAttribute("title");
    expect(chip.className).toContain("bg-surface-subtle");
  });
});
