/**
 * The statistics office's own names for the codes the records carry (T-2966): each cube's title,
 * the labels of its indicator and key dimensions, and the territories, in Slovak and English.
 * Written by `../../labels.mjs` from the URLs the pipelines read (data.statistics.sk); do not
 * edit by hand.
 */
export type Named = { sk?: string; en?: string };

export const DATASETS: Record<string, Named> = {
  "cr3802mr": {
    "sk": "Návštevnosť UZ - okresy (3 kategórie UZ)",
    "en": "Occupancy of accommodation establishments – districts (3 categories)"
  },
  "cr3803mr": {
    "sk": "Návštevnosť UZ - obce (zahraniční/domáci)",
    "en": "Occupancy of accommodation establishments – municipalities (inbound/domestic)"
  },
  "cr3807qr": {
    "sk": "Návštevnosť, tržby a kapacity UZ - okresy (3 kategórie UZ, 9 ukazovateľov)",
    "en": "Occupancy, turnover and capacity of accommodation establishments - districts (3 categories, 9 variables)"
  },
  "cr3809qr": {
    "sk": "Návštevnosť, tržby a kapacity UZ - obce (13 ukazovateľov)",
    "en": "Occupancy, turnover and capacity of accommodation establishments - municipalities (13 variables)"
  },
  "ku5008rr": {
    "sk": "Knižnice",
    "en": "Libraries"
  },
  "np3110rr": {
    "sk": "Mzdy podľa ekonomickej činnosti zistené pracoviskovou metódou",
    "en": "Wage by economic activity collected through workplace method"
  },
  "om7101qr": {
    "sk": "Počet obyvateľov podľa pohlavia - obce (štvrťročne)",
    "en": "Number of the Population by Sex - Municipalities (quarterly)"
  },
  "om7102rr": {
    "sk": "Počet obyvateľov podľa pohlavia - SR-oblasť-kraj-okres, m-v (ročne)",
    "en": "Number of the Population by Sex - SR-Area-Reg-District, U-R (yearly)"
  },
  "om7103rr": {
    "sk": "Prehľad pohybu obyvateľstva - obce (ročne)",
    "en": "Population Change - Municipalities (yearly)"
  },
  "pl5001rr": {
    "sk": "Výmera územia, využitie pôdy - SR-oblasť-kraj-okres-obec",
    "en": "Land area, land use - SR-Areas-Regions-Districts-Municipalities"
  },
  "pr5001rr": {
    "sk": "Evidovaní uchádzači o zamestnanie",
    "en": "Number of registered job applicants"
  },
  "st3004rr": {
    "sk": "Byty - vybrané ukazovatele",
    "en": "Housing construction - selected indicators"
  },
  "sv5001rr": {
    "sk": "Materské školy",
    "en": "Kindergartens"
  },
  "sv5002rr": {
    "sk": "Základné školy",
    "en": "Basic schools"
  },
  "vh5003rr": {
    "sk": "Spotreba pitnej vody",
    "en": "Consumption of drinking water"
  },
  "zp3803rs": {
    "sk": "Emisie základných znečisťujúcich látok",
    "en": "Emissions of basic pollutants"
  }
};

export const CODES: Record<string, Record<string, Named>> = {
  "cr3802mr": {
    "U_CR_0005": {
      "sk": "Počet návštevníkov spolu",
      "en": "Number of visitors total"
    },
    "U_CR_0008": {
      "sk": "Počet prenocovaní návštevníkov spolu",
      "en": "Number of nights spent by visitors"
    }
  },
  "cr3803mr": {
    "U_CR_0005": {
      "sk": "Počet návštevníkov spolu",
      "en": "Number of visitors total"
    },
    "U_CR_0008": {
      "sk": "Počet prenocovaní návštevníkov spolu",
      "en": "Number of nights spent by visitors"
    },
    "VISIT_DOM": {
      "sk": "Domáci návštevníci",
      "en": "Domestic visitors"
    },
    "VISIT_FOR": {
      "sk": "Zahraniční návštevníci",
      "en": "Foreign visitors"
    },
    "VISIT_TOTAL": {
      "sk": "Návštevníci spolu",
      "en": "Visitors total"
    }
  },
  "cr3807qr": {
    "U_CR_0002": {
      "sk": "Celkový počet ubytovacích zariadení k dispozícii",
      "en": "Total number of available accommodation establishments"
    },
    "U_CR_0003": {
      "sk": "Celkový počet izieb k dispozícii",
      "en": "Total number of available bedrooms"
    },
    "U_CR_0004": {
      "sk": "Celkový počet lôžok (vrátane kempingových miest) k dispozícii",
      "en": "Total number of available bed places (including camping grounds)"
    }
  },
  "cr3809qr": {
    "U_CR_0002": {
      "sk": "Celkový počet ubytovacích zariadení k dispozícii",
      "en": "Total number of available accommodation establishments"
    },
    "U_CR_0003": {
      "sk": "Celkový počet izieb k dispozícii",
      "en": "Total number of available bedrooms"
    },
    "U_CR_0004": {
      "sk": "Celkový počet lôžok (vrátane kempingových miest) k dispozícii",
      "en": "Total number of available bed places (including camping grounds)"
    }
  },
  "ku5008rr": {
    "U12010": {
      "sk": "Verejné knižnice - počet",
      "en": "Public libraries - number"
    },
    "U12011": {
      "sk": "Verejné knižnice - počet knižničných jednotiek spolu",
      "en": "Public libraries - number of library units"
    },
    "U12020": {
      "sk": "Vedecké knižnice - počet",
      "en": "Scientific libraries - number"
    },
    "U12021": {
      "sk": "Vedecké knižnice - počet knižničných jednotiek spolu",
      "en": "Scientific libraries - number of library units"
    },
    "U12023": {
      "sk": "Špeciálne knižnice - počet",
      "en": "Special libraries - number"
    },
    "U12024": {
      "sk": "Špeciálne knižnice - počet knižničných jednotiek",
      "en": "Special libraries - number of library units"
    },
    "U12026": {
      "sk": "Akademické knižnice - počet",
      "en": "Academic libraries - number"
    },
    "U12027": {
      "sk": "Akademické knižnice - počet knižničných jednotiek",
      "en": "Academic libraries - number of library units"
    },
    "U12028": {
      "sk": "Školské knižnice - počet",
      "en": "School libraries - number"
    },
    "U12029": {
      "sk": "Školské knižnice - počet knižničných jednotiek",
      "en": "School libraries - number of library units"
    }
  },
  "np3110rr": {
    "E_PRIEM_MZDA": {
      "sk": "Priemerná mesačná mzda zamestnanca (Eur)",
      "en": "Average monthly wage of employee (EUR)"
    }
  },
  "om7101qr": {
    "1": {
      "sk": "Muži",
      "en": "Men"
    },
    "2": {
      "sk": "Ženy",
      "en": "Women"
    },
    "IN010113": {
      "sk": "Stav trvale bývajúceho obyvateľstva na začiatku obdobia (Osoba)",
      "en": "Population as of the beginning of a given period (Person)"
    },
    "IN010114": {
      "sk": "Stredný (priemerný) stav trvale bývajúceho obyvateľstva (Osoba)",
      "en": "Mid-year (Mean) population (Person)"
    },
    "IN010115": {
      "sk": "Stav trvale bývajúceho obyvateľstva na konci obdobia (Osoba)",
      "en": "Population as of the end of a given period (Person)"
    },
    "SPOLU": {
      "sk": "Spolu",
      "en": "Total"
    }
  },
  "om7102rr": {
    "IN010115": {
      "sk": "Stav trvale bývajúceho obyvateľstva na konci obdobia (Osoba)",
      "en": "Population as of the end of a given period (Person)"
    }
  },
  "om7103rr": {
    "IN010061": {
      "sk": "Zomretí (Osoba)",
      "en": "Mortality (Person)"
    },
    "IN010071": {
      "sk": "Sobáše (Počet v jednotkách)",
      "en": "Nuptiality (Number in units)"
    },
    "IN010073": {
      "sk": "Rozvody (Počet v jednotkách)",
      "en": "Divorce (Number in units)"
    },
    "IN010076": {
      "sk": "Prirodzený prírastok obyvateľstva (Osoba)",
      "en": "Natural increase (Person)"
    },
    "IN010078": {
      "sk": "Prisťahovaní na trvalý pobyt (Osoba)",
      "en": "Immigrants (in-migrants) on permanent residence (Person)"
    },
    "IN010079": {
      "sk": "Vysťahovaní z trvalého pobytu (Osoba)",
      "en": "Emigrants (out-migrants) from permanent residence (Person)"
    },
    "IN010080": {
      "sk": "Migračné saldo (Osoba)",
      "en": "Net migration (Person)"
    },
    "IN010082": {
      "sk": "Celkový prírastok obyvateľstva (Osoba)",
      "en": "Total increase of population (Person)"
    },
    "IN010106": {
      "sk": "Živonarodení (Osoba)",
      "en": "Live births (Person)"
    },
    "IN010114": {
      "sk": "Stredný (priemerný) stav trvale bývajúceho obyvateľstva (Osoba)",
      "en": "Mid-year (Mean) population (Person)"
    },
    "IN010115": {
      "sk": "Stav trvale bývajúceho obyvateľstva na konci obdobia (Osoba)",
      "en": "Population as of the end of a given period (Person)"
    }
  },
  "pl5001rr": {
    "U14010": {
      "sk": "Celková výmera územia obce - mesta (v m2)",
      "en": "Total area of land of municipality-town in m2"
    },
    "U14020": {
      "sk": "Poľnohospodárska pôda  - spolu (v m2)",
      "en": "Agricultural land in total in m2"
    },
    "U14030": {
      "sk": "Poľnohospodárska pôda  - orná pôda (v m2)",
      "en": "Agricultural land - arable land in m2"
    },
    "U14040": {
      "sk": "Poľnohospodárska pôda  - chmeľnica (v m2)",
      "en": "Agricultural land - hop-garden in m2"
    },
    "U14050": {
      "sk": "Poľnohospodárska pôda  - vinica (v m2)",
      "en": "Agricultural land - vineyard in m2"
    },
    "U14060": {
      "sk": "Poľnohospodárska pôda  -  záhrada",
      "en": "Agricultural land - garden in m2"
    },
    "U14070": {
      "sk": "Poľnohospodárska pôda  -  ovocný sad (v m2)",
      "en": "Agricultural land - orchard in m2"
    },
    "U14080": {
      "sk": "Poľnohospodárska pôda  -  trvalý trávny porast (v m2)",
      "en": "Agricultural land - permanent grass growt in m2"
    },
    "U14100": {
      "sk": "Nepoľnohospodárska pôda - spolu",
      "en": "Non-agricultural land in total in m2"
    },
    "U14110": {
      "sk": "Nepoľnohospodárska pôda - lesný pozemok (v m2)",
      "en": "Non-agricultural land - forest area in m2"
    },
    "U14120": {
      "sk": "Nepoľnohospodárska pôda - vodná plocha (v m2)",
      "en": "Non-agricultural land - water area in m2"
    },
    "U14130": {
      "sk": "Nepoľnohospodárska pôda - zastavaná plocha a nádvorie (v m2)",
      "en": "Non-agricultural land - built-up area and yard in m2"
    },
    "U14140": {
      "sk": "Nepoľnohospodárska pôda - ostatná plocha (v m2)",
      "en": "Non-agricultural land - other area in m2"
    }
  },
  "pr5001rr": {
    "U15061": {
      "sk": "Počet evidovaných uchádzačov o zamestnanie spolu",
      "en": "Number of registered job applicants in total"
    },
    "U15062": {
      "sk": "Počet evidovaných uchádzačiek o zamestnanie",
      "en": "Number of registered job applicants - women"
    }
  },
  "st3004rr": {
    "DOKONC_BYT": {
      "sk": "Byty - dokončené v danom roku",
      "en": "Dwellings - completed in a given year"
    },
    "ROZOST_BYT_KON": {
      "sk": "Byty rozostavané k 31. 12.",
      "en": "Dwellings under construction as of December 31"
    },
    "ROZOST_BYT_ZAC": {
      "sk": "Byty - rozostavané k 1. 1.",
      "en": "Dwellings - under construction as of January 1"
    },
    "ZAC_BYT": {
      "sk": "Byty - začaté v danom roku",
      "en": "Dwellings - started in a given year"
    }
  },
  "sv5001rr": {
    "U11001": {
      "sk": "Materské školy spolu",
      "en": "Kindergartens in total"
    },
    "U11002": {
      "sk": "Materské školy spolu - deti",
      "en": "Kindergartens in total - children"
    },
    "U11020": {
      "sk": "Materské školy - štátne",
      "en": "Kindergartens - state"
    },
    "U11021": {
      "sk": "Materské školy - štátne - deti",
      "en": "Kindergartens - state - children"
    },
    "U11022": {
      "sk": "Materské školy - štátne - triedy",
      "en": "Kindergartens - state - classes"
    },
    "U11023": {
      "sk": "Materské školy - štátne - učitelia",
      "en": "Kindergartens - state - teachers"
    },
    "U11300": {
      "sk": "Cirkevné materské školy",
      "en": "Church kindergartens"
    },
    "U11301": {
      "sk": "Cirkevné materské školy - deti",
      "en": "Church kindergartens - children"
    },
    "U11302": {
      "sk": "Cirkevné materské školy - triedy",
      "en": "Church kindergartens - classes"
    },
    "U11303": {
      "sk": "Cirkevné materské školy - učitelia",
      "en": "Church kindergartens - teachers"
    },
    "U11400": {
      "sk": "Súkromné materské školy",
      "en": "Private kindergartens"
    },
    "U11401": {
      "sk": "Súkromné materské školy - deti",
      "en": "Private kindergartens - children"
    },
    "U11402": {
      "sk": "Súkromné materské školy - triedy",
      "en": "Private kindergartens - classes"
    },
    "U11403": {
      "sk": "Súkromné materské školy - učitelia",
      "en": "Private kindergartens - teachers"
    }
  },
  "sv5002rr": {
    "U11003": {
      "sk": "Základné školy - (1. - 4. roč.) spolu",
      "en": "Basic schools (1. - 4. class) in total"
    },
    "U11004": {
      "sk": "Základné školy - (1. - 4. roč.) spolu - žiaci",
      "en": "Basic schools (1. - 4. class) in total  - pupils"
    },
    "U11005": {
      "sk": "Základné školy - (1. - 9. roč.) spolu",
      "en": "Basic schools (1. - 9. class) in total"
    },
    "U11006": {
      "sk": "Základné školy - (1. - 9. roč.) spolu - žiaci",
      "en": "Basic schools (1. - 9. class) in total  - pupils"
    },
    "U11030": {
      "sk": "Základné školy - štátne (1. - 4. roč.)",
      "en": "Basic schools - state (1. - 4. class)"
    },
    "U11031": {
      "sk": "Základné školy - štátne (1. - 4. roč.) - žiaci",
      "en": "Basic schools - state (1. - 4. class) - pupils"
    },
    "U11032": {
      "sk": "Základné školy - štátne (1. - 4. roč.) - triedy",
      "en": "Basic schools - state (1. - 4. class) - classes"
    },
    "U11033": {
      "sk": "Základné školy - štátne  (1. - 4. roč.) - učitelia",
      "en": "Basic schools - state (1. - 4. class) - teachers"
    },
    "U11040": {
      "sk": "Základné školy - štátne (1. - 9. roč.)",
      "en": "Basic schools - state (1. - 9. class)"
    },
    "U11041": {
      "sk": "Základné školy - štátne (1. - 9. roč.) - žiaci",
      "en": "Basic schools - state (1. - 9. class) - pupils"
    },
    "U11042": {
      "sk": "Základné školy - štátne (1. - 9. roč.) - triedy",
      "en": "Basic schools - state (1. - 9. class) - classes"
    },
    "U11043": {
      "sk": "Základné školy - štátne (1. - 9. roč.) - učitelia",
      "en": "Basic schools - state (1. - 9. class) - teachers"
    },
    "U11310": {
      "sk": "Cirkevné základné školy (1. - 4. roč.)",
      "en": "Church basic schools (1. - 4. class)"
    },
    "U11311": {
      "sk": "Cirkevné základné školy (1. - 4. roč.) -  žiaci",
      "en": "Church basic schools (1. - 4. class) - pupils"
    },
    "U11312": {
      "sk": "Cirkevné základné školy (1. - 4. roč.) - triedy",
      "en": "Church basic schools (1. - 4. class) - classes"
    },
    "U11313": {
      "sk": "Cirkevné základné školy (1. - 4. roč.) - učitelia",
      "en": "Church basic schools (1. - 4. class) - teachers"
    },
    "U11320": {
      "sk": "Cirkevné základné školy (1. - 9. roč.)",
      "en": "Church basic schools (1. - 9. class)"
    },
    "U11321": {
      "sk": "Cirkevné základné školy (1. - 9. roč.) - žiaci",
      "en": "Church basic schools (1. - 9. class) - pupils"
    },
    "U11322": {
      "sk": "Cirkevné základné školy (1. - 9. roč.) - triedy",
      "en": "Church basic schools (1. - 9. class) - classes"
    },
    "U11323": {
      "sk": "Cirkevné základné školy (1. - 9. roč.) - učitelia",
      "en": "Church basic schools (1. - 9. class) - teachers"
    },
    "U11410": {
      "sk": "Súkromné základné školy (1.- 4. roč.)",
      "en": "Private basic schools (1. - 4. class)"
    },
    "U11411": {
      "sk": "Súkromné základné školy (1.- 4. roč.) - žiaci",
      "en": "Private basic schools (1. - 4. class) - pupils"
    },
    "U11412": {
      "sk": "Súkromné základné školy (1.- 4. roč.) - triedy",
      "en": "Private basic schools (1. - 4. class) - classes"
    },
    "U11413": {
      "sk": "Súkromné základné školy (1.- 4. roč.) - učitelia",
      "en": "Private basic schools (1. - 4. class) - teachers"
    },
    "U11420": {
      "sk": "Súkromné základné školy  (1. - 9. roč. )",
      "en": "Private basic schools (1. - 9. class)"
    },
    "U11421": {
      "sk": "Súkromné základné školy  (1. - 9. roč. ) - žiaci",
      "en": "Private basic schools (1. - 9. class) - pupils"
    },
    "U11422": {
      "sk": "Súkromné základné školy  (1. - 9. roč. ) - triedy",
      "en": "Private basic schools (1. - 9. class) - classes"
    },
    "U11423": {
      "sk": "Súkromné základné školy  (1. - 9. roč. ) - učitelia",
      "en": "Private basic schools (1. - 9. class) - teachers"
    }
  },
  "vh5003rr": {
    "U03084": {
      "sk": "Spotreba pitnej vody - spolu",
      "en": "Consumption of drinking water in total"
    }
  },
  "zp3803rs": {
    "1": {
      "sk": "Tuhé emisie",
      "en": "Particulates"
    },
    "ODPAD_TONY_KM2": {
      "sk": "Produkcia v tonách na km2",
      "en": "Productions in tons/km2"
    }
  }
};

export const AREAS: Record<string, Named> = {
  "SK032": {
    "sk": "Banskobystrický kraj",
    "en": "Region of Banská Bystrica"
  },
  "SK0321": {
    "sk": "Okres Banská Bystrica",
    "en": "District of Banská Bystrica"
  },
  "SK0321508438": {
    "sk": "Banská Bystrica",
    "en": "Banská Bystrica"
  },
  "SK0322": {
    "sk": "Okres Banská Štiavnica",
    "en": "District of Banská Štiavnica"
  },
  "SK0323": {
    "sk": "Okres Brezno",
    "en": "District of Brezno"
  },
  "SK0324": {
    "sk": "Okres Detva",
    "en": "District of Detva"
  },
  "SK0325": {
    "sk": "Okres Krupina",
    "en": "District of Krupina"
  },
  "SK0326": {
    "sk": "Okres Lučenec",
    "en": "District of Lučenec"
  },
  "SK0327": {
    "sk": "Okres Poltár",
    "en": "District of Poltár"
  },
  "SK0328": {
    "sk": "Okres Revúca",
    "en": "District of Revúca"
  },
  "SK0329": {
    "sk": "Okres Rimavská Sobota",
    "en": "District of Rimavská Sobota"
  },
  "SK032A": {
    "sk": "Okres Veľký Krtíš",
    "en": "District of Veľký Krtíš"
  },
  "SK032B": {
    "sk": "Okres Zvolen",
    "en": "District of Zvolen"
  },
  "SK032C": {
    "sk": "Okres Žarnovica",
    "en": "District of Žarnovica"
  },
  "SK032D": {
    "sk": "Okres Žiar nad Hronom",
    "en": "District of Žiar nad Hronom"
  }
};

/** The cubes each body's pipelines read, in the order of their files. */
export const BODY_DATASETS: Record<string, string[]> = {
  "banskabystrica": [
    "ku5008rr",
    "sv5001rr",
    "cr3803mr",
    "om7101qr",
    "om7103rr",
    "pl5001rr",
    "cr3809qr",
    "pr5001rr",
    "vh5003rr",
    "sv5002rr"
  ],
  "bbsk": [
    "st3004rr",
    "zp3803rs",
    "cr3807qr",
    "np3110rr",
    "cr3802mr",
    "om7102rr",
    "pr5001rr"
  ]
};
