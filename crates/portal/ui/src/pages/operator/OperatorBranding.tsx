import { useState } from "react";
import type { FormEvent, JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError, reasonOf } from "../../api/client";
import { Alert, Badge, Button, buttonClass, Card, CardHeader, Field, FilePicker, Input } from "../../components/ui";
import {
  Swatch,
  svgImageUrl,
} from "../../components/forms/widgets/AppearanceWidgets";
import { proposeChange, sentence } from "./operatorApi";
import type { InstallationLook } from "./operatorApi";

const COLOURS = [
  "primary",
  "secondary",
  "accent",
  "background",
  "text",
] as const;
type Colour = (typeof COLOURS)[number];

/** The largest logo or favicon the server takes (jc-core's appearance rule). */
export const MAX_SVG_BYTES = 64 * 1024;

/**
 * Why an SVG file is not taken before it is sent, or nothing: the server judges it again (no
 * script, event handler or external reference), this only spares a round trip.
 */
export function svgRefusal(
  text: string,
  bytes: number,
): "size" | "type" | "script" | undefined {
  if (bytes > MAX_SVG_BYTES) return "size";
  if (!/^\s*(<\?xml[^>]*>\s*)?<svg[\s>]/i.test(text)) return "type";
  if (/<script|\son[a-z]+\s*=|javascript:/i.test(text)) return "script";
  return undefined;
}

/**
 * Administration → Installation → Branding and sign-in pages (T-7188): the installation's own
 * name, colours, logo and favicon over the branding file, on the Portal and the sign-in pages.
 * Saving asks for a `BrandingSet` operator Change; a second operator approves it, and the next page
 * shows it. The sign-in preview draws the sign-in card at desktop and phone width from the form.
 */
export function OperatorBranding({
  look,
  open,
}: {
  look?: InstallationLook;
  open?: string;
}): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [instanceName, setInstanceName] = useState(look?.instanceName ?? "");
  const [shortName, setShortName] = useState(look?.shortName ?? "");
  const [colours, setColours] = useState<Partial<Record<Colour, string>>>(() =>
    Object.fromEntries(COLOURS.flatMap((name) => (look?.colours?.[name] ? [[name, look.colours[name]]] : []))),
  );
  const [logoSvg, setLogoSvg] = useState(look?.logoSvg ?? "");
  const [faviconSvg, setFaviconSvg] = useState(look?.faviconSvg ?? "");
  const [fileErrors, setFileErrors] = useState<
    Partial<Record<"logoSvg" | "faviconSvg", string>>
  >({});
  const propose = useMutation({
    mutationFn: proposeChange,
    onSettled: () => client.invalidateQueries({ queryKey: ["operator"] }),
  });
  // The server names the field it refuses ("branding.logoSvg …"): shown under that field.
  const said = propose.error
    ? reasonOf(propose.error, t("operator.tryAgain"))
    : "";
  const fieldOf = (field: string): string[] | undefined => {
    const name = `branding.${field}`;
    const local = fileErrors[field as "logoSvg" | "faviconSvg"];
    if (local) return [local];
    const lines =
      propose.error instanceof ApiError
        ? (propose.error.problem?.errors ?? [])
        : [];
    const line = [...lines, said].find((text) => text.startsWith(name));
    return line
      ? [sentence(line.replace(/^(branding\.[\w.]+:?\s*)+/, ""))]
      : undefined;
  };
  const fields = [
    "instanceName",
    "shortName",
    "logoSvg",
    "faviconSvg",
    ...COLOURS.map((c) => `colours.${c}`),
  ];
  const elsewhere = propose.isError && !fields.some((field) => fieldOf(field));

  const wanted = (): InstallationLook => {
    const set = Object.fromEntries(
      COLOURS.flatMap((name) =>
        colours[name]?.trim() ? [[name, colours[name]?.trim()]] : [],
      ),
    );
    return {
      ...(instanceName.trim() ? { instanceName: instanceName.trim() } : {}),
      ...(shortName.trim() ? { shortName: shortName.trim() } : {}),
      ...(Object.keys(set).length ? { colours: set } : {}),
      ...(logoSvg ? { logoSvg } : {}),
      ...(faviconSvg ? { faviconSvg } : {}),
    };
  };
  const saved: InstallationLook = {
    ...(look?.instanceName ? { instanceName: look.instanceName } : {}),
    ...(look?.shortName ? { shortName: look.shortName } : {}),
    ...(look?.colours && Object.keys(look.colours).length
      ? { colours: look.colours }
      : {}),
    ...(look?.logoSvg ? { logoSvg: look.logoSvg } : {}),
    ...(look?.faviconSvg ? { faviconSvg: look.faviconSvg } : {}),
  };
  const changed = JSON.stringify(wanted()) !== JSON.stringify(saved);

  const onFile =
    (field: "logoSvg" | "faviconSvg", set: (svg: string) => void) =>
    async (file: File) => {
      const text = await file.text();
      const refused = svgRefusal(text, file.size);
      setFileErrors((errors) => ({
        ...errors,
        [field]: refused
          ? t(`operator.installation.branding.file.${refused}`)
          : undefined,
      }));
      if (!refused) set(text.trim());
    };
  const onSave = (event: FormEvent) => {
    event.preventDefault();
    propose.mutate({ kind: "BrandingSet", branding: wanted() });
  };
  const logoUrl = svgImageUrl(logoSvg);
  const faviconUrl = svgImageUrl(faviconSvg);

  return (
    <>
      <Card>
        <CardHeader
          title={t("operator.installation.branding.title")}
          description={t("operator.installation.branding.lead")}
          actions={
            open ? (
              <Badge tone="info">
                {t("operator.installation.email.waitingBadge")}
              </Badge>
            ) : (
              <Badge tone={look ? "success" : "neutral"}>
                {look
                  ? t("operator.installation.branding.set")
                  : t("operator.installation.branding.unset")}
              </Badge>
            )
          }
        />
        {open ? (
          <Alert tone="info" className="mt-4">
            {t("operator.installation.branding.waiting")}{" "}
            <Link
              to="/organization/$tab"
              params={{ tab: "operator-changes" }}
              className="underline"
            >
              {t("operator.installation.email.toChanges")}
            </Link>
          </Alert>
        ) : null}
        {!look && !open ? (
          <p className="mt-4 text-body text-fg-muted">
            {t("operator.installation.branding.empty")}
          </p>
        ) : null}
        <form
          className="mt-4 grid max-w-3xl gap-4 sm:grid-cols-2"
          onSubmit={onSave}
          aria-label={t("operator.installation.branding.title")}
        >
          <fieldset className="contents" disabled={Boolean(open)}>
            <Field
              id="brand-name"
              label={t("operator.installation.branding.instanceName")}
              help={t("operator.installation.branding.hint.instanceName")}
              errors={fieldOf("instanceName")}
            >
              <Input
                id="brand-name"
                value={instanceName}
                onChange={(e) => setInstanceName(e.target.value)}
                maxLength={120}
                autoComplete="off"
              />
            </Field>
            <Field
              id="brand-short"
              label={t("operator.installation.branding.shortName")}
              help={t("operator.installation.branding.hint.shortName")}
              errors={fieldOf("shortName")}
            >
              <Input
                id="brand-short"
                value={shortName}
                onChange={(e) => setShortName(e.target.value)}
                maxLength={120}
                autoComplete="off"
              />
            </Field>
            {COLOURS.map((name) => (
              <Field
                key={name}
                id={`brand-colour-${name}`}
                label={t(`operator.installation.branding.colour.${name}`)}
                help={t("operator.installation.branding.hint.colour")}
                errors={fieldOf(`colours.${name}`)}
                aside={
                  <Swatch
                    colour={colours[name]}
                    className="h-control-xs w-control-xs"
                  />
                }
              >
                <Input
                  id={`brand-colour-${name}`}
                  value={colours[name] ?? ""}
                  onChange={(e) =>
                    setColours((c) => ({ ...c, [name]: e.target.value }))
                  }
                  placeholder="#rrggbb"
                  maxLength={7}
                  spellCheck={false}
                  autoComplete="off"
                />
              </Field>
            ))}
            {(
              [
                ["logoSvg", logoUrl, setLogoSvg],
                ["faviconSvg", faviconUrl, setFaviconSvg],
              ] as const
            ).map(([field, url, set]) => (
              <Field
                key={field}
                id={`brand-${field}`}
                label={t(`operator.installation.branding.${field}`)}
                help={t(field === "logoSvg" ? "operator.installation.branding.hint.svg" : "operator.installation.branding.hint.favicon")}
                errors={fieldOf(field)}
                aside={
                  <span className="flex items-center gap-2">
                    {url ? (
                      <img
                        src={url}
                        alt={t(`operator.installation.branding.${field}`)}
                        className="h-10 max-w-32 rounded border border-border bg-surface object-contain p-1"
                      />
                    ) : (
                      <span className="text-caption text-fg-muted">
                        {t("operator.installation.branding.fromFile")}
                      </span>
                    )}
                    {url ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => set("")}
                      >
                        {t("operator.installation.branding.removeFile")}
                      </Button>
                    ) : null}
                  </span>
                }
              >
                <FilePicker
                  id={`brand-${field}`}
                  label={t(`operator.installation.branding.${field}`)}
                  accept=".svg,image/svg+xml"
                  onFile={(file) => void onFile(field, set)(file)}
                  className={buttonClass("secondary", "sm", open ? "pointer-events-none opacity-50" : undefined)}
                >
                  {t("operator.installation.branding.chooseFile")}
                </FilePicker>
              </Field>
            ))}
          </fieldset>
          <div className="flex flex-wrap items-center gap-3 sm:col-span-2">
            <Button
              type="submit"
              loading={propose.isPending}
              disabled={Boolean(open) || !changed}
              disabledReason={
                open
                  ? t("operator.installation.email.waitingShort")
                  : t("operator.installation.branding.unchanged")
              }
            >
              {t("operator.installation.branding.propose")}
            </Button>
            {!changed && !open ? <p className="text-caption text-fg-muted">{t("operator.installation.branding.unchanged")}</p> : null}
          </div>
        </form>
        {elsewhere ? (
          <Alert tone="danger" className="mt-4">
            {sentence(said)}
          </Alert>
        ) : null}
        {propose.isSuccess && !open ? (
          <Alert tone="success" className="mt-4">
            {t("operator.installation.email.proposed")}{" "}
            <Link
              to="/organization/$tab"
              params={{ tab: "operator-changes" }}
              className="underline"
            >
              {t("operator.installation.email.toChanges")}
            </Link>
          </Alert>
        ) : null}
      </Card>
      <Card>
        <CardHeader
          title={t("operator.installation.signIn.title")}
          description={t("operator.installation.signIn.lead")}
        />
        <div className="mt-4 flex flex-wrap items-start gap-6">
          {(["desktop", "phone"] as const).map((size) => (
            <figure key={size} className={size === "desktop" ? "w-full max-w-xl space-y-2" : "space-y-2"}>
              <SignInPreview
                size={size}
                name={
                  instanceName.trim() ||
                  t("operator.installation.signIn.defaultName")
                }
                logoUrl={logoUrl}
                colours={colours}
              />
              <figcaption className="text-caption text-fg-muted">
                {t(`operator.installation.signIn.${size}`)}
              </figcaption>
            </figure>
          ))}
        </div>
      </Card>
    </>
  );
}

const HEX = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;

/** The sign-in card as kaisatsu draws it, scaled: 1440 px wide for desktop, 400 px for a phone. */
function SignInPreview({
  size,
  name,
  logoUrl,
  colours,
}: {
  size: "desktop" | "phone";
  name: string;
  logoUrl?: string;
  colours: Partial<Record<Colour, string>>;
}): JSX.Element {
  const { t } = useTranslation();
  const paint = (colour: Colour, fallback: string) =>
    HEX.test(colours[colour]?.trim() ?? "")
      ? (colours[colour]?.trim() as string)
      : fallback;
  // The page at its real width, scaled into the frame: the card keeps kaisatsu's 360 px.
  // Desktop shows the middle of a 1440 px page, so the card is large enough to read.
  const [width, height] = size === "desktop" ? [960, 600] : [400, 800];
  const x = (width - 360) / 2;
  return (
    <svg
      role="img"
      aria-label={t("operator.installation.signIn.previewOf", { which: t(`operator.installation.signIn.${size}`) })}
      viewBox={`0 0 ${width} ${height}`}
      className={`${size === "desktop" ? "w-full" : "w-56"} h-auto rounded-md border border-border`}
    >
      <rect width={width} height={height} fill={paint("background", "#f5f7fa")} />
      <rect x={x} y={height / 2 - 200} width={360} height={400} rx={12} fill="#ffffff" />
      {logoUrl ? <image href={logoUrl} x={x + 100} y={height / 2 - 176} width={160} height={56} /> : null}
      <text x={width / 2} y={height / 2 - 80} textAnchor="middle" fontSize={22} fontWeight={600} fill={paint("text", "#1f2933")}>
        {name.length > 28 ? `${name.slice(0, 27)}…` : name}
      </text>
      <rect x={x + 30} y={height / 2 - 40} width={300} height={44} rx={6} fill="none" stroke="#c4cdd5" />
      <rect x={x + 30} y={height / 2 + 20} width={300} height={44} rx={6} fill="none" stroke="#c4cdd5" />
      <rect x={x + 30} y={height / 2 + 100} width={300} height={48} rx={6} fill={paint("primary", "#0b5394")} />
    </svg>
  );
}
