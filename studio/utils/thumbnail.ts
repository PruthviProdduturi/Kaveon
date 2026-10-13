/**
 * Library thumbnail encoding.
 *
 * A thumbnail is a preview, not an export: it is only ever drawn into the
 * Library card's 128px-tall cover. So whatever a renderer hands us — an ECharts
 * canvas snapshot, an html-to-image raster — is normalised here to one
 * deliberate geometry and one deliberate budget before it is stored:
 *
 *   geometry  longest edge capped, source aspect preserved
 *             (the cover is 128px tall on cards 160-280px wide, so a few
 *              hundred pixels still has headroom on a 2x display and nothing
 *              is spent beyond that)
 *   format    JPEG — a chart is flat colour and text, and at this size JPEG is
 *             three to five times smaller than PNG with no visible difference.
 *             WebP would be smaller again, but a browser without it silently
 *             returns PNG from toDataURL, which would quietly make the payload
 *             bigger instead of smaller.
 *   budget    a data-URI character cap, with one smaller retry, then give up
 *
 * The budget is the point of this module. The API refuses a preview that
 * exceeds the record's stored-preview limit (`services/thumbnails.py`:
 * 64,000 characters for a chart, 256,000 for a dashboard), and a refusal
 * reaches the browser as a 422 that the capture has no way to retry. So every
 * capture is normalised to a budget that is strictly below the one the API
 * enforces, and a capture that cannot be brought under it is reported as
 * having produced nothing rather than sent and rejected.
 *
 * Nothing here throws: a caller that cannot produce a thumbnail gets null and
 * the Library card draws its placeholder.
 */

/** One normalisation target: the geometry to aim for and the cap to stay under. */
export interface ThumbnailBudget {
	/** Largest data URI we are willing to store for one preview. */
	maxChars: number;
	primary: { maxEdge: number; quality: number };
	/** Tried once when the primary encoding overruns the cap. */
	fallback: { maxEdge: number; quality: number };
}

/**
 * A chart preview is one plot, so a modest geometry is already generous for a
 * 128px cover. Stays well under the API's 64,000-character chart limit.
 */
export const CHART_THUMBNAIL: ThumbnailBudget = {
	maxChars: 48_000,
	primary: { maxEdge: 480, quality: 0.72 },
	fallback: { maxEdge: 320, quality: 0.55 },
};

/**
 * A dashboard preview is a whole canvas rather than one plot — often several
 * times taller than it is wide — so the cover crops it and a larger longest
 * edge is what keeps the visible band from going soft. Stays under the API's
 * 256,000-character dashboard limit with room for a differently proportioned
 * capture.
 */
export const DASHBOARD_THUMBNAIL: ThumbnailBudget = {
	maxChars: 160_000,
	primary: { maxEdge: 960, quality: 0.72 },
	fallback: { maxEdge: 640, quality: 0.55 },
};

/**
 * The colour a node is actually drawn against, walking up until something
 * paints. Read from the DOM rather than from a theme constant so the capture
 * matches what the viewer saw, whichever theme is active. White when nothing
 * in the chain paints, which is the right ground for a chart.
 */
export function surfaceColor(node: HTMLElement | null): string {
	const transparent = /^(transparent|rgba\(\s*0\s*,\s*0\s*,\s*0\s*,\s*0\s*\))$/;
	try {
		for (let el: HTMLElement | null = node; el; el = el.parentElement) {
			const background = window.getComputedStyle(el).backgroundColor;
			if (background && !transparent.test(background.trim())) return background;
		}
	} catch {
		// A detached node or a context without computed styles: fall through.
	}
	return "#ffffff";
}

function loadImage(source: string): Promise<HTMLImageElement> {
	return new Promise((resolve, reject) => {
		const image = new Image();
		image.onload = () => resolve(image);
		image.onerror = () => reject(new Error("thumbnail source could not be decoded"));
		image.src = source;
	});
}

function encode(
	image: HTMLImageElement,
	background: string,
	{ maxEdge, quality }: { maxEdge: number; quality: number },
): string | null {
	const width = image.naturalWidth || image.width;
	const height = image.naturalHeight || image.height;
	if (!width || !height) return null;

	const scale = Math.min(1, maxEdge / Math.max(width, height));
	const canvas = document.createElement("canvas");
	canvas.width = Math.max(1, Math.round(width * scale));
	canvas.height = Math.max(1, Math.round(height * scale));

	const context = canvas.getContext("2d");
	if (!context) return null;
	// JPEG has no alpha: paint the surface colour first so transparent regions
	// read as the chart's own background rather than as black.
	context.fillStyle = background;
	context.fillRect(0, 0, canvas.width, canvas.height);
	context.drawImage(image, 0, 0, canvas.width, canvas.height);
	return canvas.toDataURL("image/jpeg", quality);
}

/**
 * Normalise a rendered preview into a stored thumbnail. Returns null when the
 * source cannot be decoded or will not fit the budget.
 */
export async function encodeThumbnail(
	source: string | null,
	background: string,
	budget: ThumbnailBudget = CHART_THUMBNAIL,
): Promise<string | null> {
	if (!source || !source.startsWith("data:")) return null;
	try {
		const image = await loadImage(source);
		const primary = encode(image, background, budget.primary);
		if (primary && primary.length <= budget.maxChars) return primary;
		const fallback = encode(image, background, budget.fallback);
		return fallback && fallback.length <= budget.maxChars ? fallback : null;
	} catch {
		return null;
	}
}

/**
 * The device-pixel scale to rasterise a node at before normalisation.
 *
 * html-to-image allocates the whole raster at once, and a dashboard canvas can
 * be several thousand pixels tall, so the source is captured no larger than it
 * needs to be to survive the downscale: a little over the budget's longest
 * edge, and never magnified past 1:1.
 */
export function capturePixelRatio(node: HTMLElement, budget: ThumbnailBudget): number {
	// Scroll extent as well as box size: html-to-image rasterises the whole
	// subtree, which for a dashboard canvas inside a scroller is taller than the
	// box the layout gives it.
	const longest = Math.max(
		node.offsetWidth || 0, node.offsetHeight || 0,
		node.scrollWidth || 0, node.scrollHeight || 0,
	);
	if (!longest) return 1;
	return Math.min(1, (budget.primary.maxEdge * 1.25) / longest);
}
