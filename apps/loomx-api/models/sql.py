"""Pydantic models — SQL."""

from typing import Any, Optional
from pydantic import BaseModel, Field


class SqlGenerateBody(BaseModel):
    dataset_id: int
    chart_type: str = Field(..., min_length=1, max_length=100)
    config: dict[str, Any]


class SqlExecuteBody(BaseModel):
    sql_text: str = Field(..., min_length=1, max_length=65_536)
    database: str = Field(..., min_length=1, max_length=255)
    source: Optional[str] = Field(default=None, max_length=64)
    tables_used: Optional[str] = None
    chart_id: Optional[int] = None
    dashboard_id: Optional[str] = None
    chart_type: Optional[str] = Field(default=None, max_length=100)
    dataset_id: Optional[int] = None
    row_limit: Optional[int] = Field(default=None, ge=1, le=5000)
