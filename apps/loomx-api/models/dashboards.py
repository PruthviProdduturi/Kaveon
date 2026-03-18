"""Pydantic models — Dashboards."""

from typing import Any, Optional
from pydantic import BaseModel, Field


class DashboardCreate(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    layout: Optional[dict[str, Any]] = None
    charts: Optional[list[Any]] = None


class DashboardUpdate(BaseModel):
    name: Optional[str] = Field(default=None, min_length=1, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    layout: Optional[dict[str, Any]] = None
    charts: Optional[list[Any]] = None


class DashboardFavoriteBody(BaseModel):
    is_favorite: bool
