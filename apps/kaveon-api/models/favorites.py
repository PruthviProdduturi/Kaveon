"""Pydantic models — Favorites."""

from pydantic import BaseModel, Field


class FavoriteCreate(BaseModel):
    object_type: str = Field(..., min_length=1, max_length=64)
    object_id: str = Field(..., min_length=1, max_length=64)
    object_name: str = Field(..., min_length=1, max_length=255)
