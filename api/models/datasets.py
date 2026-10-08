"""Pydantic models — Datasets."""

from typing import Any, ClassVar, Literal, Optional
from pydantic import BaseModel, Field, model_validator


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
    """An update carries only the fields the caller sent. A field the caller
    omits is left as it stands; a field the caller sends as `null` is cleared.

    `NOT_CLEARABLE` names the fields that have no cleared state — a dataset
    always has a name, a table, a schema and a visibility, and a component
    collection is emptied with `[]` rather than with `null`. Sending `null`
    for one of those is a caller mistake, answered with 422 rather than with
    a constraint violation from the store or a silently ignored field."""

    NOT_CLEARABLE: ClassVar[tuple[str, ...]] = (
        "name", "table_name", "schema_name", "visibility",
        "dimensions", "columns", "metrics",
    )

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

    @model_validator(mode="before")
    @classmethod
    def _reject_unclearable_nulls(cls, data: Any) -> Any:
        if not isinstance(data, dict):
            return data
        sent_null = [f for f in cls.NOT_CLEARABLE if f in data and data[f] is None]
        if sent_null:
            raise ValueError(
                f"{', '.join(sent_null)} cannot be cleared; omit the field to leave it unchanged"
            )
        return data
