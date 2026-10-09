"""Pydantic models — Dashboards."""

from typing import Any, Literal, Optional
from pydantic import BaseModel, Field, field_validator

from services import thumbnails


class DashboardCreate(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    theme: Optional[str] = Field(default=None, max_length=32)
    layout: Optional[Any] = None
    charts: Optional[Any] = None
    filters: Optional[Any] = None
    visibility: Optional[Literal["private", "internal", "published"]] = None


class DashboardUpdate(BaseModel):
    name: Optional[str] = Field(default=None, min_length=1, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    theme: Optional[str] = Field(default=None, max_length=32)
    thumbnail: Optional[str] = Field(default=None, max_length=thumbnails.DASHBOARD_MAX_CHARS)
    thumbnail_dark: Optional[str] = Field(default=None, max_length=thumbnails.DASHBOARD_MAX_CHARS)

    @field_validator("thumbnail", "thumbnail_dark")
    @classmethod
    def _validate_thumbnail(cls, value: Optional[str]) -> Optional[str]:
        return thumbnails.normalise(value, thumbnails.DASHBOARD_MAX_CHARS)
    layout: Optional[Any] = None
    charts: Optional[Any] = None
    filters: Optional[Any] = None
    visibility: Optional[Literal["private", "internal", "published"]] = None
    is_published: Optional[bool] = None


class DashboardFavoriteBody(BaseModel):
    is_favorite: bool
