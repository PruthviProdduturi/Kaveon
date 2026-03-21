"""Pydantic models — Users / Role management."""

from typing import Literal
from pydantic import BaseModel, Field


class RoleAssignBody(BaseModel):
    role: Literal["Viewer", "Analyst", "Editor", "Admin"] = Field(
        ..., description="Role to assign to the user"
    )
