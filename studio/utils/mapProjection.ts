/**
 * Geo-chart projection and layout geometry.
 *
 * Two problems are solved here.
 *
 * 1. Fit. ECharts sizes a geo coordinate system from `layoutSize`, and a
 *    percentage `layoutSize` resolves against the *smaller* side of the
 *    container (see `coord/geo/geoCreator`). A fixed percentage therefore
 *    overflows every container whose aspect ratio is narrower than the map's,
 *    which is exactly what a dashboard tile becomes as it gets smaller — and
 *    no amount of roaming recovers it, because the fitted zoom is already the
 *    minimum. `fitGeoLayout` turns a measured container into the explicit
 *    pixel size and centre that make the map fill the tile and still fit it.
 *
 * 2. Legibility at small sizes. An equirectangular world map narrower than a
 *    few hundred pixels gives under a pixel per degree of longitude, so the
 *    populated parts of the world collapse into noise. Below that width the
 *    map switches to an orthographic projection — a globe — which spends the
 *    whole of the tile's shorter side on a single hemisphere and roughly
 *    doubles the scale of the regions that carry the data.
 *
 * The orthographic projection is plain 2-D canvas work through ECharts'
 * `projection` hook: no WebGL, so it themes, labels, tooltips, exports and
 * screenshots exactly like the flat map does.
 */

const DEG = Math.PI / 180;

/** Map name the orthographic world is registered under. */
export const GLOBE_MAP_NAME = "kaveon-world-globe";

/** Name of the synthetic region that draws the sphere itself. */
export const GLOBE_SPHERE_REGION = "__kaveon_sphere__";

/** Container width (px, measured on the chart's own box) that turns the globe on. */
export const GLOBE_ENTER_WIDTH = 360;

/** Width the container has to regain before the flat map returns. */
export const GLOBE_EXIT_WIDTH = 408;

/** Container aspect (w/h) at or below which a flat world map wastes the tile. */
export const GLOBE_ENTER_ASPECT = 1.1;

/** Aspect the container has to regain before the flat map returns. */
export const GLOBE_EXIT_ASPECT = 1.25;

export type MapProjectionMode = "flat" | "globe";

export interface Size {
  width: number;
  height: number;
}

/**
 * Which projection a container of this size should use, given the one it is
 * showing. The enter and exit bounds differ so that dragging a tile handle
 * across the threshold cannot oscillate between the two.
 */
export function nextProjectionMode(current: MapProjectionMode, size: Size): MapProjectionMode {
  const { width, height } = size;
  if (!(width > 0) || !(height > 0)) return current;
  const aspect = width / height;
  if (current === "flat") {
    return width < GLOBE_ENTER_WIDTH || aspect < GLOBE_ENTER_ASPECT ? "globe" : "flat";
  }
  return width >= GLOBE_EXIT_WIDTH && aspect >= GLOBE_EXIT_ASPECT ? "flat" : "globe";
}

export interface Projection {
  project: (point: number[]) => number[] | null;
  unproject: (point: number[]) => number[] | null;
}

/**
 * Orthographic projection centred on (lng0, lat0), normalised to a unit disc.
 *
 * Points on the far side of the sphere return `null`. ECharts drops null
 * points when it builds a region's path and when it measures the projected
 * bounding box, so a region behind the horizon simply is not drawn and one
 * crossing the horizon is drawn from its visible vertices.
 */
export function createOrthographicProjection(lng0: number, lat0: number): Projection {
  const phi0 = lat0 * DEG;
  const cosPhi0 = Math.cos(phi0);
  const sinPhi0 = Math.sin(phi0);

  return {
    project: (point: number[]) => {
      const lambda = (point[0] - lng0) * DEG;
      const phi = point[1] * DEG;
      const cosPhi = Math.cos(phi);
      const sinPhi = Math.sin(phi);
      const cosLambda = Math.cos(lambda);
      // Cosine of the angular distance from the centre of the view; negative
      // means the point sits behind the horizon.
      if (sinPhi0 * sinPhi + cosPhi0 * cosPhi * cosLambda < 0) return null;
      // y is returned screen-down, the direction ECharts draws in.
      return [cosPhi * Math.sin(lambda), -(cosPhi0 * sinPhi - sinPhi0 * cosPhi * cosLambda)];
    },
    unproject: (point: number[]) => {
      const x = point[0];
      const y = -point[1];
      const rho = Math.min(1, Math.sqrt(x * x + y * y));
      const c = Math.asin(rho);
      const cosC = Math.cos(c);
      const lat = Math.asin(cosC * sinPhi0 + y * cosPhi0);
      const lng = lng0 + Math.atan2(x, cosC * cosPhi0 - y * sinPhi0) / DEG;
      return [lng, lat / DEG];
    },
  };
}

/** Point at angular distance `distDeg` from (lng0, lat0) on bearing `bearingDeg`. */
function pointAtBearing(lng0: number, lat0: number, distDeg: number, bearingDeg: number): [number, number] {
  const phi0 = lat0 * DEG;
  const d = distDeg * DEG;
  const theta = bearingDeg * DEG;
  const sinPhi = Math.sin(phi0) * Math.cos(d) + Math.cos(phi0) * Math.sin(d) * Math.cos(theta);
  const phi = Math.asin(Math.max(-1, Math.min(1, sinPhi)));
  const lambda = Math.atan2(
    Math.sin(theta) * Math.sin(d) * Math.cos(phi0),
    Math.cos(d) - Math.sin(phi0) * sinPhi,
  );
  return [lng0 + lambda / DEG, phi / DEG];
}

/**
 * The world GeoJSON with a sphere region prepended, oriented for a view
 * centred on (lng0, lat0).
 *
 * The sphere is a ring just inside the horizon, so it projects to the unit
 * circle: it paints the ocean, it draws the limb, and — because it is part of
 * the same registered map — it pins the projected bounding box to the unit
 * square. That makes the fitted disc exactly `layoutSize` across whatever
 * subset of the world happens to be in view, so the globe neither drifts nor
 * resizes as the data changes.
 */
export function buildGlobeGeoJson(world: any, lng0: number, lat0: number): any {
  const ring: [number, number][] = [];
  const steps = 180;
  for (let i = 0; i <= steps; i++) {
    ring.push(pointAtBearing(lng0, lat0, 89.95, (i / steps) * 360));
  }
  const sphere = {
    type: "Feature",
    properties: { name: GLOBE_SPHERE_REGION },
    geometry: { type: "Polygon", coordinates: [ring] },
  };
  return { ...world, type: "FeatureCollection", features: [sphere, ...(world?.features ?? [])] };
}

export interface GeoLayout {
  /** Explicit pixel size for `layoutSize`. */
  layoutSize: number;
  /** Explicit pixel centre for `layoutCenter`. */
  layoutCenter: [number, number];
  /** Whether the legend fits beside the map without covering it. */
  showLegend: boolean;
}

export interface GeoLayoutInput {
  width: number;
  height: number;
  /** Projected bounding-box aspect, already multiplied by the geo's aspectScale. */
  aspect: number;
  /** Space to keep clear at the top (a chart title lives there). */
  padTop: number;
  pad: number;
  legendWidth: number;
  legendHeight: number;
}

/**
 * Pixel size and centre that make a geo coordinate system as large as it can
 * be while staying wholly inside its container — and, when there is room
 * beside it, a column reserved for the legend so the two never overlap.
 */
export function fitGeoLayout(input: GeoLayoutInput): GeoLayout {
  const { width, height, aspect, padTop, pad, legendWidth, legendHeight } = input;
  const availW = Math.max(1, width - pad * 2);
  const availH = Math.max(1, height - padTop - pad);

  // `layoutSize` is the long side of the map's box: the width when the map is
  // wider than it is tall, the height otherwise (see `resizeGeo`).
  const sizeFor = (boxW: number) =>
    aspect >= 1 ? Math.min(boxW, availH * aspect) : Math.min(availH, boxW / aspect);

  const full = sizeFor(availW);
  const reservedW = availW - legendWidth;
  const reserved = reservedW >= 140 ? sizeFor(reservedW) : 0;
  // Only give the legend its column when it both fits vertically and does not
  // cost the map more than a fifth of its size.
  const showLegend = reserved > 0 && availH >= legendHeight && reserved >= full * 0.8;

  const size = showLegend ? reserved : full;
  const boxW = showLegend ? reservedW : availW;
  const centerX = pad + (showLegend ? legendWidth : 0) + boxW / 2;
  const centerY = padTop + availH / 2;

  return {
    layoutSize: Math.max(1, Math.round(size)),
    layoutCenter: [Math.round(centerX), Math.round(centerY)],
    showLegend,
  };
}
