# KPI dashboard

**For:** managers who want to know, in one look, which indicators are on target and which are
not, and whether each is getting better.
**Shape:** a header with a period picker, a grid of indicator cards (latest value, target, a
standing and a trend line), and a comparison of every indicator to its target.

## What it uses
- `useEntities` for the observations; the grouping, the period and the standing are pure
  functions in `src/indicators.ts`, tested on their own.
- `Page`, `Header`, `Grid` (up to four columns), `Card` from the SDK; the template's
  `ChartCard` with an ECharts option per trend (the target as a dashed mark line, the axis
  rounded around values and target by `axisRange`) and one for the comparison.
- `format` for every value.

## What to copy
- The standing is a word and a glyph ("▲ Ahead by 2 %", "▼ Short by 0.8 %"), never colour alone,
  and "better" follows `higherIsBetter`: a falling waiting time is good news.
- The period is counted back from the newest observation in the data, not from today, so a feed
  that stopped a month ago still shows its last months instead of an empty dashboard.

## Data
`KeyPerformanceIndicator` (`model.linkml.yaml`), one row per indicator per month.
