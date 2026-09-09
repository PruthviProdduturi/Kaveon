"""Pydantic models — SQL."""

from typing import Any, Optional
from pydantic import BaseModel, Field, field_validator


class SqlGenerateBody(BaseModel):
    dataset_id: int
    chart_type: str = Field(..., min_length=1, max_length=100)
    config: dict[str, Any]


class SqlExecuteBody(BaseModel):
    sql_text: str = Field(..., min_length=1, max_length=65_536)
    database: str = Field(..., min_length=1, max_length=255)
    source: Optional[str] = Field(default=None, max_length=64)
    tables_used: Optional[str] = None
    chart_id: Optional[str] = Field(default=None, max_length=36)
    dashboard_id: Optional[str] = None
    chart_type: Optional[str] = Field(default=None, max_length=100)
    dataset_id: Optional[int] = None
    row_limit: Optional[int] = Field(default=None, ge=1, le=5000)
    use_cache: Optional[bool] = Field(default=False)
    cache_ttl: Optional[int] = Field(default=300, ge=30, le=3600)

    @field_validator("chart_id", mode="before")
    @classmethod
    def chart_id_to_string(cls, value):
        # Existing clients posted numeric chart IDs; PostgreSQL chart IDs are
        # UUID text.  Normalize both at the API boundary for history context.
        return str(value) if value is not None else None
