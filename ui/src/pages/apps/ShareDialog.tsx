import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { isChange, localized } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { useBranding } from "../../branding";
import { Button, Dialog, InlineError, RadioGroup } from "../../components/ui";
import type { RadioOption } from "../../components/ui";
import { appAddress } from "./AppOpenPage";
import { appSpec, visibilityLabel } from "./AppsCatalog";

type Visibility = "private" | "project" | "organization" | "roles" | "public";
const CHOICES: Visibility[] = ["private", "project", "organization", "roles", "public"];

interface AppRole {
  name: string;
  title?: Record<string, string>;
}

/**
 * Share an App (AP-173): who opens it now, each choice with who it reaches and how many people,
 * and the App's address to copy. A choice is proposed as a Change of `spec.visibility` and waits
 * for an approver; making it public waits for a publisher who did not propose it (AP-175). The
 * server checks the proposal again, so a choice this dialog offers by mistake is still refused.
 */
export function ShareDialog({
  project,
  app,
  onClose,
}: {
  project: string;
  app: Manifest;
  onClose: () => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const branding = useBranding();
  const name = app.metadata.name;
  const spec = appSpec(app);
  const current = (spec.visibility ?? "project") as Visibility;
  const roles = ((app.spec as { roles?: AppRole[] }).roles ?? []).map((role) =>
    localized(role.title, i18n.language, role.name),
  );
  const [choice, setChoice] = useState<Visibility>(current);
  const [change, setChange] = useState<Change | null>(null);
  const [copied, setCopied] = useState<"done" | "failed" | null>(null);
  const title = localized(app.metadata.title, i18n.language, name);
  const address = appAddress(name, branding.appsOrigin);
  const absolute = new URL(address, window.location.origin).toString();

  const audience = useQuery({
    queryKey: [...queryKeys.list(project, "apps"), name, "audience"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/apps/{name}/audience", {
          params: { path: { project, name } },
        }),
      ),
  });

  const propose = useMutation({
    mutationFn: async () =>
      proposeChecked(
        project,
        "apps",
        { apiVersion: app.apiVersion, kind: app.kind, metadata: app.metadata, spec: { ...spec, visibility: choice } },
        false,
      ),
    onSuccess: (result) => {
      if (isChange(result)) setChange(result);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "apps") });
    },
  });

  const counts = audience.data;
  const people = (count: number | undefined) =>
    count === undefined ? t("apps.share.counting") : t("apps.share.people", { count });
  const roleCount = counts?.roles.reduce((sum, role) => sum + role.people, 0);
  const reach: Record<Visibility, string> = {
    private: t("apps.share.choice.private.reach"),
    project: t("apps.share.choice.project.reach", { project, people: people(counts?.project) }),
    organization: t("apps.share.choice.organization.reach", {
      organization: branding.organisation || t("apps.share.theOrganization"),
      people: people(counts?.organization),
    }),
    roles:
      roles.length === 0
        ? t("apps.share.rolesNone")
        : t("apps.share.choice.roles.reach", { roles: roles.join(", "), people: people(roleCount) }),
    public:
      counts?.publicAllowed === false ? t("apps.share.publicRefused") : t("apps.share.choice.public.reach"),
  };
  // A published App is never private (AP-18); the server refuses it, so the dialog says why first.
  const published = spec.lifecycle === "published";
  const disabled: Record<Visibility, boolean> = {
    private: published && current !== "private",
    project: false,
    organization: false,
    roles: roles.length === 0,
    public: counts?.publicAllowed === false,
  };
  const options: RadioOption<Visibility>[] = CHOICES.map((value) => ({
    value,
    label: t(`apps.share.choice.${value}.label`),
    description:
      disabled.private && value === "private" ? t("apps.share.privatePublished") : reach[value],
    disabled: disabled[value] && value !== current,
  }));
  const failure = propose.error
    ? propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : t("app.error.generic")
    : null;

  const copy = () => {
    navigator.clipboard.writeText(absolute).then(
      () => setCopied("done"),
      () => setCopied("failed"),
    );
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={t("apps.share.title", { name: title })}
      description={t("apps.share.description")}
      closeLabel={t("app.close")}
      footer={
        change ? (
          <Button onClick={onClose}>{t("app.close")}</Button>
        ) : (
          <>
            <Button onClick={onClose}>{t("apps.share.cancel")}</Button>
            <Button
              variant="primary"
              disabled={propose.isPending || choice === current}
              disabledReason={choice === current ? t("apps.share.same") : undefined}
              onClick={() => propose.mutate()}
            >
              {t("apps.share.propose")}
            </Button>
          </>
        )
      }
    >
      <div className="space-y-4">
        <p className="text-body text-fg">
          {t("apps.share.now", { audience: visibilityLabel(current, t) })}
        </p>
        {change ? (
          <div className="space-y-2" role="status">
            <p className="text-body text-fg">{t("apps.share.proposed")}</p>
            <ChangeNotice change={change} project={project} />
          </div>
        ) : (
          <RadioGroup
            name="app-share-visibility"
            legend={t("apps.share.legend")}
            description={t("apps.share.hint")}
            value={choice}
            options={options}
            onChange={setChoice}
          />
        )}
        {audience.isError ? <p className="text-caption text-fg-muted">{t("apps.share.countFailed")}</p> : null}
        {failure ? <InlineError>{failure}</InlineError> : null}
        <section aria-labelledby="app-share-address" className="space-y-1">
          <h3 id="app-share-address" className="text-body font-medium text-fg">
            {t("apps.share.address")}
          </h3>
          <p className="text-caption text-fg-muted">{t("apps.share.addressHint")}</p>
          <div className="flex flex-wrap items-center gap-2">
            <code className="min-w-0 break-all rounded bg-surface-subtle px-2 py-1 text-caption">{absolute}</code>
            <Button size="sm" onClick={copy}>
              {t("apps.share.copy")}
            </Button>
          </div>
          <p aria-live="polite" className="text-caption text-fg-muted">
            {copied === "done" ? t("apps.share.copied") : copied === "failed" ? t("apps.share.copyFailed") : null}
          </p>
        </section>
      </div>
    </Dialog>
  );
}
