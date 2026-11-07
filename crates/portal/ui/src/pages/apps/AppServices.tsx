import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { isChange, ORG_NAMESPACE, storedMetadata } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { useProjectOrganization } from "../../api/projects";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert, Button, Checkbox } from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { APP_SERVICES, DEFAULT_ORGANIZATION_SERVICES } from "../../schemas/kinds";
import type { AppServiceName } from "../../schemas/kinds";

/** Why a box cannot be ticked: every App has it, or the organization or the project keeps it off. */
export type ServiceLock = "everyApp" | "organization" | "project";

/**
 * Whether the App may switch `service` and, when not, which layer decides (AP-163, AP-164): the
 * same order the Portal checks on every call, so the page and the refusal name the same layer.
 */
export function serviceLock(
  service: AppServiceName,
  organization: readonly string[] | undefined,
  project: readonly string[] | undefined,
): ServiceLock | undefined {
  if (service === "identity" || service === "data") return "everyApp";
  if (!(organization ?? DEFAULT_ORGANIZATION_SERVICES).includes(service)) return "organization";
  if (project && !project.includes(service)) return "project";
  return undefined;
}

interface Spec {
  services?: string[];
  [rest: string]: unknown;
}

/**
 * The App's platform services (AP-163, AP-164): each one ticked when the App lists it in
 * `spec.services`, `identity` and `data` always, and a service the organization or the project
 * keeps off shown off with that layer named rather than hidden. Saving proposes the App with the
 * new list, a Change like any other edit; the Portal checks the three layers on every call, so a
 * service switched off stops at once, not at the next deploy.
 */
export function AppServices({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t } = useTranslation();
  const heading = useId();
  const queryClient = useQueryClient();
  const [picked, setPicked] = useState<string[] | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);
  const app = useQuery({
    queryKey: queryKeys.resource(project, "apps", name),
    queryFn: async () =>
      unwrap<unknown>(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "apps", name } },
        }),
      ),
    retry: false,
  });
  const projectManifest = useQuery({
    queryKey: [...queryKeys.list(ORG_NAMESPACE, "projects"), project],
    queryFn: async () =>
      unwrap<unknown>(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project: ORG_NAMESPACE, plural: "projects", name: project } },
        }),
      ),
    retry: false,
  });
  // The project's own Organization, whose locks bind this App (ADR-N-058, F-UI1-012).
  const { organization: owner } = useProjectOrganization(project);
  const propose = useMutation({
    mutationFn: (manifest: Manifest) => proposeChecked(project, "apps", manifest as ResourceProposal, false),
    onMutate: () => {
      setError(null);
      setChange(null);
    },
    onSuccess: (result) => {
      if (isChange(result)) setChange(result);
      setPicked(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.resource(project, "apps", name) });
    },
    onError: (err) =>
      setError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic")),
  });

  if (!app.data) return null;
  const spec = ((app.data as { spec?: Spec }).spec ?? {}) as Spec;
  const stored = spec.services ?? [];
  const services = picked ?? stored;
  const organization = (
    owner?.spec as { policies?: { apps?: { services?: string[] } } } | undefined
  )?.policies?.apps?.services;
  const projectServices = (projectManifest.data as { spec?: { apps?: { services?: string[] } } } | undefined)?.spec
    ?.apps?.services;
  const changed = picked !== null && [...picked].sort().join() !== [...stored].sort().join();

  return (
    <section aria-labelledby={heading} className="space-y-3 rounded border border-border p-4">
      <h2 id={heading} className="text-lg font-semibold">
        {t("appServices.title")}
      </h2>
      <p className="text-sm text-fg-muted">{t("appServices.appLead")}</p>
      {change && <ChangeNotice change={change} project={project} />}
      {error && (
        <Alert tone="danger" role="alert">
          {error}
        </Alert>
      )}
      <ul className="space-y-2">
        {APP_SERVICES.map((service) => {
          const lock = serviceLock(service, organization, projectServices);
          return (
            <li key={service}>
              <Checkbox
                label={t(`appServices.service.${service}.label`)}
                hint={t(`appServices.service.${service}.hint`)}
                checked={lock === "everyApp" || (lock === undefined && services.includes(service))}
                disabled={lock !== undefined || propose.isPending}
                disabledReason={lock ? t(`appServices.lock.${lock}`) : undefined}
                onChange={(event) =>
                  setPicked(
                    event.target.checked ? [...services, service] : services.filter((other) => other !== service),
                  )
                }
              />
            </li>
          );
        })}
      </ul>
      <PermissionGuard project={project} kind="App" verb="propose">
        <Button
          size="sm"
          disabled={!changed || propose.isPending}
          disabledReason={changed ? undefined : t("appServices.unchanged")}
          onClick={() =>
            propose.mutate({
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "App",
              metadata: { ...storedMetadata(app.data), name, namespace: project },
              spec: { ...spec, services },
            } as Manifest)
          }
        >
          {t("appServices.save")}
        </Button>
      </PermissionGuard>
    </section>
  );
}
