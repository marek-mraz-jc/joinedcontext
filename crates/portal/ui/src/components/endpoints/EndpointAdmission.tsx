import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { Alert } from "../ui/Alert";

/** Where the Endpoint page holds the person's own ask for a read (EP-55). */
export const REQUEST_ACCESS_ANCHOR = "request-access";

/**
 * Whether the gateway admits the signed-in person on Endpoint `name`, as the Portal decides it with
 * the gateway's own rule (EP-14, EP-15, T-6802). A page reads nothing under `/api/endpoint/{slug}`
 * until this says yes: an administrator outside the Endpoint's project groups would meet a 403 per
 * read.
 */
export function useEndpointAdmission(project: string, name: string | undefined) {
  return useQuery({
    queryKey: ["endpoint-admission", project, name],
    enabled: Boolean(name),
    retry: false,
    // One person's standing never outlives their page (R17).
    gcTime: 0,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/endpoints/{name}/admission", {
          params: { path: { project, name: name ?? "" } },
        }),
      ),
  });
}

/**
 * What a page shows in place of an Endpoint's data while the admission is asked, when it says no,
 * or when it cannot be read; `null` once the person is admitted. Names no group or Policy.
 */
export function EndpointRefused({
  project,
  name,
  admission,
  onPage,
}: {
  project: string;
  name: string;
  admission: ReturnType<typeof useEndpointAdmission>;
  /** On the Endpoint page the request form is below; elsewhere the link opens that page. */
  onPage?: boolean;
}): React.JSX.Element | null {
  const { t } = useTranslation();
  if (admission.isPending) {
    return <p role="status" className="text-body text-fg-muted">{t("endpoints.refused.checking")}</p>;
  }
  if (admission.isError) {
    return <Alert tone="warning">{t("endpoints.refused.unavailable")}</Alert>;
  }
  if (admission.data.admitted) return null;
  const linkClass = "focus-ring touch-size inline-flex items-center font-medium text-primary-soft-fg underline hover:no-underline";
  return (
    <Alert
      tone="info"
      title={t("endpoints.refused.title")}
      data-endpoint-refused=""
      actions={
        onPage ? (
          <a href={`#${REQUEST_ACCESS_ANCHOR}`} className={linkClass}>
            {t("endpoints.refused.action")}
          </a>
        ) : (
          <Link
            to="/projects/$project/$plural/$name"
            params={{ project, plural: "endpoints", name }}
            hash={REQUEST_ACCESS_ANCHOR}
            className={linkClass}
          >
            {t("endpoints.refused.action")}
          </Link>
        )
      }
    >
      {t("endpoints.refused.body")}
    </Alert>
  );
}
