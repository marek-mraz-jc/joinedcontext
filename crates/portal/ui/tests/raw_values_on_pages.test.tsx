/**
 * T-2756, UI-16: what a page shows is words, not the manifest's values: a runner input by its
 * technology's name, an entry of a list by what it is and its number, a layer's filters by label.
 */
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { inputLabel } from "../src/pages/datasources/DataSourcesPage";
import { layerSchema } from "../src/schemas/kinds";

const t = (key: string): string => i18n.t(key);

describe("a runner input as a person reads it (T-2756)", () => {
  it.each([
    ["sql_select", "SQL select"],
    ["nats", "NATS"],
    ["csv", "CSV"],
    ["aws_s3", "AWS S3"],
    ["gcp_pubsub", "GCP pubsub"],
    ["file_tail", "File tail"],
    ["amqp_0_9", "AMQP 0 9"],
    ["kafka", "Kafka"],
    ["", ""],
  ])("%s reads %s", (name, words) => {
    expect(inputLabel(name)).toBe(words);
  });
});

describe("an entry of a list (T-2756)", () => {
  function show(items: Record<string, unknown>) {
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<{ grants?: { name?: string }[] }>
          schema={{
            type: "object",
            properties: {
              grants: { type: "array", title: "Grants", items: { type: "object", ...items, properties: { name: { type: "string", title: "Name" } } } },
            },
          }}
          formData={{ grants: [{ name: "a" }, { name: "b" }] }}
          onSubmit={vi.fn()}
        />
      </I18nextProvider>,
    );
  }

  it("is numbered in words, never the list's name with a dash", async () => {
    await i18n.changeLanguage("en");
    show({});
    expect(screen.getByText(i18n.t("form.entry", { number: 1 }))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("form.entry", { number: 2 }))).toBeInTheDocument();
    expect(screen.queryByText(/Grants-\d/)).not.toBeInTheDocument();
  });

  it("is named by its own title when it has one", async () => {
    await i18n.changeLanguage("en");
    show({ title: "Grant" });
    expect(screen.getByText("Grant 2")).toBeInTheDocument();
  });
});

describe("a layer's filters (T-2756)", () => {
  it("are labelled in words, not by their NGSI-LD parameter", () => {
    const filter = (layerSchema(t, ["helsinki"], ["Bike"]) as { properties: { filter: { properties: Record<string, { title: string }> } } })
      .properties.filter.properties;
    expect(filter.q.title).toBe(en.pipelines.field.q);
    expect(filter.geoQ.title).toBe(en.pipelines.field.geoQ);
    for (const raw of ["q", "scopeQ", "geoQ"]) {
      expect(Object.values(filter).map((field) => field.title)).not.toContain(raw);
    }
  });
});
