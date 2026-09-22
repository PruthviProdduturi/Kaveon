"""Pydantic models — Engine catalog management (/api/v1/engine/catalog).

Names are what a Trino user types: plain SQL identifiers for catalogs,
schemas and tables, Trino type names for columns. The Engine's own ids,
revisions and Arrow types are derived here, never typed by hand.
"""

from typing import Any, Literal, Optional, Union

from pydantic import BaseModel, Field, field_validator, model_validator

IDENTIFIER = r"^[A-Za-z_][A-Za-z0-9_]*$"
Format = Literal["Parquet", "Delta", "Iceberg"]
Access = Literal["Shortcut", "Optimized"]

# Trino type name → the Engine's Arrow data type, as its catalog serializes it.
# `timestamp(p)` and `decimal(p, s)` are handled in arrow_type; names not here
# may be given as the Arrow name itself (Int64, Utf8, Date32 …) or as the
# Arrow JSON value for a parameterized type ({"Timestamp": ["Microsecond", null]}).
TRINO_TYPES: dict[str, Any] = {
    "bigint": "Int64", "integer": "Int32", "int": "Int32", "smallint": "Int16", "tinyint": "Int8",
    "boolean": "Boolean", "bool": "Boolean",
    "double": "Float64", "real": "Float32", "float": "Float32",
    "varchar": "Utf8", "char": "Utf8", "string": "Utf8", "text": "Utf8",
    "varbinary": "Binary", "binary": "Binary",
    "date": "Date32",
}
ARROW_NAMES = {
    "Null", "Boolean", "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64",
    "Float16", "Float32", "Float64", "Utf8", "LargeUtf8", "Binary", "LargeBinary", "Date32", "Date64",
}
TIMESTAMP_UNITS = {0: "Second", 3: "Millisecond", 6: "Microsecond", 9: "Nanosecond"}


def arrow_type(spec: Union[str, dict]) -> Any:
    """Translate a column type as typed (Trino name or Arrow value) into the Engine's Arrow JSON."""
    if isinstance(spec, dict):
        if len(spec) != 1:
            raise ValueError("An Arrow type value names exactly one type")
        return spec
    text = spec.strip()
    if text in ARROW_NAMES:
        return text
    lowered = text.lower()
    base, _, rest = lowered.partition("(")
    base = base.strip()
    args = [part.strip() for part in rest.rstrip(")").split(",")] if rest else []
    if base in TRINO_TYPES and (not args or base in {"varchar", "char"}):
        return TRINO_TYPES[base]
    if base == "timestamp":
        precision = int(args[0]) if args and args[0].isdigit() else 6
        if precision not in TIMESTAMP_UNITS:
            raise ValueError("timestamp precision must be 0, 3, 6 or 9")
        return {"Timestamp": [TIMESTAMP_UNITS[precision], None]}
    if base == "decimal":
        if len(args) != 2 or not all(part.isdigit() for part in args):
            raise ValueError("decimal needs a precision and a scale, as decimal(18, 2)")
        precision, scale = int(args[0]), int(args[1])
        if not 1 <= precision <= 38 or not 0 <= scale <= precision:
            raise ValueError("decimal precision must be 1–38 and the scale no larger than the precision")
        return {"Decimal128": [precision, scale]}
    raise ValueError(f"Unsupported column type '{text}'")


def _plain(value: str, what: str, limit: int = 255) -> str:
    value = value.strip()
    if not value:
        raise ValueError(f"{what} cannot be empty")
    if len(value.encode("utf-8")) > limit:
        raise ValueError(f"{what} exceeds {limit} bytes")
    if any(ch.isspace() and ch != " " or ord(ch) < 32 or ord(ch) == 127 for ch in value):
        raise ValueError(f"{what} cannot contain control characters")
    return value


class ColumnSpec(BaseModel):
    name: str = Field(..., min_length=1, max_length=255)
    type: Union[str, dict] = Field(..., description="Trino type name, Arrow type name, or Arrow type JSON")
    nullable: bool = True

    @field_validator("name")
    @classmethod
    def plain_name(cls, value: str) -> str:
        return _plain(value, "Column name")

    @field_validator("type")
    @classmethod
    def known_type(cls, value: Union[str, dict]) -> Union[str, dict]:
        arrow_type(value)   # raises ValueError → 422 with the reason
        return value

    def engine(self) -> dict:
        return {"name": self.name, "data_type": arrow_type(self.type), "nullable": self.nullable}


def _location(value: str) -> str:
    value = _plain(value, "Location", 1024)
    if "://" in value:
        raise ValueError("Location is a path relative to the catalog's container or root, not a URI")
    if value.startswith("/") or value.startswith("\\"):
        raise ValueError("Location is relative to the catalog root; drop the leading slash")
    return value.rstrip("/")


def _format(value: str) -> str:
    text = value.strip().capitalize()
    if text not in {"Parquet", "Delta", "Iceberg"}:
        raise ValueError("format must be Parquet, Delta or Iceberg")
    return text


class TableCreate(BaseModel):
    schema_id: str = Field(..., min_length=1, max_length=255)
    name: str = Field(..., pattern=IDENTIFIER, max_length=128)
    location: str = Field(..., min_length=1, max_length=1024)
    format: str = Field(..., description="Parquet, Delta or Iceberg")
    access: Access = "Shortcut"
    # Empty = the Engine reads the columns from the table's own metadata
    # (Delta log, Iceberg metadata, the first Parquet footer).
    columns: list[ColumnSpec] = Field(default_factory=list, max_length=2000)
    id: Optional[str] = Field(default=None, min_length=1, max_length=255)
    verify: bool = True

    @field_validator("location")
    @classmethod
    def relative_location(cls, value: str) -> str:
        return _location(value)

    @field_validator("format")
    @classmethod
    def known_format(cls, value: str) -> str:
        return _format(value)

    @field_validator("id")
    @classmethod
    def plain_id(cls, value: Optional[str]) -> Optional[str]:
        return _plain(value, "Table id") if value is not None else None

    @model_validator(mode="after")
    def unique_columns(self):
        names = [column.name for column in self.columns]
        if len(set(names)) != len(names):
            raise ValueError("Column names must be unique")
        return self


class TableAnalyze(BaseModel):
    """How deeply to measure one table. Nothing set is the metadata read: the
    Parquet footers, the Delta log or the Iceberg manifests, and no data page.
    `sketches` reads the columns once to build the distinct-count and quantile
    sketches; `distinct` counts distinct values exactly instead of estimating
    them; `cube` builds the cells of the table's declared shape in the same
    scan. Each is a property of the Engine's ANALYZE statement, assembled from
    these flags alone."""
    sketches: bool = False
    distinct: bool = False
    cube: bool = False


class TableReplace(BaseModel):
    """A full replacement, as the Engine requires; the id, schema and revision come from the path and If-Match."""
    name: str = Field(..., pattern=IDENTIFIER, max_length=128)
    location: str = Field(..., min_length=1, max_length=1024)
    format: str
    access: Access = "Shortcut"
    columns: list[ColumnSpec] = Field(..., min_length=1, max_length=2000)
    lifecycle: Optional[Literal["Active", "Suspended", "Deleting", "Deleted"]] = None

    @field_validator("location")
    @classmethod
    def relative_location(cls, value: str) -> str:
        return _location(value)

    @field_validator("format")
    @classmethod
    def known_format(cls, value: str) -> str:
        return _format(value)

    @model_validator(mode="after")
    def unique_columns(self):
        names = [column.name for column in self.columns]
        if len(set(names)) != len(names):
            raise ValueError("Column names must be unique")
        return self


class SchemaCreate(BaseModel):
    name: str = Field(..., pattern=IDENTIFIER, max_length=128)
    id: Optional[str] = Field(default=None, min_length=1, max_length=255)

    @field_validator("id")
    @classmethod
    def plain_id(cls, value: Optional[str]) -> Optional[str]:
        return _plain(value, "Schema id") if value is not None else None


class LocalStorage(BaseModel):
    type: Literal["local"]
    base_path: str = Field(..., min_length=1, max_length=1024)


class AdlsStorage(BaseModel):
    type: Literal["adls_gen2"]
    account: str = Field(..., min_length=3, max_length=24, pattern=r"^[a-z0-9]+$")
    container: str = Field(..., min_length=3, max_length=63, pattern=r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$")
    root_path: str = Field(default="", max_length=1024)


class S3Storage(BaseModel):
    type: Literal["s3"]
    bucket: str = Field(..., min_length=3, max_length=63)
    region: str = Field(..., min_length=1, max_length=32)
    prefix: str = Field(default="", max_length=1024)


class CredentialSpec(BaseModel):
    kind: Literal["managed_identity", "workload_identity", "environment", "secret_store"]
    reference: str = Field(..., min_length=1, max_length=512)


class CatalogCreate(BaseModel):
    """A catalog is registered once, in the platform's source registry, and
    synchronized to the Engine from there — the one registration path, so a
    catalog never exists in the Engine without the record Studio lists."""
    name: str = Field(..., pattern=IDENTIFIER, max_length=128)
    storage: Union[LocalStorage, AdlsStorage, S3Storage] = Field(..., discriminator="type")
    credential: Optional[CredentialSpec] = None
    format: Literal["parquet", "delta", "iceberg"] = "parquet"
    description: Optional[str] = Field(default=None, max_length=1000)
