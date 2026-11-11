# UiSchema

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `UiSchema`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/uischemas`
- Repository path: `portal/forms/{name}.uischema.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (UiSchemaSpec)

Desired specification of a `UiSchema` resource (UI-02).

| Field | Type | Required | Description |
|---|---|---|---|
| `fields` | map of [UiSchemaField](#uischemafield) | no | Per-field arrangement, keyed by the field's name in the schema. |
| `for` | string | yes | The kind whose form this arranges, written as the manifest writes it: `Endpoint`. |
| `groups` | list of [UiSchemaGroup](#uischemagroup) | no | Visual grouping of a flat schema, drawn as one fieldset each. |
| `order` | list of string | no | The fields in the order the form shows them; the rest follow in schema order. |

## UiSchemaField

How one field is drawn (UI-02, CC-29).

| Field | Type | Required | Description |
|---|---|---|---|
| `advanced` | boolean | no | Kept off the default form until the reader asks for the advanced mode (CC-29). |
| `columns` | integer (uint8) | no | Width in twelfths of the row, so `6` is half. |
| `help` | [Text](#text) | no | Help text shown with the field. |
| `placeholder` | string | no | Placeholder text of an empty input. |
| `readOnly` | boolean | no | Shown but not editable. |
| `widget` | string | no | A widget the Portal has registered; an unknown one degrades to the default input. |

## UiSchemaGroup

One fieldset of a form (UI-02).

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | [Text](#text) | no | Optional sentence under the legend. |
| `fields` | list of string | yes | The fields this group holds, in the order it holds them. |
| `title` | [Text](#text) | yes | The legend: one plain string, like every other human-facing string in a manifest (UI-50); the legacy language map still reads. |

## Text

A human label or description (UI-50): one plain string in the author's language. The legacy language map is still read, through release `v0.9`, and resolves to one string.

Type: string or [MultiLanguageMap](#multilanguagemap)

## MultiLanguageMap

Multi-language map keyed by ISO 639-1 two-letter lowercase language codes

Type: map of string
