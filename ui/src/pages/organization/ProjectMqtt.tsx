import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { ORG_NAMESPACE } from "../../api/manifest";
import { useAdministers } from "../../api/permissions";
import { EditResourceAction } from "../../components/EditResourceDialog";
import type { EditableForm } from "../../components/EditResourceDialog";
import { Badge, Button } from "../../components/ui";
import {
  ALLOWANCE_UI,
  allowanceSchema,
  fromProjectAllowance,
  toProjectAllowance,
  useMqttAllowance,
} from "../mqttbrokers/allowance";

/**
 * Administration → Projects → MQTT of one project (T-4860, PF-117): whether the project may run
 * edge brokers, how many and how large, which modes and addresses. Saving proposes the Project
 * with its `spec.quotas.mqtt`, a Change a second admin approves (T-4833). Only an organization
 * administrator sees it; the API refuses the change to anybody else (403).
 */
export function ProjectMqtt({ project, title }: { project: string; title: string }): JSX.Element | null {
  const { t } = useTranslation();
  const { administers } = useAdministers();
  const [open, setOpen] = useState(false);
  if (!administers) return null;
  const form: EditableForm = {
    schema: allowanceSchema(t),
    uiSchema: ALLOWANCE_UI,
    fromManifest: fromProjectAllowance,
    toManifest: toProjectAllowance,
  };
  return (
    <>
      <Button
        variant="secondary"
        size="sm"
        aria-label={t("mqttAllowance.admin.action", { project: title })}
        onClick={() => setOpen(true)}
      >
        {t("mqttAllowance.admin.button")}
      </Button>
      <EditResourceAction
        target={{ project, home: ORG_NAMESPACE, kind: "Project", plural: "projects", name: project, label: title }}
        form={form}
        trigger={false}
        open={open}
        onOpenChange={setOpen}
      />
    </>
  );
}

/** The project's MQTT state in a word for the projects table: off, or on with its brokers. */
export function ProjectMqttState({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const allowance = useMqttAllowance(project);
  if (allowance.isPending) return <>{t("app.loading")}</>;
  if (allowance.isError) return <>—</>;
  const { enabled, brokers } = allowance.data;
  return enabled ? (
    <Badge tone="success">{t("mqttAllowance.admin.on", { used: brokers.used, limit: brokers.limit })}</Badge>
  ) : (
    <Badge tone="neutral">{t("mqttAllowance.admin.off")}</Badge>
  );
}
