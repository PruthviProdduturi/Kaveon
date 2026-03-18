"""Pydantic models — Setup."""

from typing import Literal, Optional
from pydantic import BaseModel, Field, model_validator


DbType = Literal["fabric_sql", "azure_sql", "postgresql", "mysql"]


class SetupConnectionBody(BaseModel):
    db_type: DbType = "fabric_sql"

    # Fabric SQL / Azure SQL — server FQDN (required for mssql types)
    endpoint: Optional[str] = Field(default=None, max_length=512)

    # PostgreSQL / MySQL — host + port (required for non-mssql types)
    host: Optional[str] = Field(default=None, max_length=255)
    port: Optional[int] = Field(default=None, ge=1, le=65535)

    # Auth — required for PostgreSQL / MySQL
    username: Optional[str] = Field(default=None, max_length=128)
    password: Optional[str] = Field(default=None, max_length=256)

    # All types
    database: str = Field(..., min_length=1, max_length=255)

    @model_validator(mode="after")
    def _check_required(self) -> "SetupConnectionBody":
        if self.db_type in ("fabric_sql", "azure_sql"):
            if not self.endpoint:
                raise ValueError("endpoint is required for Fabric SQL and Azure SQL")
        else:
            if not self.host:
                raise ValueError("host is required for PostgreSQL and MySQL")
            if not self.username:
                raise ValueError("username is required for PostgreSQL and MySQL")
        return self
