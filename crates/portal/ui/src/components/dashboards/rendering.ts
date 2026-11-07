/**
 * The renderer a map layer belongs to (UI-21).
 *
 * Its own module, free of any deck.gl import, so the dashboards page can ask the question
 * without pulling the WebGL bundle into the main chunk to answer it.
 */

/**
 * The count at which native vector rendering stops being the right tool.
 *
 * Below it MapLibre draws crisper features and keeps label collision and zoom transitions;
 * at and above it the browser spends its frame budget on DOM-side geometry it cannot
 * finish, so the same features go to the GPU as a deck.gl overlay instead.
 */
export const DECK_GL_THRESHOLD = 50_000;

/**
 * Whether this layer is the overlay's rather than MapLibre's.
 *
 * An aggregation is deck.gl's whether the dataset is large or not: `hexagon` and `heatmap`
 * are not styles MapLibre can draw from point features at all.
 */
export function rendersWithDeckGl(style: string, featureCount: number): boolean {
  return style === "hexagon" || style === "heatmap" || featureCount >= DECK_GL_THRESHOLD;
}
