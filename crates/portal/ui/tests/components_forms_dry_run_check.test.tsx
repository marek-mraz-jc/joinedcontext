/**
 * T-7585: the Check of a form dialog: the Endpoints a Policy's verdict says it binds (only names are
 * kept), a Check pressed where checking is not allowed asks nothing, and a YAML that does not read
 * asks nothing either.
 */
import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import type { JsonSchema } from "../src/components/forms/types";
import { useDryRunCheck } from "../src/components/forms/useDryRunCheck";

const odata = vi.hoisted(() => ({ proposeManifest: vi.fn() }));
vi.mock("../src/api/odata", async (original) => ({
  ...(await original<typeof import("../src/api/odata")>()),
  ...odata,
}));

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </QueryClientProvider>
  );
}

type Form = { name: string };
const SCHEMA: JsonSchema = { type: "object", properties: { name: { type: "string", title: "Name" } } };

const options = {
  kind: "Policy",
  schema: SCHEMA,
  formView: true,
  currentDigest: "d1",
  verdict: null,
  updateVerdict: () => undefined,
  canCheck: true,
  disabled: false,
  autoCheck: false,
  project: "helsinki",
  plural: "policies",
  readYaml: () => null,
  onYamlChecked: () => undefined,
  advancedFolded: false,
  showAdvanced: () => undefined,
  advanced: false,
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  odata.proposeManifest.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the check of a Policy", () => {
  it("keeps the names of the Endpoints it binds and drops anything that is not a name", async () => {
    odata.proposeManifest.mockResolvedValue({ verdict: null, bindsEndpoints: ["air-open", 7, null, "bikes"] });
    const formData: Form = { name: "open-read" };
    const { result } = renderHook(() => useDryRunCheck<Form>({ ...options, formData }), { wrapper });
    act(() => result.current.runCheck());
    await waitFor(() => expect(result.current.bindsEndpoints).toEqual(["air-open", "bikes"]));
    expect(odata.proposeManifest).toHaveBeenCalledWith("helsinki", "policies", { name: "open-read" }, {
      create: true,
      dryRun: true,
    });
  });

  it("asks nothing where checking is not allowed", () => {
    const formData: Form = { name: "open-read" };
    const { result } = renderHook(() => useDryRunCheck<Form>({ ...options, formData, canCheck: false }), { wrapper });
    act(() => result.current.runCheck());
    expect(odata.proposeManifest).not.toHaveBeenCalled();
    expect(result.current.stoppedMessage).toBeNull();
  });

  it("asks nothing for a YAML that did not read", () => {
    const onYamlChecked = vi.fn();
    const readYaml = vi.fn(() => null);
    const { result } = renderHook(() => useDryRunCheck<Form>({ ...options, formView: false, readYaml, onYamlChecked }), { wrapper });
    act(() => result.current.runCheck());
    expect(readYaml).toHaveBeenCalled();
    expect(onYamlChecked).not.toHaveBeenCalled();
    expect(odata.proposeManifest).not.toHaveBeenCalled();
  });

  it("shows the advanced fields when the empty required field is among them, and asks nothing yet", () => {
    const showAdvanced = vi.fn();
    const schema: JsonSchema = { type: "object", required: ["name"], properties: { name: { type: "string", title: "Name" } } };
    const formData = {} as Form;
    const { result } = renderHook(
      () => useDryRunCheck<Form>({ ...options, schema, formData, advancedFolded: true, showAdvanced }),
      { wrapper },
    );
    act(() => result.current.runCheck());
    expect(result.current.stoppedMessage).toBe(i18n.t("form.missing", { field: "Name" }));
    expect(showAdvanced).toHaveBeenCalledTimes(1);
    expect(odata.proposeManifest).not.toHaveBeenCalled();
  });
});
