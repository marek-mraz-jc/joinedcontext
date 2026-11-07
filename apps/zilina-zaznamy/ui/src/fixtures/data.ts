/**
 * What the app's three endpoints answer: the entities the Žilina pipelines wrote from the feeds
 * recorded on 2026-10-06 (joinedcontext-deployment tests/fixtures/zilina): five monuments, the
 * five stations and the air station of `zilina-verejne`, and eight works of `zilina-uniza` (one
 * per kind and language).
 */
export const MONUMENTS = [
  {
    "architecturalStyle": {
      "type": "Property",
      "value": "klasicizmus"
    },
    "cadastralArea": {
      "type": "Property",
      "value": "Bánová"
    },
    "constructionPeriod": {
      "type": "Property",
      "value": "1821"
    },
    "id": "urn:ngsi-ld:PointOfInterest:zilina.sk:zilina-verejne:uzpf-1317-2",
    "monumentKind": {
      "type": "Property",
      "value": "SÚSOŠIE"
    },
    "monumentNumber": {
      "type": "Property",
      "value": "1317/2"
    },
    "name": {
      "languageMap": {
        "sk": "Trojičný stĺp"
      },
      "type": "LanguageProperty"
    },
    "ownershipForm": {
      "type": "Property",
      "value": "Vlastníctvo cirkvi a cirk. organizácií"
    },
    "type": "PointOfInterest"
  },
  {
    "address": {
      "type": "Property",
      "value": "Mariánske námestie 158/23, Žilina"
    },
    "architecturalStyle": {
      "type": "Property",
      "value": "barok"
    },
    "cadastralArea": {
      "type": "Property",
      "value": "Žilina"
    },
    "constructionPeriod": {
      "type": "Property",
      "value": "1743"
    },
    "id": "urn:ngsi-ld:PointOfInterest:zilina.sk:zilina-verejne:uzpf-1395-1",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.7388850378545,
          49.2230372277457
        ],
        "type": "Point"
      }
    },
    "monumentKind": {
      "type": "Property",
      "value": "KLÁŠTOR JEZUITOV"
    },
    "monumentNumber": {
      "type": "Property",
      "value": "1395/1"
    },
    "name": {
      "languageMap": {
        "sk": "jezitský kláštor"
      },
      "type": "LanguageProperty"
    },
    "ownershipForm": {
      "type": "Property",
      "value": "Vlastníctvo cirkvi a cirk. organizácií"
    },
    "type": "PointOfInterest"
  },
  {
    "address": {
      "type": "Property",
      "value": "Námestie sv. Jána Bosca 1/87, Žilina"
    },
    "architecturalStyle": {
      "type": "Property",
      "value": "klasicizmus"
    },
    "cadastralArea": {
      "type": "Property",
      "value": "Bánová"
    },
    "constructionPeriod": {
      "type": "Property",
      "value": "1.pol.19.st."
    },
    "id": "urn:ngsi-ld:PointOfInterest:zilina.sk:zilina-verejne:uzpf-1316-1",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.7212848,
          49.2023
        ],
        "type": "Point"
      }
    },
    "monumentKind": {
      "type": "Property",
      "value": "KAŠTIEĽ"
    },
    "monumentNumber": {
      "type": "Property",
      "value": "1316/1"
    },
    "name": {
      "languageMap": {
        "sk": "kaštieľ"
      },
      "type": "LanguageProperty"
    },
    "ownershipForm": {
      "type": "Property",
      "value": "Súkromné vlastníctvo občanov"
    },
    "type": "PointOfInterest"
  },
  {
    "address": {
      "type": "Property",
      "value": "Hlavná 410/13, Žilina"
    },
    "architecturalStyle": {
      "type": "Property",
      "value": "klasicizmus"
    },
    "cadastralArea": {
      "type": "Property",
      "value": "Bytčica"
    },
    "constructionPeriod": {
      "type": "Property",
      "value": "1844"
    },
    "id": "urn:ngsi-ld:PointOfInterest:zilina.sk:zilina-verejne:uzpf-1328-1",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.7312574039944,
          49.1831077434067
        ],
        "type": "Point"
      }
    },
    "monumentKind": {
      "type": "Property",
      "value": "KOSTOL"
    },
    "monumentNumber": {
      "type": "Property",
      "value": "1328/1"
    },
    "name": {
      "languageMap": {
        "sk": "kostol sv.Imricha"
      },
      "type": "LanguageProperty"
    },
    "ownershipForm": {
      "type": "Property",
      "value": "Vlastníctvo cirkvi a cirk. organizácií"
    },
    "type": "PointOfInterest"
  },
  {
    "address": {
      "type": "Property",
      "value": "Dlhá 605/101, Žilina"
    },
    "architecturalStyle": {
      "type": "Property",
      "value": "renesancia"
    },
    "cadastralArea": {
      "type": "Property",
      "value": "Bytčica"
    },
    "constructionPeriod": {
      "type": "Property",
      "value": "2/4 17.st."
    },
    "id": "urn:ngsi-ld:PointOfInterest:zilina.sk:zilina-verejne:uzpf-1329-1",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.73147534,
          49.1849631597
        ],
        "type": "Point"
      }
    },
    "monumentKind": {
      "type": "Property",
      "value": "KAŠTIEĽ"
    },
    "monumentNumber": {
      "type": "Property",
      "value": "1329/1"
    },
    "name": {
      "languageMap": {
        "sk": "kaštieľ Bytčica"
      },
      "type": "LanguageProperty"
    },
    "ownershipForm": {
      "type": "Property",
      "value": "Spoločnosť s ručením obmedzeným"
    },
    "type": "PointOfInterest"
  }
];

export const STATIONS = [
  {
    "dailyDepartures": {
      "type": "Property",
      "value": 58
    },
    "id": "urn:ngsi-ld:GtfsStop:zilina.sk:zilina-verejne:zsr-5618255",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.7524032860906,
          49.267399835039676
        ],
        "type": "Point"
      }
    },
    "name": {
      "languageMap": {
        "sk": "Brodno"
      },
      "type": "LanguageProperty"
    },
    "stopCode": {
      "type": "Property",
      "value": "5618255"
    },
    "type": "GtfsStop"
  },
  {
    "dailyDepartures": {
      "type": "Property",
      "value": 42
    },
    "id": "urn:ngsi-ld:GtfsStop:zilina.sk:zilina-verejne:zsr-5617535",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.73059245995625,
          49.189092800975104
        ],
        "type": "Point"
      }
    },
    "name": {
      "languageMap": {
        "sk": "Bytčica"
      },
      "type": "LanguageProperty"
    },
    "stopCode": {
      "type": "Property",
      "value": "5617535"
    },
    "type": "GtfsStop"
  },
  {
    "dailyDepartures": {
      "type": "Property",
      "value": 166
    },
    "id": "urn:ngsi-ld:GtfsStop:zilina.sk:zilina-verejne:zsr-5617915",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.746633728837626,
          49.22791375158773
        ],
        "type": "Point"
      }
    },
    "name": {
      "languageMap": {
        "sk": "Žilina"
      },
      "type": "LanguageProperty"
    },
    "stopCode": {
      "type": "Property",
      "value": "5617915"
    },
    "type": "GtfsStop"
  },
  {
    "dailyDepartures": {
      "type": "Property",
      "value": 42
    },
    "id": "urn:ngsi-ld:GtfsStop:zilina.sk:zilina-verejne:zsr-5617930",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.73306296929719,
          49.1996620716122
        ],
        "type": "Point"
      }
    },
    "name": {
      "languageMap": {
        "sk": "Žilina-Solinky"
      },
      "type": "LanguageProperty"
    },
    "stopCode": {
      "type": "Property",
      "value": "5617930"
    },
    "type": "GtfsStop"
  },
  {
    "dailyDepartures": {
      "type": "Property",
      "value": 42
    },
    "id": "urn:ngsi-ld:GtfsStop:zilina.sk:zilina-verejne:zsr-5617925",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.731086282011677,
          49.21933349306341
        ],
        "type": "Point"
      }
    },
    "name": {
      "languageMap": {
        "sk": "Žilina-zariečie"
      },
      "type": "LanguageProperty"
    },
    "stopCode": {
      "type": "Property",
      "value": "5617925"
    },
    "type": "GtfsStop"
  }
];

export const AIR = [
  {
    "co": {
      "observedAt": "2026-10-06T18:00:00Z",
      "type": "Property",
      "unitCode": "GP",
      "value": 0.63452
    },
    "dateObserved": {
      "type": "Property",
      "value": "2026-10-06T18:00:00Z"
    },
    "id": "urn:ngsi-ld:AirQualityObserved:zilina.sk:zilina-verejne:eea-SK0020A",
    "location": {
      "type": "GeoProperty",
      "value": {
        "coordinates": [
          18.771215,
          49.211447
        ],
        "type": "Point"
      }
    },
    "name": {
      "languageMap": {
        "en": "Station SK0020A, urban background",
        "sk": "Stanica SK0020A, mestské pozadie"
      },
      "type": "LanguageProperty"
    },
    "type": "AirQualityObserved",
    "no2": {
      "observedAt": "2026-10-06T18:00:00Z",
      "type": "Property",
      "unitCode": "GQ",
      "value": 28.1313
    },
    "o3": {
      "observedAt": "2026-10-05T17:00:00Z",
      "type": "Property",
      "unitCode": "GQ",
      "value": 11.8076
    },
    "pm10": {
      "observedAt": "2026-10-06T18:00:00Z",
      "type": "Property",
      "unitCode": "GQ",
      "value": 32.341
    },
    "pm25": {
      "observedAt": "2026-10-06T18:00:00Z",
      "type": "Property",
      "unitCode": "GQ",
      "value": 15.417
    }
  }
];

export const WORKS = [
  {
    "dataProvider": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline, DREPO"
    },
    "id": "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:hdluniza-936",
    "collectionName": {
      "type": "Property",
      "value": "Práce a štúdie - Vydanie 13"
    },
    "license": {
      "type": "Property",
      "value": "http://creativecommons.org/licenses/by/4.0/"
    },
    "name": {
      "languageMap": {
        "sk": "Accuracy of digital terrain model on forest roads using airborne LIDAR and UAV point clouds"
      },
      "type": "LanguageProperty"
    },
    "publisher": {
      "type": "Property",
      "value": "University of Žilina"
    },
    "source": {
      "type": "Property",
      "value": "https://dspace.uniza.sk/server/api/"
    },
    "type": "Article",
    "url": {
      "type": "Property",
      "value": "http://drepo.uniza.sk/handle/hdluniza/936"
    },
    "workType": {
      "type": "Property",
      "value": "Conference paper"
    },
    "yearPublished": {
      "type": "Property",
      "value": 2023
    }
  },
  {
    "dataProvider": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline, DREPO"
    },
    "id": "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:hdluniza-237",
    "collectionName": {
      "type": "Property",
      "value": "Pošta, Telekomunikácie a Elektronický obchod - ročník 13.; číslo 2/2018"
    },
    "license": {
      "type": "Property",
      "value": "http://creativecommons.org/licenses/by/4.0/"
    },
    "name": {
      "languageMap": {
        "sk": "ANALÝZA METÓD STANOVENIA HODNOTY PODNIKU PLATNÝCH V PODMIENKACH SLOVENSKEJ REPUBLIKY"
      },
      "type": "LanguageProperty"
    },
    "publisher": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline"
    },
    "source": {
      "type": "Property",
      "value": "https://dspace.uniza.sk/server/api/"
    },
    "type": "Article",
    "url": {
      "type": "Property",
      "value": "http://drepo.uniza.sk/handle/hdluniza/237"
    },
    "workType": {
      "type": "Property",
      "value": "Article"
    },
    "yearPublished": {
      "type": "Property",
      "value": 2018
    }
  },
  {
    "dataProvider": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline, DREPO"
    },
    "id": "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:hdluniza-95",
    "collectionName": {
      "type": "Property",
      "value": "Pošta, Telekomunikácie a Elektronický obchod - ročník 11.; číslo 1/2016"
    },
    "license": {
      "type": "Property",
      "value": "http://creativecommons.org/licenses/by/4.0/"
    },
    "name": {
      "languageMap": {
        "sk": "MARKETINGOVÝ MIX V PODMIENKACH ENERGETICKÝCH PODNIKOV NA SLOVENSKU"
      },
      "type": "LanguageProperty"
    },
    "publisher": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline"
    },
    "source": {
      "type": "Property",
      "value": "https://dspace.uniza.sk/server/api/"
    },
    "type": "Article",
    "url": {
      "type": "Property",
      "value": "http://drepo.uniza.sk/handle/hdluniza/95"
    },
    "workType": {
      "type": "Property",
      "value": "Working Paper"
    },
    "yearPublished": {
      "type": "Property",
      "value": 2016
    }
  },
  {
    "dataProvider": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline, DREPO"
    },
    "id": "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:hdluniza-94",
    "collectionName": {
      "type": "Property",
      "value": "Pošta, Telekomunikácie a Elektronický obchod - ročník 10.; číslo 2/2015"
    },
    "license": {
      "type": "Property",
      "value": "http://creativecommons.org/licenses/by/4.0/"
    },
    "name": {
      "languageMap": {
        "sk": "Pošta, Telekomunikácie a Elektronický obchod - celé číslo 2/2015"
      },
      "type": "LanguageProperty"
    },
    "publisher": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline"
    },
    "source": {
      "type": "Property",
      "value": "https://dspace.uniza.sk/server/api/"
    },
    "type": "Article",
    "url": {
      "type": "Property",
      "value": "http://drepo.uniza.sk/handle/hdluniza/94"
    },
    "workType": {
      "type": "Property",
      "value": "Journal"
    },
    "yearPublished": {
      "type": "Property",
      "value": 2015
    }
  },
  {
    "dataProvider": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline, DREPO"
    },
    "id": "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:hdluniza-157",
    "collectionName": {
      "type": "Property",
      "value": "Práce a štúdie - Vydanie 07"
    },
    "license": {
      "type": "Property",
      "value": "http://creativecommons.org/licenses/by/4.0/"
    },
    "name": {
      "languageMap": {
        "sk": "ZMENY ÚLOH ANSP V BUDÚCOM PROSTREDÍ ATM"
      },
      "type": "LanguageProperty"
    },
    "publisher": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline"
    },
    "source": {
      "type": "Property",
      "value": "https://dspace.uniza.sk/server/api/"
    },
    "type": "Article",
    "url": {
      "type": "Property",
      "value": "http://drepo.uniza.sk/handle/hdluniza/157"
    },
    "workType": {
      "type": "Property",
      "value": "Book of proceedings"
    },
    "yearPublished": {
      "type": "Property",
      "value": 2020
    }
  },
  {
    "dataProvider": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline, DREPO"
    },
    "id": "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:hdluniza-905",
    "collectionName": {
      "type": "Property",
      "value": "Súčasné problémy v koľajových vozidlách – PRORAIL 2023  Diel I."
    },
    "license": {
      "type": "Property",
      "value": "http://creativecommons.org/licenses/by/4.0/"
    },
    "name": {
      "languageMap": {
        "cs": "MODERNÍ VZDĚLÁVÁNÍ PRO OBOR KOLEJOVÝCH VOZIDEL"
      },
      "type": "LanguageProperty"
    },
    "publisher": {
      "type": "Property",
      "value": "VTS pri Žilinskej univerzite v Žiline"
    },
    "source": {
      "type": "Property",
      "value": "https://dspace.uniza.sk/server/api/"
    },
    "type": "Article",
    "url": {
      "type": "Property",
      "value": "http://drepo.uniza.sk/handle/hdluniza/905"
    },
    "workType": {
      "type": "Property",
      "value": "Conference paper"
    },
    "yearPublished": {
      "type": "Property",
      "value": 2023
    }
  },
  {
    "dataProvider": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline, DREPO"
    },
    "id": "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:hdluniza-979",
    "collectionName": {
      "type": "Property",
      "value": "Práce a štúdie - Vydanie 14"
    },
    "license": {
      "type": "Property",
      "value": "http://creativecommons.org/licenses/by/4.0/"
    },
    "name": {
      "languageMap": {
        "en": "Práce a štúdie - celé vydanie č.14"
      },
      "type": "LanguageProperty"
    },
    "publisher": {
      "type": "Property",
      "value": "University of Žilina"
    },
    "source": {
      "type": "Property",
      "value": "https://dspace.uniza.sk/server/api/"
    },
    "type": "Article",
    "url": {
      "type": "Property",
      "value": "http://drepo.uniza.sk/handle/hdluniza/979"
    },
    "workType": {
      "type": "Property",
      "value": "Conference paper"
    },
    "yearPublished": {
      "type": "Property",
      "value": 2023
    }
  },
  {
    "dataProvider": {
      "type": "Property",
      "value": "Žilinská univerzita v Žiline, DREPO"
    },
    "id": "urn:ngsi-ld:Article:zilina.sk:zilina-uniza:hdluniza-524",
    "collectionName": {
      "type": "Property",
      "value": "Pošta, Telekomunikácie a Elektronický obchod - ročník 15.; číslo 2/2020"
    },
    "license": {
      "type": "Property",
      "value": "http://creativecommons.org/licenses/by/4.0/"
    },
    "name": {
      "languageMap": {
        "en": "Porovnanie platobných brán"
      },
      "type": "LanguageProperty"
    },
    "publisher": {
      "type": "Property",
      "value": "University of Zilina"
    },
    "source": {
      "type": "Property",
      "value": "https://dspace.uniza.sk/server/api/"
    },
    "type": "Article",
    "url": {
      "type": "Property",
      "value": "http://drepo.uniza.sk/handle/hdluniza/524"
    },
    "workType": {
      "type": "Property",
      "value": "Article"
    },
    "yearPublished": {
      "type": "Property",
      "value": 2020
    }
  }
];

const BY_TYPE: Record<string, unknown[]> = {
  PointOfInterest: MONUMENTS,
  GtfsStop: STATIONS,
  AirQualityObserved: AIR,
  Article: WORKS,
};

/** An endpoint's answer to `?type=…`: every entity of that type, or none. */
export function answer(type: string | null): unknown[] {
  return (type && BY_TYPE[type]) || [];
}
