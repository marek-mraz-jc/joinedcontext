// The chart theme (T-6886): crates/shared/jc-chart/src/theme.json as a classic script, held
// equal to that file by bins/jc/src/modules/viz.rs `the_theme_script_is_the_theme_file`.
self.jcTheme = {
  "light": {
    "background": "#ffffff",
    "font": "Inter, system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif",
    "padding": 8,
    "view": { "stroke": null },
    "title": { "color": "#1f2937", "fontSize": 15, "fontWeight": 600, "anchor": "start", "offset": 10 },
    "axis": {
      "labelColor": "#4b5563",
      "titleColor": "#1f2937",
      "labelFontSize": 12,
      "titleFontSize": 12,
      "titleFontWeight": 600,
      "titlePadding": 8,
      "gridColor": "#e5e7eb",
      "domainColor": "#6b7280",
      "tickColor": "#6b7280",
      "labelLimit": 160
    },
    "legend": {
      "labelColor": "#4b5563",
      "titleColor": "#1f2937",
      "labelFontSize": 12,
      "titleFontSize": 12,
      "titleFontWeight": 600,
      "symbolType": "square"
    },
    "range": {
      "category": ["#1d4ed8", "#d97706", "#7e22ce", "#0d9488", "#be123c", "#65a30d", "#475569", "#db2777"]
    },
    "mark": { "color": "#1d4ed8" },
    "line": { "strokeWidth": 2.5 },
    "point": { "filled": true, "size": 48 },
    "area": { "fillOpacity": 0.3, "line": true },
    "rule": { "strokeWidth": 2 },
    "text": { "color": "#1f2937" },
    "arc": { "stroke": "#ffffff", "strokeWidth": 1 }
  },
  "dark": {
    "background": "#16191d",
    "font": "Inter, system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif",
    "padding": 8,
    "view": { "stroke": null },
    "title": { "color": "#e5e7eb", "fontSize": 15, "fontWeight": 600, "anchor": "start", "offset": 10 },
    "axis": {
      "labelColor": "#a0a8b3",
      "titleColor": "#e5e7eb",
      "labelFontSize": 12,
      "titleFontSize": 12,
      "titleFontWeight": 600,
      "titlePadding": 8,
      "gridColor": "#2b3038",
      "domainColor": "#8b939e",
      "tickColor": "#8b939e",
      "labelLimit": 160
    },
    "legend": {
      "labelColor": "#a0a8b3",
      "titleColor": "#e5e7eb",
      "labelFontSize": 12,
      "titleFontSize": 12,
      "titleFontWeight": 600,
      "symbolType": "square"
    },
    "range": {
      "category": ["#60a5fa", "#fcd34d", "#c084fc", "#5eead4", "#f87171", "#a3e635", "#f472b6", "#7dd3fc"]
    },
    "mark": { "color": "#60a5fa" },
    "line": { "strokeWidth": 2.5 },
    "point": { "filled": true, "size": 48 },
    "area": { "fillOpacity": 0.3, "line": true },
    "rule": { "strokeWidth": 2 },
    "text": { "color": "#e5e7eb" },
    "arc": { "stroke": "#16191d", "strokeWidth": 1 }
  }
};
