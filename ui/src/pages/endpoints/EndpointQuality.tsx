import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { QualityScore } from "../../components/quality/QualityScore";

/**
 * The quality score of the dataset this Endpoint serves, with what to fix for a reader of its
 * space (T-5112). A failed read says so in one line and never takes the page down.
 */
export function EndpointQuality({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t } = useTranslation();
  const score = useQuery({
    queryKey: ["endpoint-quality", project, name],
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/endpoints/{name}/quality", {
          params: { path: { project, name } },
        }),
      ),
  });
  if (score.isPending) return null;
  if (score.isError || !Array.isArray(score.data?.parts)) {
    return (
      <p className="text-body text-fg-muted" role="status">
        {t("quality.score.unavailable")}
      </p>
    );
  }
  return <QualityScore score={score.data} id="endpoint-quality" />;
}
