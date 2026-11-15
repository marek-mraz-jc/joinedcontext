import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Field, Row, Schema } from "@joinedcontext/sdk";
import { EntityForm, parseInput } from "./EntityForm";
import { stubClient } from "@joinedcontext/sdk/testing";

describe("parseInput", () => {
  it("parses every input kind and returns error on invalid input", () => {
    const textBase: Field = { name: "desc", input: "text", required: false };
    expect(parseInput(textBase, "")).toEqual({ value: null });
    expect(parseInput(textBase, "  ")).toEqual({ value: null });
    expect(parseInput(textBase, "Hello")).toEqual({ value: "Hello" });

    const textPattern: Field = { name: "code", input: "text", pattern: "[A-Z]{3}", required: false };
    expect(parseInput(textPattern, "ABC")).toEqual({ value: "ABC" });
    expect(parseInput(textPattern, "abc")).toEqual({ error: "does not match the expected format" });

    const numberField: Field = { name: "count", input: "number", min: 0, max: 100, required: false };
    expect(parseInput(numberField, "42")).toEqual({ value: 42 });
    expect(parseInput(numberField, "0")).toEqual({ value: 0 });
    expect(parseInput(numberField, "not-a-number")).toEqual({ error: "must be a number" });
    expect(parseInput(numberField, "-1")).toEqual({ error: "must be at least 0" });
    expect(parseInput(numberField, "101")).toEqual({ error: "must be at most 100" });

    const checkboxField: Field = { name: "active", input: "checkbox", required: false };
    expect(parseInput(checkboxField, "true")).toEqual({ value: true });
    expect(parseInput(checkboxField, "false")).toEqual({ value: false });

    const geoField: Field = { name: "location", input: "geo", required: false };
    expect(parseInput(geoField, "60.15, 24.95")).toEqual({
      value: { type: "Point", coordinates: [24.95, 60.15] },
    });
    expect(parseInput(geoField, "60.15 24.95")).toEqual({
      value: { type: "Point", coordinates: [24.95, 60.15] },
    });
    expect(parseInput(geoField, "invalid")).toEqual({ error: 'must be "lat, lon"' });
    expect(parseInput(geoField, "100, 200")).toEqual({ error: 'must be "lat, lon"' });

    const selectField: Field = { name: "status", input: "select", options: [{ value: "open", title: "Open" }, { value: "closed" }], required: false };
    expect(parseInput(selectField, "open")).toEqual({ value: "open" });
    expect(parseInput(selectField, "unknown")).toEqual({ error: "must be one of Open, closed" });

    const dateField: Field = { name: "created", input: "date", required: false };
    expect(parseInput(dateField, "2025-05-14")).toEqual({ value: "2025-05-14" });
  });
});

describe("EntityForm component", () => {
  const schema: Schema = {
    Station: {
      properties: {
        name: { type: "string" },
        bikes: { type: "integer", minimum: 0 },
        status: { enum: ["open", "closed"] },
      },
      required: ["name"],
    },
  };

  it("create through stubClient writes the row and calls onSaved with minted urn", async () => {
    const client = stubClient({ schema });
    const onSaved = vi.fn();

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" onSaved={onSaved} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("name")).toBeInTheDocument();
    });

    fireEvent.change(screen.getByLabelText("Local id"), { target: { value: "stat-001" } });
    fireEvent.change(screen.getByLabelText("name"), { target: { value: "Central Station" } });
    fireEvent.change(screen.getByLabelText("bikes"), { target: { value: "15" } });
    fireEvent.change(screen.getByLabelText("status"), { target: { value: "open" } });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSaved).toHaveBeenCalled();
    });

    const minted = onSaved.mock.calls[0][0];
    expect(minted).toBe("urn:ngsi-ld:Station:example.org:demo:stat-001");

    const rows = client.transport.rows();
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({
      id: "urn:ngsi-ld:Station:example.org:demo:stat-001",
      type: "Station",
      name: "Central Station",
      bikes: 15,
      status: "open",
    });
  });

  it("update sends only changed fields", async () => {
    const row: Row = {
      id: "urn:ngsi-ld:Station:001",
      type: "Station",
      name: "Central Station",
      bikes: 10,
      status: "open",
    };
    const client = stubClient({ schema, entities: [row] });
    const onSaved = vi.fn();

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" row={row} onSaved={onSaved} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("bikes")).toHaveValue(10);
    });

    fireEvent.change(screen.getByLabelText("bikes"), { target: { value: "12" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSaved).toHaveBeenCalledWith("urn:ngsi-ld:Station:001");
    });

    const patchCall = client.transport.calls.find((c) => c.method === "PATCH");
    expect(patchCall).toBeDefined();
    expect(patchCall?.body).toEqual({
      bikes: { type: "Property", value: 12 },
    });
  });

  // A date, a yes or no, and a place each take what is typed and go out as one PATCH of the three.
  it("writes a date, a checkbox and a place as typed", async () => {
    const kinds: Schema = {
      Station: {
        properties: {
          opened: { type: "string", format: "date" },
          staffed: { type: "boolean" },
          location: { type: "object", "x-ngsi-ld-kind": "GeoProperty" },
        },
      },
    };
    const row: Row = { id: "urn:ngsi-ld:Station:002", type: "Station", opened: "2024-05-01", staffed: false, location: { type: "Point", coordinates: [24.95, 60.15] } };
    const client = stubClient({ schema: kinds, entities: [row] });
    const onSaved = vi.fn();
    render(
      <JcProvider client={client}>
        <EntityForm type="Station" row={row} onSaved={onSaved} />
      </JcProvider>,
    );
    await waitFor(() => expect(screen.getByLabelText("opened")).toHaveValue("2024-05-01"));
    fireEvent.change(screen.getByLabelText("opened"), { target: { value: "2024-06-01" } });
    fireEvent.click(screen.getByLabelText("staffed"));
    fireEvent.change(screen.getByLabelText("location"), { target: { value: "60.17, 24.94" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(row.id));
    expect(client.transport.calls.find((c) => c.method === "PATCH")?.body).toEqual({
      opened: { type: "Property", value: "2024-06-01" },
      staffed: { type: "Property", value: true },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.94, 60.17] } },
    });
  });

  it("required blank blocks the request and shows error", async () => {
    const client = stubClient({ schema });
    const onSaved = vi.fn();

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" onSaved={onSaved} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("name")).toBeInTheDocument();
    });

    fireEvent.change(screen.getByLabelText("bikes"), { target: { value: "5" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("name is required");
    expect(onSaved).not.toHaveBeenCalled();
    expect(client.transport.calls.filter((c) => c.method === "POST")).toHaveLength(0);
  });

  it("refuse fixture returning 403 says it is refused with role alert and keeps typed text", async () => {
    const row: Row = {
      id: "urn:ngsi-ld:Station:001",
      type: "Station",
      name: "Central Station",
      bikes: 10,
    };
    const client = stubClient({
      schema,
      entities: [row],
      refuse: (req) => {
        if (req.method === "PATCH") {
          return {
            status: 403,
            body: { title: "Forbidden", detail: "You are not allowed to update this station." },
          };
        }
        return null;
      },
    });

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" row={row} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("bikes")).toBeInTheDocument();
    });

    fireEvent.change(screen.getByLabelText("bikes"), { target: { value: "99" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    const alert = await screen.findByRole("alert");
    // A refusal is the SDK's sentence, and the App's own reason, a sentence, stays under it (T-4238).
    expect(alert).toHaveTextContent("You do not have the right to do this.");
    expect(alert).toHaveTextContent("You are not allowed to update this station.");
    expect(screen.getByLabelText("bikes")).toHaveValue(99);
  });

  it("disables a field with the reason when access document does not grant it", async () => {
    const row: Row = {
      id: "urn:ngsi-ld:Station:001",
      type: "Station",
      name: "Central Station",
      bikes: 10,
    };
    const client = stubClient({
      schema,
      entities: [row],
      access: {
        permissions: [
          {
            resource: { type: "Station" },
            actions: ["updateAttrs"],
            attributes: ["bikes"],
          },
        ],
        prohibitions: [],
      },
    });

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" row={row} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("name")).toBeDisabled();
    });

    expect(screen.getByLabelText("name")).toHaveAttribute(
      "title",
      "Your role may not change name of Station.",
    );
    expect(screen.getByLabelText("bikes")).not.toBeDisabled();
  });

  it("renders a select input for schema enum", async () => {
    const client = stubClient({ schema });

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("status")).toBeInTheDocument();
    });

    const select = screen.getByLabelText("status") as HTMLSelectElement;
    expect(select.tagName).toBe("SELECT");
    expect([...select.options].map((o) => o.value)).toEqual(["", "open", "closed"]);
  });

  // SDK-07, AP-62: a LanguageProperty is written whole, so an edit of one language keeps the rest.
  describe("a LanguageProperty", () => {
    const multilingual: Schema = {
      Alert: {
        properties: {
          name: { type: ["object", "null"], "x-ngsi-ld-kind": "LanguageProperty" },
          address: { type: ["string", "null"] },
        },
      },
    };
    const alert = {
      id: "urn:ngsi-ld:Alert:example.org:demo:1",
      type: "Alert",
      name: { languageMap: { fi: "Tietyö", sv: "Vägarbete", en: "Road work" } },
      address: "Mannerheimintie 12",
    } as unknown as Row;
    // The row a page holds: one language of the map, as the client's list reads it.
    const shown: Row = { ...alert, name: "Road work" };

    it("edits one language and sends one PATCH that keeps every other", async () => {
      const client = stubClient({ schema: multilingual, entities: [alert] });
      const onSaved = vi.fn();
      render(
        <JcProvider client={client}>
          <EntityForm type="Alert" row={shown} onSaved={onSaved} />
        </JcProvider>,
      );

      await waitFor(() => expect(screen.getByLabelText("name (fi)")).toHaveValue("Tietyö"));
      expect(screen.getByLabelText("name (sv)")).toHaveValue("Vägarbete");
      // A language typed back to what it held is no change of it.
      fireEvent.change(screen.getByLabelText("name (sv)"), { target: { value: "Vägarbete" } });
      fireEvent.change(screen.getByLabelText("name (en)"), { target: { value: "Resurfacing" } });
      await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled());
      fireEvent.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => expect(onSaved).toHaveBeenCalledWith(alert.id));
      const writes = client.transport.calls.filter((c) => c.method !== "GET");
      expect(writes).toHaveLength(1);
      expect(writes[0].body).toEqual({
        name: { type: "LanguageProperty", languageMap: { fi: "Tietyö", sv: "Vägarbete", en: "Resurfacing" } },
      });
    });

    it("leaves an unchanged map out of the patch", async () => {
      const client = stubClient({ schema: multilingual, entities: [alert] });
      const onSaved = vi.fn();
      render(
        <JcProvider client={client}>
          <EntityForm type="Alert" row={shown} onSaved={onSaved} />
        </JcProvider>,
      );

      await waitFor(() => expect(screen.getByLabelText("name (fi)")).toHaveValue("Tietyö"));
      fireEvent.change(screen.getByLabelText("address"), { target: { value: "Mannerheimintie 14" } });
      await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled());
      fireEvent.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => expect(onSaved).toHaveBeenCalled());
      const patch = client.transport.calls.find((c) => c.method === "PATCH");
      expect(patch?.body).toEqual({ address: { type: "Property", value: "Mannerheimintie 14" } });
    });

    it("creates one in the application's language", async () => {
      const client = stubClient({ schema: multilingual });
      const onSaved = vi.fn();
      render(
        <JcProvider client={client}>
          <EntityForm type="Alert" onSaved={onSaved} />
        </JcProvider>,
      );

      await waitFor(() => expect(screen.getByLabelText("name (en)")).toBeInTheDocument());
      expect(screen.queryByLabelText("name (fi)")).not.toBeInTheDocument();
      fireEvent.change(screen.getByLabelText("Local id"), { target: { value: "a2" } });
      fireEvent.change(screen.getByLabelText("name (en)"), { target: { value: "Market day" } });
      fireEvent.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => expect(onSaved).toHaveBeenCalled());
      const post = client.transport.calls.find((c) => c.method === "POST");
      expect((post?.body as Record<string, unknown>).name).toEqual({ type: "LanguageProperty", languageMap: { en: "Market day" } });
    });

    it("keeps Save disabled and says why when the languages cannot be read", async () => {
      const client = stubClient({
        schema: multilingual,
        entities: [alert],
        refuse: (request) =>
          request.method === "GET" && request.path.includes("/entities/urn")
            ? { status: 403, body: { title: "Forbidden", status: 403, detail: "no read of name" } }
            : null,
      });
      render(
        <JcProvider client={client}>
          <EntityForm type="Alert" row={shown} />
        </JcProvider>,
      );

      expect(await screen.findByText("You do not have the right to do this.")).toBeInTheDocument();
      expect(screen.queryByText(/no read of name/)).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
      expect(screen.getByLabelText("name (en)")).toBeDisabled();
      expect(client.transport.calls.filter((c) => c.method !== "GET")).toEqual([]);
    });
  });
});
