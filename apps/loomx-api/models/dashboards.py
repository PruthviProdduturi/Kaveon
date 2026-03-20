"""Pydantic models — Dashboards."""

from typing import Any, Optional
from pydantic import BaseModel, Field


class DashboardCreate(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    layout: Optional[Any] = None
    charts: Optional[Any] = None


class DashboardUpdate(BaseModel):
    name: Optional[str] = Field(default=None, min_length=1, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    layout: Optional[Any] = None
    charts: Optional[Any] = None


class DashboardFavoriteBody(BaseModel):
    is_favorite: bool
