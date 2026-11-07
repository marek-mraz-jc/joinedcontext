import type { Schema } from "@joinedcontext/sdk";

/** `Alert` as the `helsinki` endpoint serves its JSON Schema (seed/helsinki/helsinki.v1.schema.json), types and kinds only. */
export const SCHEMA: Schema = {
  "Alert": {
    "properties": {
      "address": {
        "type": [
          "string",
          "null"
        ],
        "x-ngsi-ld-kind": "Property"
      },
      "category": {
        "type": [
          "string",
          "null"
        ],
        "x-ngsi-ld-kind": "Property"
      },
      "dateIssued": {
        "type": [
          "string",
          "null"
        ],
        "format": "date-time",
        "x-ngsi-ld-kind": "Property"
      },
      "description": {
        "type": [
          "object",
          "null"
        ],
        "x-ngsi-ld-kind": "LanguageProperty"
      },
      "location": {
        "type": [
          "object",
          "null"
        ],
        "x-ngsi-ld-kind": "GeoProperty"
      },
      "name": {
        "type": [
          "object",
          "null"
        ],
        "x-ngsi-ld-kind": "LanguageProperty"
      },
      "observedAt": {
        "type": [
          "string",
          "null"
        ],
        "format": "date-time",
        "x-ngsi-ld-kind": "Property"
      },
      "source": {
        "type": [
          "string",
          "null"
        ],
        "format": "uri",
        "x-ngsi-ld-kind": "Property"
      },
      "subCategory": {
        "type": [
          "string",
          "null"
        ],
        "x-ngsi-ld-kind": "Property"
      },
      "validFrom": {
        "type": [
          "string",
          "null"
        ],
        "format": "date-time",
        "x-ngsi-ld-kind": "Property"
      },
      "validTo": {
        "type": [
          "string",
          "null"
        ],
        "format": "date-time",
        "x-ngsi-ld-kind": "Property"
      }
    },
    "required": []
  }
};
