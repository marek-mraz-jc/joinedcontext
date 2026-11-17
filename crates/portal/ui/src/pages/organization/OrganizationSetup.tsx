import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { organizationReading } from "../../api/odata";
import { useAdministers } from "../../api/permissions";
import { Alert, Badge, buttonClass, PageFailed, PageLoading } from "../../components/ui";
import { AskAdmins } from "./AskAdmins";

/**
 * Organization → Setup (T-2748, PF-90, UI-82, API/01 §25): what a new organization still lacks,
 * one step at a time, each with the page that proposes it the normal way, and what the
 * installation's operator has to provide. The page reads; every write happens where it links.
 */

const SETUP_KEY = ["organizationSetup"] as const;

type Step = "organization" | "domain" | "people" | "project" | "publishers" | "policies";

/** Where each step is done: the organization's own tab, or a project's page for the catalogue. */
function StepLink({ step, anchor }: { step: Step; anchor: string }): JSX.Element {
  const { t } = useTranslation();
  const label = t(`organization.setup.step.${step}.action`);
  const className = buttonClass("secondary", "sm");
  if (step === "publishers") {
    return (
      <Link to="/projects/$project/ckan" params={{ project: anchor }} className={className}>
        {label}
      </Link>
    );
  }
  const tab = step === "people" ? "people" : step === "project" ? "projects" : "settings";
  return (
    <Link to="/organization/$tab" params={{ tab }} className={className}>
      {label}
    </Link>
  );
}

function useSetup(enabled = true) {
  return useQuery({
    queryKey: SETUP_KEY,
    enabled,
    queryFn: async () => organizationReading("setup"),
  });
}

function State({ done, theirs }: { done: boolean; theirs?: "member" | "deployment" }): JSX.Element {
  const { t } = useTranslation();
  // An item somebody else sets is no alarm for the person reading it (T-7152): a member's
  // operators set it, and the deployment still sets what the Installation page does not yet.
  const open = theirs === "deployment" ? "organization.setup.deploymentSets" : theirs ? "organization.setup.notYet" : "organization.setup.todo";
  return (
    <Badge tone={done ? "success" : theirs ? "neutral" : "warning"}>
      {done ? t("organization.setup.done") : t(open)}
    </Badge>
  );
}

/**
 * What an operator item offers: an operator opens the Installation page where it is set (the
 * e-mail server today, T-7152); anybody else reads that their operator sets it.
 */
function OperatorAction({ item, operates }: { item: string; operates: boolean }): JSX.Element | null {
  const { t } = useTranslation();
  if (!operates) {
    return <span className="text-caption text-fg-muted">{t("organization.setup.operatorSets")}</span>;
  }
  if (item !== "smtp") return null;
  return (
    <Link to="/organization/$tab" params={{ tab: "operator-installation" }} className={buttonClass("secondary", "sm")}>
      {t("organization.setup.setIt")}
    </Link>
  );
}

export function OrganizationSetup({ anchor }: { anchor: string }): JSX.Element {
  const { t } = useTranslation();
  const { known, administers } = useAdministers();
  const setup = useSetup(administers);

  if (!known) return <PageLoading label={t("organization.setup.loading")} />;
  if (!administers) {
    return (
      <section className="space-y-4" aria-labelledby="setup-steps-heading">
        <h2 id="setup-steps-heading" className="text-title font-semibold text-fg">
          {t("organization.setup.title")}
        </h2>
        <Alert tone="info">{t("organization.setup.hidden")}</Alert>
      </section>
    );
  }
  if (setup.isPending) return <PageLoading label={t("organization.setup.loading")} />;
  if (setup.isError) return <PageFailed error={setup.error} onRetry={() => void setup.refetch()} />;

  const { complete, steps, operator, operates } = setup.data;
  return (
    <div className="space-y-8">
      <section className="space-y-4" aria-labelledby="setup-steps-heading">
        <div>
          <h2 id="setup-steps-heading" className="text-title font-semibold text-fg">
            {t("organization.setup.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("organization.setup.lead")}</p>
        </div>
        {complete ? <Alert tone="success">{t("organization.setup.complete")}</Alert> : null}
        <ol className="space-y-3">
          {steps.map((step, index) => (
            <li
              key={step.id}
              className="flex flex-col gap-3 rounded-lg border border-border p-4 sm:flex-row sm:items-start sm:justify-between"
            >
              <div className="min-w-0 space-y-1">
                <h3 className="text-body font-semibold text-fg">
                  {t("organization.setup.stepNumber", { number: index + 1 })}{" "}
                  {t(`organization.setup.step.${step.id}.title`)}
                </h3>
                <p className="text-body text-fg-muted">{t(`organization.setup.step.${step.id}.hint`)}</p>
              </div>
              <div className="flex items-center gap-3">
                <State done={step.done} />
                <StepLink step={step.id as Step} anchor={anchor} />
              </div>
            </li>
          ))}
        </ol>
      </section>
      <section className="space-y-4" aria-labelledby="setup-operator-heading">
        <div>
          <h2 id="setup-operator-heading" className="text-title font-semibold text-fg">
            {t("organization.setup.operatorTitle")}
          </h2>
          <p className="text-body text-fg-muted">
            {t(operates ? "organization.setup.operatorLead" : "organization.setup.operatorLeadMember")}
          </p>
        </div>
        <ul className="space-y-3">
          {operator.map((item) => (
            <li
              key={item.id}
              className="flex flex-col gap-3 rounded-lg border border-border p-4 sm:flex-row sm:items-start sm:justify-between"
            >
              <div className="min-w-0 space-y-1">
                <h3 className="text-body font-semibold text-fg">{t(`organization.setup.operator.${item.id}.title`)}</h3>
                <p className="text-body text-fg-muted">
                  {t(`organization.setup.operator.${item.id}.${operates ? "hint" : "about"}`)}
                </p>
              </div>
              <div className="flex items-center gap-3">
                {/* Only the e-mail server is set on the Installation page today: the other items are no
                    red to-do for anybody yet (branding T-7188, backups T-7189). */}
                <State done={item.done} theirs={!operates ? "member" : item.id !== "smtp" ? "deployment" : undefined} />
                <OperatorAction item={item.id} operates={operates} />
              </div>
            </li>
          ))}
        </ul>
        {/* Whoever cannot set these reads whom to ask (the installation's contacts, T-7152). */}
        {operates ? null : (
          <AskAdmins title={t("organization.setup.whomToAsk")} message={t("organization.setup.whomToAskLead")} installation />
        )}
      </section>
    </div>
  );
}

/**
 * The reminder on every other tab while the setup is not complete, shown only to a person who
 * may change the organization: nobody else is asked for a read the API would refuse.
 */
export function SetupReminder(): JSX.Element | null {
  const { t } = useTranslation();
  const { administers } = useAdministers();
  const setup = useSetup(administers);
  if (!administers || setup.data?.complete !== false) return null;
  const left = [...setup.data.steps, ...setup.data.operator].filter((item) => !item.done).length;
  return (
    <Alert
      tone="info"
      actions={
        <Link to="/organization/$tab" params={{ tab: "setup" }} className={buttonClass("secondary", "sm")}>
          {t("organization.setup.continue")}
        </Link>
      }
    >
      {t("organization.setup.reminder", { count: left })}
    </Alert>
  );
}
