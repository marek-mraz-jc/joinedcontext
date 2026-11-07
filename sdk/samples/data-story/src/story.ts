import type { Row } from "@joinedcontext/sdk";

/** The WHO 2021 air quality guideline for PM2.5 over 24 hours, in µg/m³. */
export const GUIDELINE = 15;

export interface Reading {
  station: string;
  day: string;
  pm25: number;
}

export interface StationMean {
  station: string;
  mean: number;
  /** Days on which the station's own daily mean was above the guideline. */
  daysOver: number;
}

/** Only readings that carry a station, a date and a finite value: the rest are not guessed. */
export function readings(rows: Row[]): Reading[] {
  return rows.flatMap((row) => {
    const pm25 = row.pm25;
    const station = typeof row.stationName === "string" ? row.stationName : "";
    const day = typeof row.dateObserved === "string" ? row.dateObserved.slice(0, 10) : "";
    if (typeof pm25 !== "number" || !Number.isFinite(pm25) || station === "" || !/^\d{4}-\d{2}-\d{2}$/.test(day)) return [];
    return [{ station, day, pm25 }];
  });
}

function mean(values: number[]): number {
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

function group<K>(list: Reading[], key: (reading: Reading) => K): Map<K, Reading[]> {
  const out = new Map<K, Reading[]>();
  for (const reading of list) out.set(key(reading), [...(out.get(key(reading)) ?? []), reading]);
  return out;
}

/** The mean over every station, day by day, oldest first. */
export function dailyMeans(list: Reading[]): { day: string; mean: number }[] {
  return [...group(list, (reading) => reading.day).entries()]
    .map(([day, found]) => ({ day, mean: mean(found.map((reading) => reading.pm25)) }))
    .sort((a, b) => a.day.localeCompare(b.day));
}

/** Each station's mean over the whole period and its days over the guideline, worst first. */
export function stationMeans(list: Reading[]): StationMean[] {
  return [...group(list, (reading) => reading.station).entries()]
    .map(([station, found]) => {
      const days = [...group(found, (reading) => reading.day).values()].map((day) => mean(day.map((reading) => reading.pm25)));
      return { station, mean: mean(found.map((reading) => reading.pm25)), daysOver: days.filter((value) => value > GUIDELINE).length };
    })
    .sort((a, b) => b.mean - a.mean || a.station.localeCompare(b.station));
}

export interface Summary {
  stations: number;
  from: string;
  to: string;
  mean: number;
  worst: StationMean;
  cleanest: StationMean;
  /** Days on which the city-wide mean was above the guideline. */
  daysOver: number;
  days: number;
  /** The change from the first to the last week's mean, in µg/m³; null with less than two weeks. */
  change: number | null;
}

/** The numbers the story tells, all read from the data; null when there is nothing to tell. */
export function summarise(list: Reading[]): Summary | null {
  if (list.length === 0) return null;
  const daily = dailyMeans(list);
  const stations = stationMeans(list);
  const week = (part: typeof daily) => mean(part.map((day) => day.mean));
  return {
    stations: stations.length,
    from: daily[0].day,
    to: daily[daily.length - 1].day,
    mean: mean(list.map((reading) => reading.pm25)),
    worst: stations[0],
    cleanest: stations[stations.length - 1],
    daysOver: daily.filter((day) => day.mean > GUIDELINE).length,
    days: daily.length,
    change: daily.length >= 14 ? week(daily.slice(-7)) - week(daily.slice(0, 7)) : null,
  };
}
