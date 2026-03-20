"""Pydantic models — Datasets."""

from typing import Any, Optional
from pydantic import BaseModel, Field


class DatasetCreate(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    # table_name is optional — omit when creating a virtual (SQL-backed) dataset
    table_name: Optional[str] = Field(default=None, max_length=255)
    # sql_text is stored for virtual datasets created from Lab queries
    sql_text: Optional[str] = None
    schema_name: Optional[str] = Field(default=None, max_length=128)
    database_name: Optional[str] = Field(default=None, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    dimensions: Optional[list[dict[str, Any]]] = None
    columns: Optional[list[dict[str, Any]]] = None


class DatasetUpdate(BaseModel):
    name: Optional[str] = Field(default=None, min_length=1, max_length=255)
    table_name: Optional[str] = Field(default=None, max_length=255)
    sql_text: Optional[str] = None
    schema_name: Optional[str] = Field(default=None, max_length=128)
    database_name: Optional[str] = Field(default=None, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    dimensions: Optional[list[dict[str, Any]]] = None
    columns: Optional[list[dict[str, Any]]] = None
