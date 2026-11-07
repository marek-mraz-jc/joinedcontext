/**
 * What the public endpoint of `zilina-kpi` answers: the six indicators the pipeline ukazovatele
 * computed from the ŠÚ SR cubes recorded on 2026-10-06 (joinedcontext-deployment
 * tests/fixtures/zilina), its run time set to 19:00 UTC.
 */
export const INDICATORS = [
  {
    "calculationFormula": {
      "type": "Property",
      "value": "the published value of om7105rr/IN010082/SPOLU for the city and the newest year the cube carries"
    },
    "calculationPeriod": {
      "type": "Property",
      "value": {
        "end": "2025-12-31T23:59:59Z",
        "start": "2025-01-01T00:00:00Z"
      }
    },
    "computedBy": {
      "object": "urn:ngsi-ld:Pipeline:zilina.sk:zilina-kpi:ukazovatele",
      "type": "Relationship"
    },
    "currentValue": {
      "observedAt": "2026-03-31T00:00:00Z",
      "type": "Property",
      "unitCode": "C62",
      "value": -384
    },
    "derivedFrom": {
      "object": "urn:ngsi-ld:Endpoint:zilina.sk:zilina-mesto:zilina-mesto",
      "type": "Relationship"
    },
    "id": "urn:ngsi-ld:KeyPerformanceIndicator:zilina.sk:zilina-kpi:celkovy-prirastok-mesto",
    "name": {
      "type": "Property",
      "value": "celkovy-prirastok-mesto"
    },
    "type": "KeyPerformanceIndicator",
    "updatedAt": {
      "type": "Property",
      "value": {
        "@type": "DateTime",
        "@value": "2026-10-06T19:00:00Z"
      }
    }
  },
  {
    "calculationFormula": {
      "type": "Property",
      "value": "the published value of om7052rr/IN010087/SPOLU for the city and the newest year the cube carries"
    },
    "calculationPeriod": {
      "type": "Property",
      "value": {
        "end": "2025-12-31T23:59:59Z",
        "start": "2025-01-01T00:00:00Z"
      }
    },
    "computedBy": {
      "object": "urn:ngsi-ld:Pipeline:zilina.sk:zilina-kpi:ukazovatele",
      "type": "Relationship"
    },
    "currentValue": {
      "observedAt": "2026-03-31T00:00:00Z",
      "type": "Property",
      "unitCode": "P1",
      "value": 150.61
    },
    "derivedFrom": {
      "object": "urn:ngsi-ld:Endpoint:zilina.sk:zilina-mesto:zilina-mesto",
      "type": "Relationship"
    },
    "id": "urn:ngsi-ld:KeyPerformanceIndicator:zilina.sk:zilina-kpi:index-starnutia-mesto",
    "name": {
      "type": "Property",
      "value": "index-starnutia-mesto"
    },
    "type": "KeyPerformanceIndicator",
    "updatedAt": {
      "type": "Property",
      "value": {
        "@type": "DateTime",
        "@value": "2026-10-06T19:00:00Z"
      }
    }
  },
  {
    "calculationFormula": {
      "type": "Property",
      "value": "the sum of cr3803mr/U_CR_0005/VISIT_TOTAL over the twelve months of the newest year that carries all twelve"
    },
    "calculationPeriod": {
      "type": "Property",
      "value": {
        "end": "2025-12-31T23:59:59Z",
        "start": "2025-01-01T00:00:00Z"
      }
    },
    "computedBy": {
      "object": "urn:ngsi-ld:Pipeline:zilina.sk:zilina-kpi:ukazovatele",
      "type": "Relationship"
    },
    "currentValue": {
      "observedAt": "2026-09-11T00:00:00Z",
      "type": "Property",
      "unitCode": "C62",
      "value": 81291
    },
    "derivedFrom": {
      "object": "urn:ngsi-ld:Endpoint:zilina.sk:zilina-mesto:zilina-mesto",
      "type": "Relationship"
    },
    "id": "urn:ngsi-ld:KeyPerformanceIndicator:zilina.sk:zilina-kpi:navstevnici-rok-mesto",
    "name": {
      "type": "Property",
      "value": "navstevnici-rok-mesto"
    },
    "type": "KeyPerformanceIndicator",
    "updatedAt": {
      "type": "Property",
      "value": {
        "@type": "DateTime",
        "@value": "2026-10-06T19:00:00Z"
      }
    }
  },
  {
    "calculationFormula": {
      "type": "Property",
      "value": "the published value of om7101qr/IN010115/SPOLU for the city and the newest quarter the cube carries"
    },
    "calculationPeriod": {
      "type": "Property",
      "value": {
        "end": "2026-06-30T23:59:59Z",
        "start": "2026-04-01T00:00:00Z"
      }
    },
    "computedBy": {
      "object": "urn:ngsi-ld:Pipeline:zilina.sk:zilina-kpi:ukazovatele",
      "type": "Relationship"
    },
    "currentValue": {
      "observedAt": "2026-08-31T00:00:00Z",
      "type": "Property",
      "unitCode": "C62",
      "value": 79617
    },
    "derivedFrom": {
      "object": "urn:ngsi-ld:Endpoint:zilina.sk:zilina-mesto:zilina-mesto",
      "type": "Relationship"
    },
    "id": "urn:ngsi-ld:KeyPerformanceIndicator:zilina.sk:zilina-kpi:obyvatelstvo-stav-mesto",
    "name": {
      "type": "Property",
      "value": "obyvatelstvo-stav-mesto"
    },
    "type": "KeyPerformanceIndicator",
    "updatedAt": {
      "type": "Property",
      "value": {
        "@type": "DateTime",
        "@value": "2026-10-06T19:00:00Z"
      }
    }
  },
  {
    "calculationFormula": {
      "type": "Property",
      "value": "the published value of om7052rr/IN010089/SPOLU for the city and the newest year the cube carries"
    },
    "calculationPeriod": {
      "type": "Property",
      "value": {
        "end": "2025-12-31T23:59:59Z",
        "start": "2025-01-01T00:00:00Z"
      }
    },
    "computedBy": {
      "object": "urn:ngsi-ld:Pipeline:zilina.sk:zilina-kpi:ukazovatele",
      "type": "Relationship"
    },
    "currentValue": {
      "observedAt": "2026-03-31T00:00:00Z",
      "type": "Property",
      "unitCode": "ANN",
      "value": 44.03
    },
    "derivedFrom": {
      "object": "urn:ngsi-ld:Endpoint:zilina.sk:zilina-mesto:zilina-mesto",
      "type": "Relationship"
    },
    "id": "urn:ngsi-ld:KeyPerformanceIndicator:zilina.sk:zilina-kpi:priemerny-vek-mesto",
    "name": {
      "type": "Property",
      "value": "priemerny-vek-mesto"
    },
    "type": "KeyPerformanceIndicator",
    "updatedAt": {
      "type": "Property",
      "value": {
        "@type": "DateTime",
        "@value": "2026-10-06T19:00:00Z"
      }
    }
  },
  {
    "calculationFormula": {
      "type": "Property",
      "value": "the published value of pr5001rr/U15061 for the city and the newest year the cube carries"
    },
    "calculationPeriod": {
      "type": "Property",
      "value": {
        "end": "2025-12-31T23:59:59Z",
        "start": "2025-01-01T00:00:00Z"
      }
    },
    "computedBy": {
      "object": "urn:ngsi-ld:Pipeline:zilina.sk:zilina-kpi:ukazovatele",
      "type": "Relationship"
    },
    "currentValue": {
      "observedAt": "2026-05-29T00:00:00Z",
      "type": "Property",
      "unitCode": "C62",
      "value": 1708
    },
    "derivedFrom": {
      "object": "urn:ngsi-ld:Endpoint:zilina.sk:zilina-mesto:zilina-mesto",
      "type": "Relationship"
    },
    "id": "urn:ngsi-ld:KeyPerformanceIndicator:zilina.sk:zilina-kpi:uchadzaci-mesto",
    "name": {
      "type": "Property",
      "value": "uchadzaci-mesto"
    },
    "type": "KeyPerformanceIndicator",
    "updatedAt": {
      "type": "Property",
      "value": {
        "@type": "DateTime",
        "@value": "2026-10-06T19:00:00Z"
      }
    }
  }
];

/** The endpoint's answer to `?type=…`. */
export function answer(type: string | null): unknown[] {
  return type === "KeyPerformanceIndicator" ? INDICATORS : [];
}
