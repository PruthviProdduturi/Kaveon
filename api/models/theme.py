"""Pydantic models — Theme."""

from pydantic import BaseModel, Field


class ThemeUpdate(BaseModel):
    theme_color: str = Field(..., min_length=1, max_length=32)
