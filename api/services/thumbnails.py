"""Stored preview images for Library cards.

A thumbnail is a small raster captured in the browser once a chart or dashboard
has finished rendering (see `studio/utils/thumbnail.ts`). It lives inside the
product record, so it is bounded on the way in: a product document is written
to KaveonDB as one statement, and an unbounded preview would let a decoration
dominate the record it decorates.

Reads are served out of band — `GET /charts/{id}/thumbnail` and
`GET /dashboards/{id}/thumbnail` — so list responses carry only whether a
preview exists. Seventy inline data URIs on every Library load would be
megabytes of JSON that the browser could not cache.
"""

import base64
import binascii
from typing import Optional

# A chart preview is normalised client-side to a 480px JPEG inside a 48KB data
# URI; this leaves headroom for a differently proportioned capture.
CHART_MAX_CHARS = 64_000
# A dashboard preview is a whole canvas rather than one plot, and is captured
# twice (light and dark), so it is allowed more room while still being far too
# small to be mistaken for an export.
DASHBOARD_MAX_CHARS = 256_000

_MEDIA_TYPES = {"image/jpeg": ".jpg", "image/png": ".png", "image/webp": ".webp"}


def normalise(value: Optional[str], max_chars: int) -> Optional[str]:
    """Validate a captured preview for storage.

    Returns the data URI unchanged, or None when there is nothing to store.
    Raises ValueError for anything that is not a bounded base64 image data URI,
    so a malformed or oversized capture is refused rather than persisted.
    """
    if value is None or value == "":
        return None
    if not isinstance(value, str):
        raise ValueError("A thumbnail must be a base64 image data URI")
    media_type, _, payload = _split(value)
    if media_type not in _MEDIA_TYPES or not payload:
        raise ValueError("A thumbnail must be a base64 JPEG, PNG or WebP data URI")
    if len(value) > max_chars:
        raise ValueError("The thumbnail exceeds the stored preview size limit")
    return value


def decode(value: str) -> tuple[bytes, str]:
    """Return the image bytes and media type of a stored thumbnail data URI."""
    media_type, _, payload = _split(value)
    if media_type not in _MEDIA_TYPES:
        raise ValueError("The stored thumbnail is not a supported image type")
    try:
        return base64.b64decode(payload, validate=True), media_type
    except (binascii.Error, ValueError) as error:
        raise ValueError("The stored thumbnail is not valid base64") from error


def _split(value: str) -> tuple[str, str, str]:
    if not value.startswith("data:"):
        raise ValueError("A thumbnail must be a data URI")
    header, separator, payload = value[len("data:"):].partition(",")
    if not separator:
        raise ValueError("A thumbnail data URI has no payload")
    media_type, _, parameters = header.partition(";")
    if parameters != "base64":
        raise ValueError("A thumbnail data URI must be base64 encoded")
    return media_type.strip().lower(), parameters, payload
