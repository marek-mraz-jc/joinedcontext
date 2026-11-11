import type { MapFailure } from "./map";
import { MAP_FAILED, MAP_RETRY } from "./map";

/** The box of a map that could not start (T-6737): why, and a retry where one can help. */
export function MapFailed({ failure, onRetry }: { failure: MapFailure; onRetry: () => void }) {
  return (
    <div className="map map-failed" data-testid="map-failed">
      <p role="status">{MAP_FAILED[failure]}</p>
      {/* No WebGL 2 is the browser's setting: another try draws nothing more. */}
      {failure !== "webgl" && (
        <button type="button" className="map-retry" onClick={onRetry}>
          {MAP_RETRY}
        </button>
      )}
    </div>
  );
}
