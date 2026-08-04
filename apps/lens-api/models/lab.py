"""Pydantic models — Lab."""

from typing import Any, Optional
from pydantic import BaseModel, Field, field_validator


class SavedQueryCreate(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    sql: str = Field(..., min_length=1)
    description: Optional[str] = Field(default=None, max_length=1000)

    @field_validator("sql")
    @classmethod
    def sql_max_size(cls, v: str) -> str:
        if len(v.encode("utf-8")) > 65_536:
            raise ValueError("SQL exceeds maximum allowed size (64 KB)")
        return v


class SavedQueryUpdate(BaseModel):
    name: Optional[str] = Field(default=None, min_length=1, max_length=255)
    sql: Optional[str] = None
    description: Optional[str] = Field(default=None, max_length=1000)

    @field_validator("sql")
    @classmethod
    def sql_max_size(cls, v: Optional[str]) -> Optional[str]:
        if v is not None and len(v.encode("utf-8")) > 65_536:
            raise ValueError("SQL exceeds maximum allowed size (64 KB)")
        return v


class LabExecuteBody(BaseModel):
    sql: str = Field(..., min_length=1)
    database: Optional[str] = Field(default=None, max_length=255)

    @field_validator("sql")
    @classmethod
    def sql_max_size(cls, v: str) -> str:
        if len(v.encode("utf-8")) > 65_536:
            raise ValueError("Query exceeds maximum allowed size (64 KB)")
        return v


class LabQueryBody(BaseModel):
    query: str = Field(..., min_length=1)
    database: Optional[str] = Field(default=None, max_length=255)
    datasetId: Optional[int] = None
    runContext: Optional[str] = Field(default=None, max_length=64)
    tablesUsed: Optional[list[str]] = None

    @field_validator("query")
    @classmethod
    def query_max_size(cls, v: str) -> str:
        if len(v.encode("utf-8")) > 65_536:
            raise ValueError("Query exceeds maximum allowed size (64 KB)")
        return v


class SwitchDatabaseBody(BaseModel):
    database_name: str = Field(..., min_length=1, max_length=255)


class CtasBody(BaseModel):
    sql: str = Field(..., min_length=1)
    database: str = Field(..., min_length=1, max_length=255)
    schema: str = Field(default="dbo", min_length=1, max_length=128)
    table_name: str = Field(..., min_length=1, max_length=255)

    @field_validator("sql")
    @classmethod
    def sql_max_size(cls, v: str) -> str:
        if len(v.encode("utf-8")) > 65_536:
            raise ValueError("Query exceeds maximum allowed size (64 KB)")
        return v

    @field_validator("table_name")
    @classmethod
    def safe_name(cls, v: str) -> str:
        import re
        if not re.match(r'^[\w\s]+$', v):
            raise ValueError("Table name may only contain letters, digits, underscores and spaces")
        return v
