import { useState } from "react";
import { act, projectBound } from "../../api/odata";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import { Button, Field, Select } from "../../components/ui";
import { PRESET_KINDS } from "../../schemas/kinds";
import type { PresetKind } from "../../schemas/kinds";

/** What the server listed for a preset (`ops/datasource_discover.rs`), or why it listed nothing. */
export interface Discovery {
  field: "organization" | "layer" | "entitySet" | "typeNames" | "collection" | "thing";
  url: string;
  choices: Array<{ value: string; label: string }>;
  truncated: boolean;
  skipped?: string;
}

type Http = Record<string, unknown>;

/** `listing` without its query: the address a pick is written under. */
function base(listing: string): string {
  const url = new URL(listing);
  url.search = "";
  return url.toString().replace(/\/$/, "");
}

/**
 * The `spec.http` block with `value` of a discovery written in: an organization or a feature type
 * into the preset, a layer, entity set, collection or Thing into the URL the source reads.
 */
export function applied(http: Http, discovery: Pick<Discovery, "field" | "url">, value: string): Http {
  const preset = (http.preset ?? {}) as Record<string, unknown>;
  const at = encodeURIComponent(value);
  switch (discovery.field) {
    case "organization":
      return { ...http, preset: { ...preset, organization: value } };
    case "typeNames":
      return { ...http, preset: { ...preset, typeNames: value } };
    case "layer":
      return { ...http, url: `${base(discovery.url)}/${at}/query` };
    case "entitySet":
      return { ...http, url: `${base(discovery.url)}/${at}` };
    case "collection":
      return { ...http, url: `${base(discovery.url)}/${at}/items` };
    case "thing":
      return { ...http, url: `${base(discovery.url)}(${at})/Datastreams` };
  }
}

function presetKind(http: Http | undefined): PresetKind | undefined {
  const kind = (http?.preset as { kind?: unknown } | undefined)?.kind;
  return (PRESET_KINDS as readonly unknown[]).includes(kind) ? (kind as PresetKind) : undefined;
}

/**
 * Find in the service (T-7076, G-08): the service's own list of what a preset can read, read on
 * the project's runner, offered as a choice the person writes into the form with Use.
 */
export function PresetDiscovery({
  project,
  http,
  onUse,
  inline = false,
}: {
  project: string;
  http: Http | undefined;
  onUse: (http: Http) => void;
  /** Inside the new source wizard, whose step heading names it: no heading or landmark of its own. */
  inline?: boolean;
}): JSX.Element {
  const prefix = inline ? "wizard-discovery" : "preset-discovery";
  const { t } = useTranslation();
  const Wrapper = inline ? "div" : "section";
  const kind = presetKind(http);
  const url = typeof http?.url === "string" ? http.url.trim() : "";
  const [picked, setPicked] = useState("");
  const [used, setUsed] = useState<string | null>(null);
  const find = useMutation({
    mutationFn: async (asked: { kind: PresetKind; url: string }) =>
      (await act("jc_datasource_discover", projectBound(project), asked)) as Discovery,
    onSuccess: (answer) => {
      setPicked(answer.choices[0]?.value ?? "");
      setUsed(null);
    },
  });
  // An answer belongs to the kind and URL it was asked for; another one in the form hides it.
  const answer =
    find.data && find.variables?.kind === kind && find.variables?.url === url ? find.data : undefined;
  // A refusal says what to do next beside the server's words.
  const failure =
    find.error instanceof ApiError
      ? [find.error.problem?.detail ?? find.error.message, find.error.status === 403 ? t("datasources.discover.refused") : null]
          .filter(Boolean)
          .join(" ")
      : find.error
        ? t("app.error.generic")
        : null;

  return (
    <Wrapper
      {...(inline ? {} : { "aria-labelledby": `${prefix}-heading` })}
      className="flex flex-col gap-2"
      data-testid={prefix}
    >
      {inline ? null : (
        <h2 id={`${prefix}-heading`} className="text-body font-medium">
          {t("datasources.discover.title")}
        </h2>
      )}
      {kind && url ? <p className="text-caption text-fg-muted">{t("datasources.discover.hint")}</p> : null}
      {kind && url ? (
        <div>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            loading={find.isPending}
            onClick={() => find.mutate({ kind, url })}
          >
            {find.isPending ? t("datasources.discover.running") : t("datasources.discover.button")}
          </Button>
        </div>
      ) : (
        <p className="text-caption text-fg-muted">{t("datasources.discover.needs")}</p>
      )}
      <div role="status" aria-live="polite" className="mt-1 flex flex-col gap-2">
        {failure ? <p className="text-body font-medium text-danger">{failure}</p> : null}
        {answer?.skipped ? (
          <p className="text-body font-medium text-danger">{t("datasources.discover.failed", { reason: answer.skipped })}</p>
        ) : answer && answer.choices.length === 0 ? (
          <p className="text-body">{t("datasources.discover.none")}</p>
        ) : answer ? (
          <>
            <Field
              id={`${prefix}-choice`}
              label={`${t(`datasources.discover.fields.${answer.field}`)} · ${t("datasources.discover.pick", { count: answer.choices.length })}`}
              description={t("datasources.discover.pickHint")}
            >
              <div className="flex flex-wrap items-center gap-2">
                <Select
                  id={`${prefix}-choice`}
                  className="min-w-0 flex-1"
                  value={picked}
                  onChange={(event) => setPicked(event.target.value)}
                >
                  {answer.choices.map((choice) => (
                    <option key={choice.value} value={choice.value}>
                      {choice.label === choice.value ? choice.value : `${choice.label} (${choice.value})`}
                    </option>
                  ))}
                </Select>
                <Button
                  type="button"
                  variant="primary"
                  size="sm"
                  disabled={picked === "" || !http}
                  title={picked === "" || !http ? t("datasources.discover.useNeeds") : undefined}
                  onClick={() => {
                    if (!http) return;
                    onUse(applied(http, answer, picked));
                    setUsed(picked);
                  }}
                >
                  {t("datasources.discover.use")}
                </Button>
              </div>
            </Field>
            {answer.truncated ? <p className="text-caption text-fg-muted">{t("datasources.discover.truncated")}</p> : null}
            {used ? (
              <p className="text-body font-medium text-success">
                ✓ {t(inline ? "datasources.wizard.picked" : "datasources.discover.used", { value: used })}
              </p>
            ) : null}
          </>
        ) : null}
      </div>
    </Wrapper>
  );
}
