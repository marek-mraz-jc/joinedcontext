/** T-7589: coverage for DashboardJsonMonaco line 70 and component lifecycle edge cases. */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render } from "@testing-library/react";
import DashboardJsonMonaco from "../src/pages/dashboards/DashboardJsonMonaco";

let mockEditor: { getValue: () => string; setValue: ReturnType<typeof vi.fn> } | null = null;
let triggerChange: ((val: string | undefined) => void) | null = null;

vi.mock("@monaco-editor/react", async () => {
  const { useEffect, useRef } = await vi.importActual<typeof import("react")>("react");
  return {
    default: function FakeEditor(props: {
      defaultValue?: string;
      onChange?: (val: string | undefined) => void;
      onMount?: (editor: unknown) => void;
      options?: { ariaLabel?: string };
    }) {
      triggerChange = props.onChange ?? null;
      const { onMount } = props;
      const mounted = useRef(false);
      useEffect(() => {
        if (mockEditor && !mounted.current) {
          mounted.current = true;
          onMount?.(mockEditor);
        }
      }, [onMount]);
      return (
        <div data-testid="fake-editor" aria-label={props.options?.ariaLabel}>
          <button data-testid="trigger-change" onClick={() => props.onChange?.("user-input")}>
            Change
          </button>
        </div>
      );
    },
  };
});
vi.mock("../src/pages/models/monaco-setup", () => ({}));
const setDiagnostics = vi.fn();
vi.mock("monaco-editor/languages/features/json/register", () => ({
  jsonDefaults: { setDiagnosticsOptions: (opts: unknown) => setDiagnostics(opts) },
}));

describe("DashboardJsonMonaco", () => {
  afterEach(() => {
    vi.clearAllMocks();
    mockEditor = null;
    triggerChange = null;
  });

  it("calls mounted.setValue(value) on mount if mounted.getValue() differs from value (line 70)", () => {
    const setValueMock = vi.fn();
    mockEditor = {
      getValue: () => "different-initial-value",
      setValue: setValueMock,
    };

    render(
      <DashboardJsonMonaco
        value='{"kind": "Dashboard"}'
        onChange={vi.fn()}
        schema={null}
        ariaLabel="Manifest"
        height="200px"
      />,
    );

    expect(setValueMock).toHaveBeenCalledWith('{"kind": "Dashboard"}');
  });

  it("does not call mounted.setValue on mount if mounted.getValue() matches value", () => {
    const setValueMock = vi.fn();
    mockEditor = {
      getValue: () => '{"kind": "Dashboard"}',
      setValue: setValueMock,
    };

    render(
      <DashboardJsonMonaco
        value='{"kind": "Dashboard"}'
        onChange={vi.fn()}
        schema={null}
        ariaLabel="Manifest"
        height="200px"
      />,
    );

    expect(setValueMock).not.toHaveBeenCalled();
  });

  it("tracks onChange and emitted values, handling undefined input correctly", () => {
    const onChangeMock = vi.fn();
    const setValueMock = vi.fn();
    mockEditor = {
      getValue: () => "current",
      setValue: setValueMock,
    };

    render(
      <DashboardJsonMonaco
        value="current"
        onChange={onChangeMock}
        schema={{ type: "object" }}
        ariaLabel="Manifest"
        height="200px"
      />,
    );

    act(() => {
      triggerChange?.("first-edit");
    });
    expect(onChangeMock).toHaveBeenCalledWith("first-edit");

    act(() => {
      triggerChange?.(undefined);
    });
    expect(onChangeMock).toHaveBeenCalledWith("");
  });

  it("updates value from outside via useEffect when not in emitted cache", () => {
    const setValueMock = vi.fn();
    mockEditor = {
      getValue: () => "old-value",
      setValue: setValueMock,
    };

    const { rerender } = render(
      <DashboardJsonMonaco
        value="old-value"
        onChange={vi.fn()}
        schema={null}
        ariaLabel="Manifest"
        height="200px"
      />,
    );

    rerender(
      <DashboardJsonMonaco
        value="new-outside-value"
        onChange={vi.fn()}
        schema={null}
        ariaLabel="Manifest"
        height="200px"
      />,
    );

    expect(setValueMock).toHaveBeenCalledWith("new-outside-value");
  });
});
