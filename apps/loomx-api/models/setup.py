"""Pydantic models — Setup."""

from typing import Literal, Optional
from pydantic import BaseModel, Field, model_validator


DbType = Literal["fabric_sql", "azure_sql", "postgresql", "mysql"]


class SetupConnectionBody(BaseModel):
    db_type: DbType = "fabric_sql"

    # Fabric SQL — full ODBC connection string (server + database parsed server-side)
    connection_string: Optional[str] = Field(default=None, max_length=2048)

    # Azure SQL — server FQDN
    endpoint: Optional[str] = Field(default=None, max_length=512)

    # PostgreSQL / MySQL — host + port
    host: Optional[str] = Field(default=None, max_length=255)
    port: Optional[int] = Field(default=None, ge=1, le=65535)

    # Required for azure_sql, postgresql, mysql; derived from connection_string for fabric_sql
    database: Optional[str] = Field(default=None, min_length=1, max_length=255)

    @model_validator(mode="after")
    def _check_required(self) -> "SetupConnectionBody":
        if self.db_type == "fabric_sql":
            # Accept either an ODBC connection string OR explicit endpoint+database
            # (the latter is used by the schema_missing re-init flow)
            has_conn_str = bool(self.connection_string and self.connection_string.strip())
            has_explicit = bool(self.endpoint and self.database)
            if not has_conn_str and not has_explicit:
                raise ValueError(
                    "connection_string is required for Fabric SQL "
                    "(or provide endpoint + database directly)"
                )
        elif self.db_type == "azure_sql":
            if not self.endpoint:
                raise ValueError("endpoint is required for Azure SQL")
            if not self.database:
                raise ValueError("database is required for Azure SQL")
        else:
            if not self.host:
                raise ValueError("host is required for PostgreSQL and MySQL")
            if not self.database:
                raise ValueError("database is required")
        return self
