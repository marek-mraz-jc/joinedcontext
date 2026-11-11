import { useContext } from "react";
import type { JSX } from "react";
import type { WidgetProps } from "@rjsf/utils";
import { useTranslation } from "react-i18next";
import { CheckboxesWidget } from "../theme";
import { FormDataContext } from "./EntitySelectorField";

/**
 * `spec.enabledRepresentations` as checkboxes. MCP on a public Endpoint is the person's switch
 * (EP-24, T-6761): unlocked there and locked on an internal one, decided from the form's data
 * because rjsf keeps the uiSchema it validated with (T-2322), and the line under the boxes says
 * whether anonymous AI clients read the Endpoint and how to change it.
 */
export function RepresentationCheckboxes(props: WidgetProps): JSX.Element {
  const { t } = useTranslation();
  const root = useContext(FormDataContext) as { audience?: unknown } | undefined;
  const isPublic = root?.audience === "public";
  const locked = (props.options.enumDisabled ?? []).filter((rep) => rep !== "mcp");
  const enumDisabled = isPublic ? locked : [...locked, "mcp"];
  const on = Array.isArray(props.value) && props.value.includes("mcp");
  return (
    <>
      <CheckboxesWidget {...props} options={{ ...props.options, enumDisabled }} />
      {isPublic ? (
        <p id={`${props.id}__mcp`} data-testid="public-mcp-hint" className="mt-2 text-caption text-fg-muted">
          {t(on ? "endpoints.publicMcpOn" : "endpoints.publicMcpHint")}
        </p>
      ) : null}
    </>
  );
}
