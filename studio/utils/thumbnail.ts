/**
 * Library thumbnail encoding.
 *
 * A thumbnail is a preview, not an export: it is only ever drawn into the
 * Library card's 128px-tall cover. So whatever a renderer hands us — an ECharts
 * canvas snapshot, an html-to-image raster — is normalised here to one
 * deliberate geometry and one deliberate budget before it is stored:
 *
 *   geometry  longest edge 480px, source aspect preserved
 *             (the cover is 128px tall on cards 160-280px wide, so 480px still
 *              has headroom on a 2x display and nothing is spent beyond that)
 *   format    JPEG — a chart is flat colour and text, and at this size JPEG is
 *             three to five times smaller than PNG with no visible difference.
 *             WebP would be smaller again, but a browser without it silently
 *             returns PNG from toDataURL, which would quietly make the payload
 *             bigger instead of smaller.
 *   budget    48KB of data URI, with one smaller retry, then give up
 *
 * Nothing here throws: a caller that cannot produce a thumbnail gets null and
 * the Library card draws its placeholder.
 */

/** Largest data URI we are willing to store for one preview. */
export const THUMBNAIL_MAX_CHARS = 48_000;

const PRIMARY = { maxEdge: 480, quality: 0.72 };
const FALLBACK = { maxEdge: 320, quality: 0.55 };

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
export async function encodeThumbnail(source: string | null, background: string): Promise<string | null> {
	if (!source || !source.startsWith("data:")) return null;
	try {
		const image = await loadImage(source);
		const primary = encode(image, background, PRIMARY);
		if (primary && primary.length <= THUMBNAIL_MAX_CHARS) return primary;
		const fallback = encode(image, background, FALLBACK);
		return fallback && fallback.length <= THUMBNAIL_MAX_CHARS ? fallback : null;
	} catch {
		return null;
	}
}
