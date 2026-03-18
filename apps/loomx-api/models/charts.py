"""Pydantic models — Charts."""

from typing import Any, Optional
from pydantic import BaseModel, Field


class ChartCreate(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    dataset_id: int
    chart_type: str = Field(..., min_length=1, max_length=100)
    config: Optional[dict[str, Any]] = None
    description: Optional[str] = Field(default=None, max_length=1000)


class ChartUpdate(BaseModel):
    name: Optional[str] = Field(default=None, min_length=1, max_length=255)
    dataset_id: Optional[int] = None
    chart_type: Optional[str] = Field(default=None, min_length=1, max_length=100)
    config: Optional[dict[str, Any]] = None
    description: Optional[str] = Field(default=None, max_length=1000)
