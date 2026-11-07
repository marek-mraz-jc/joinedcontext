import { useMemo } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { Button, Card, PageFailed, PageLoading } from "../../components/ui";

/** The three review lanes a blueprint declares (CC-59, CC-63). */
const RISK_STYLES: Record<string, string> = {
  green: "bg-success/15 border-success/40",
  yellow: "bg-warning/20 border-warning/40",
  red: "bg-danger/15 border-danger/40",
};

export interface BlueprintSpec {
  version?: string;
  category?: string | null;
  riskClass?: string;
  allowedRoles?: string[];
  parameterSchema?: Record<string, unknown>;
}

export function blueprintSpec(blueprint: Manifest): BlueprintSpec {
  return blueprint.spec as BlueprintSpec;
}

function RiskBadge({ riskClass }: { riskClass?: string }): JSX.Element | null {
  const { t } = useTranslation();
  const key = (riskClass ?? "").toLowerCase();
  if (!RISK_STYLES[key]) {
    return null;
  }
  return (
    <span
      className={`rounded border px-2 py-0.5 text-caption ${RISK_STYLES[key]}`}
      // The colour is never the only carrier: the label says what the lane costs the user.
      title={t("flows.riskLabel")}
    >
      {t(`flows.risk.${key}`)}
    </span>
  );
}

/**
 * The ready-made set-ups of one category, as cards on the page that owns that kind of work (the
 * Data sources page shows `ingestion`; the Flows page that listed every category is gone, T-6096).
 * Only the blueprints this caller may run arrive: the server filters by `spec.allowedRoles`
 * (CC-59), so a card that is missing here is refused there too. Nobody may run one: no section.
 */
export function BlueprintCards({
  category,
  onRun,
}: {
  category: string;
  onRun: (blueprint: Manifest) => void;
}): JSX.Element | null {
  const { t, i18n } = useTranslation();

  const list = useQuery({
    queryKey: queryKeys.blueprints(),
    queryFn: async () => unwrap(await api.GET("/api/v1/blueprints", {})),
  });

  const shown = useMemo(
    () => asManifests(list.data?.items ?? []).filter((b) => blueprintSpec(b).category === category),
    [list.data, category],
  );

  if (list.isSuccess && shown.length === 0) {
    return null;
  }

  return (
    <section aria-labelledby="blueprints-title" className="space-y-2" data-testid="blueprint-cards">
      <h2 id="blueprints-title" className="text-body font-semibold">
        {t("datasources.setups.title")}
      </h2>
      <p className="text-caption text-fg-muted">{t("datasources.setups.lead")}</p>
      {list.isPending ? <PageLoading label={t("app.loading")} lines={2} /> : null}
      {list.isError ? (
        <PageFailed
          error={list.error}
          onRetry={() => {
            void list.refetch();
          }}
        />
      ) : null}
      <ul className="grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-4">
        {shown.map((blueprint) => {
          const spec = blueprintSpec(blueprint);
          const title = localized(blueprint.metadata.title, i18n.language, blueprint.metadata.name);
          return (
            <Card as="li" padding="md" key={blueprint.metadata.name} className="flex flex-col gap-2">
              <div className="flex items-start justify-between gap-2">
                <h3 className="font-semibold">{title}</h3>
                <RiskBadge riskClass={spec.riskClass} />
              </div>
              <p className="text-caption text-fg-muted">
                {localized(blueprint.metadata.description, i18n.language, "")}
              </p>
              {spec.version ? (
                <p className="text-caption text-fg-muted">{t("flows.version", { version: spec.version })}</p>
              ) : null}
              <Button
                size="sm"
                className="mt-auto self-start"
                onClick={() => {
                  onRun(blueprint);
                }}
              >
                {t("flows.run")}
              </Button>
            </Card>
          );
        })}
      </ul>
    </section>
  );
}
