"""Pydantic models — Setup."""

from pydantic import BaseModel, Field


class SetupConnectionBody(BaseModel):
    endpoint: str = Field(..., min_length=1, max_length=512)
    database: str = Field(..., min_length=1, max_length=255)
