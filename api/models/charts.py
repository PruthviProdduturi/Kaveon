"""Pydantic models — Charts."""

from typing import Any, Literal, Optional
from pydantic import BaseModel, Field, field_validator

from services import thumbnails


class ChartCreate(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    dataset_id: int
    chart_type: str = Field(..., min_length=1, max_length=100)
    config: Optional[dict[str, Any]] = None
    query_config: Optional[dict[str, Any]] = None
    viz_config: Optional[dict[str, Any]] = None
    description: Optional[str] = Field(default=None, max_length=1000)
    visibility: Optional[Literal["private", "internal", "published"]] = None


class ChartUpdate(BaseModel):
    name: Optional[str] = Field(default=None, min_length=1, max_length=255)
    dataset_id: Optional[int] = None
    chart_type: Optional[str] = Field(default=None, min_length=1, max_length=100)
    config: Optional[dict[str, Any]] = None
    query_config: Optional[dict[str, Any]] = None
    viz_config: Optional[dict[str, Any]] = None
    description: Optional[str] = Field(default=None, max_length=1000)
    # A base64 data-URI preview captured client-side after the chart renders.
    # Bounded here as well as in services.thumbnails so an oversized capture is
    # refused at the edge rather than carried into a product record.
    thumbnail: Optional[str] = Field(default=None, max_length=thumbnails.CHART_MAX_CHARS)

    @field_validator("thumbnail")
    @classmethod
    def _validate_thumbnail(cls, value: Optional[str]) -> Optional[str]:
        return thumbnails.normalise(value, thumbnails.CHART_MAX_CHARS)
