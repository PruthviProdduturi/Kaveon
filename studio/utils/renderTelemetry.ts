/**
 * Render timing for chart surfaces.
 *
 * A large geographic result can take a visible moment to paint, and until now
 * nothing separated a slow query from a slow render -- both arrived as "the
 * dashboard is slow". This records the paint half.
 *
 * Samples are written as browser performance marks and measures, so they show
 * up in devtools and in whatever real monitoring the host page installs, and
 * are also kept in a small bounded ring so the last few can be read back
 * without one. There is no console output and no network call here: this is
 * instrumentation, not logging.
 */

export interface RenderSample {
  /** What rendered, e.g. `map:world`. */
  surface: string;
  /** Milliseconds from the option being produced to the paint finishing. */
  durationMs: number;
  /** Rows the surface was asked to draw. */
  rows: number;
  /** Rows that reached the canvas, when that differs -- unmatched geography never does. */
  drawn?: number;
  /** Wall-clock time of the sample, for ordering across surfaces. */
  at: number;
}

const RING_SIZE = 20;
const ring: RenderSample[] = [];
const started = new Map<string, number>();

const now = (): number =>
  typeof performance !== "undefined" && typeof performance.now === "function"
    ? performance.now()
    : Date.now();

/** Note that `surface` has an option in hand and is about to paint. */
export function startRender(surface: string): void {
  started.set(surface, now());
  try {
    performance.mark(`kaveon:${surface}:start`);
  } catch {
    // Marks are a convenience. A browser that refuses them must not take the
    // chart down with it, and the in-memory sample below still holds.
  }
}

/**
 * Note that `surface` finished painting, and keep the sample.
 *
 * Returns null when no start was recorded, because a paint can finish for a
 * reason other than a new option -- a pan, a resize, a theme change -- and
 * timing one of those against a stale start would invent a number.
 */
export function endRender(
  surface: string,
  facts: { rows: number; drawn?: number },
): RenderSample | null {
  const start = started.get(surface);
  if (start === undefined) return null;
  started.delete(surface);

  const sample: RenderSample = {
    surface,
    durationMs: now() - start,
    rows: facts.rows,
    ...(facts.drawn !== undefined ? { drawn: facts.drawn } : {}),
    at: Date.now(),
  };
  try {
    performance.measure(`kaveon:${surface}`, {
      start: `kaveon:${surface}:start`,
      detail: sample,
    });
  } catch {
    // Same reasoning: the measure is for whoever is watching, the ring is the
    // record this module promises.
  }
  ring.push(sample);
  if (ring.length > RING_SIZE) ring.shift();
  return sample;
}

/** The last few render samples, oldest first. */
export function renderSamples(): RenderSample[] {
  return ring.slice();
}
