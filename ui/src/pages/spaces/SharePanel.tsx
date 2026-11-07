/**
 * `SharePanel` publishes one type of a space as a public link (T-3108, API/01 §33, ADR-N-042 §3.5):
 * a public Endpoint narrowed to that type and to the attributes the person chose, rendered by
 * `propose-endpoint` and proposed as one Change with its Policy. Apart from `PublicView` so the
 * anonymous page does not download the publisher's form (bundle budget, T-5621).
 */
import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import type { Change, ResourceProposal } from "../../api/manifest";
import { useProposal } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert, Button, Checkbox, Field, Input } from "../../components/ui";
import { DNS1123 } from "../../schemas/dns1123";
import { publicName } from "./publicName";

/** What `propose-endpoint` is asked: this type alone, public, the attributes not shown hidden. */
export function publishRequest(
  space: string,
  name: string,
  type: string,
  attributes: string[],
  shown: string[],
): Record<string, unknown> {
  return {
    contextSpace: space,
    name,
    audience: "public",
    entityTypes: [type],
    hiddenAttributes: attributes.filter((attr) => !shown.includes(attr)),
  };
}

interface Rendering {
  slug?: string;
  endpoint: ResourceProposal;
  policies: ResourceProposal[];
}

export function SharePanel({
  project,
  space,
  type,
  attributes,
  shownAtFirst,
  open,
}: {
  project: string;
  space: string;
  type: string;
  /** The type's attributes, from the model or the rows read. */
  attributes: string[];
  /** The attributes ticked at first: a grid view's visible fields (T-5186); every one without it. */
  shownAtFirst?: string[];
  /** Opened at once, as the grid toolbar's Share panel shows it. */
  open?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [name, setName] = useState(() => publicName(space, type));
  const [shown, setShown] = useState<string[]>(shownAtFirst ?? attributes);
  const [change, setChange] = useState<Change | null>(null);
  const [slug, setSlug] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const proposal = useProposal(project, "endpoints", setChange);
  const nameOk = new RegExp(DNS1123).test(name) && name.length <= 63;

  const publish = async () => {
    setFailed(null);
    try {
      const rendering = (await unwrap(
        await api.POST("/api/v1/projects/{project}/assistant/propose-endpoint", {
          params: { path: { project } },
          // The route documents its body as an object of no named members (API/01 §19).
          body: publishRequest(space, name, type, attributes, shown) as Record<string, never>,
        }),
      )) as unknown as Rendering;
      setSlug(rendering.slug ?? null);
      proposal.mutation.mutate({ body: rendering.endpoint, create: true, bundle: rendering.policies });
    } catch (error) {
      setFailed(error instanceof ApiError ? (error.problem?.detail ?? error.message) : String(error));
    }
  };

  return (
    <details className="rounded-lg border border-border p-3" data-testid="view-share" open={open}>
      <summary className="cursor-pointer text-body font-semibold text-fg">{t("spaces.share.title")}</summary>
      <div className="mt-2 flex flex-col gap-3">
        <p className="text-body text-fg-muted">{t("spaces.share.lead", { type })}</p>
        {shownAtFirst ? <p className="text-caption text-fg-muted">{t("spaces.share.viewNote")}</p> : null}
        <Field
          id="share-name"
          label={t("spaces.share.name")}
          help={t("spaces.share.nameHint")}
          errors={nameOk ? undefined : [t("spaces.share.nameHint")]}
        >
          <Input id="share-name" value={name} onChange={(event) => setName(event.target.value)} />
        </Field>
        <fieldset className="flex flex-wrap gap-x-3 gap-y-1">
          <legend className="text-caption font-semibold text-fg">{t("spaces.share.shown")}</legend>
          {attributes.map((attr) => (
            <Checkbox
              key={attr}
              label={attr}
              checked={shown.includes(attr)}
              onChange={(event) =>
                setShown(event.target.checked ? [...shown, attr] : shown.filter((one) => one !== attr))
              }
            />
          ))}
        </fieldset>
        {failed || proposal.error ? (
          <Alert role="alert" tone="danger">
            {failed ?? proposal.error}
          </Alert>
        ) : null}
        <Button
          className="w-fit"
          disabled={!nameOk || proposal.mutation.isPending}
          disabledReason={!nameOk ? t("spaces.share.nameHint") : t("app.loading")}
          onClick={() => void publish()}
        >
          {t("spaces.share.publish")}
        </Button>
        {change ? (
          <div className="flex flex-col gap-1">
            <ChangeNotice change={change} project={project} />
            {slug ? (
              <p className="text-body text-fg [overflow-wrap:anywhere]" data-testid="share-link">
                {t("spaces.share.link", { url: `${window.location.origin}/v/${slug}` })}
              </p>
            ) : null}
          </div>
        ) : null}
      </div>
    </details>
  );
}
