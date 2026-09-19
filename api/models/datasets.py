"""Pydantic models — Datasets."""

from typing import Any, Literal, Optional
from pydantic import BaseModel, Field


class DatasetSource(BaseModel):
    """Where a dataset's rows live. `engine` binds the dataset to one table of
    the Engine's durable catalog by its id: the catalog, schema and table
    names, the columns and — when the table declares a shape — the dimensions
    and measures are read from the Engine's definition rather than typed by
    hand. A dataset without a source is a warehouse dataset over
    `database_name`."""
    kind: Literal["engine"]
    table_id: str = Field(..., min_length=1, max_length=512)


class DatasetCreate(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    source: Optional[DatasetSource] = None
    table_name: Optional[str] = Field(default=None, max_length=255)
    sql_text: Optional[str] = None
    schema_name: Optional[str] = Field(default=None, max_length=128)
    database_name: Optional[str] = Field(default=None, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    dimensions: Optional[list[dict[str, Any]]] = None
    columns: Optional[list[dict[str, Any]]] = None
    metrics: Optional[list[dict[str, Any]]] = None
    date_column: Optional[str] = Field(default=None, max_length=128)
    visibility: Optional[Literal["private", "internal", "published"]] = None


class DatasetUpdate(BaseModel):
    name: Optional[str] = Field(default=None, min_length=1, max_length=255)
    source: Optional[DatasetSource] = None
    table_name: Optional[str] = Field(default=None, max_length=255)
    sql_text: Optional[str] = None
    schema_name: Optional[str] = Field(default=None, max_length=128)
    database_name: Optional[str] = Field(default=None, max_length=255)
    description: Optional[str] = Field(default=None, max_length=1000)
    dimensions: Optional[list[dict[str, Any]]] = None
    columns: Optional[list[dict[str, Any]]] = None
    metrics: Optional[list[dict[str, Any]]] = None
    date_column: Optional[str] = Field(default=None, max_length=128)
    visibility: Optional[Literal["private", "internal", "published"]] = None
