/**
 * A manifest as YAML, read only (T-6169): the row menu's "View YAML" on the grid pages. It shows
 * what the project's repository holds (apiVersion, kind, metadata, spec), never the Portal's
 * `status`. A manifest names a secret by `secretRef` only (non-negotiable), and a value under a
 * key that reads like a secret is still masked here, so a hand-written file that broke the rule
 * does not put the secret on a screen.
 */
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { stringify } from "yaml";
import type { Manifest } from "../../api/manifest";
import { CodeBlock, Dialog } from "../ui";
import { withoutUserinfo } from "../ui/safeHref";

const SECRET_KEY = /(password|passwd|secret|token|apikey|api_key|privatekey|private_key|credential)/i;
export const MASK = "********";

function masked(value: unknown, key: string): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => masked(item, key));
  }
  if (typeof value === "object" && value !== null) {
    return Object.fromEntries(Object.entries(value).map(([inner, v]) => [inner, masked(v, inner)]));
  }
  if (typeof value !== "string") return value;
  // `secretRef: {name, key}` names a secret; its strings are references, not values.
  if (SECRET_KEY.test(key) && !/ref$/i.test(key)) return MASK;
  // A password can also sit inside an address under any key (`dsn`, `url`): its userinfo is masked.
  const bare = withoutUserinfo(value);
  if (bare === value) return value;
  const scheme = /^[a-z][a-z0-9+.-]*:\/\//i.exec(value)?.[0] ?? "";
  return `${scheme}${MASK}@${bare.slice(scheme.length)}`;
}

/** The text the dialog shows. Exported for its test. */
export function manifestYaml(manifest: Manifest): string {
  const { status: _status, ...stored } = manifest as Manifest & { status?: unknown };
  void _status;
  return stringify(masked(stored, ""), { lineWidth: 0 });
}

export function ManifestYamlDialog({
  manifest,
  label,
  onClose,
}: {
  manifest: Manifest;
  label: string;
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Dialog
      open
      size="lg"
      closeLabel={t("app.close")}
      onOpenChange={(open) => (open ? undefined : onClose())}
      title={t("grid.yaml.title", { name: label })}
      description={t("grid.yaml.lead")}
    >
      <CodeBlock label={t("grid.yaml.title", { name: label })} className="max-h-96 rounded-md bg-surface-muted p-3 font-mono text-caption">
        {manifestYaml(manifest)}
      </CodeBlock>
    </Dialog>
  );
}
