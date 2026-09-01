"""
Custom exception classes and FastAPI exception handlers.
Port of errorHandler.ts.
"""

import os
import traceback
from typing import Any
from fastapi import Request
from fastapi.responses import JSONResponse


# ── Exception classes ─────────────────────────────────────────────────────────

class AppError(Exception):
    def __init__(self, status_code: int, code: str, message: str, details: Any = None):
        super().__init__(message)
        self.status_code = status_code
        self.code = code
        self.message = message
        self.details = details


class ValidationError(AppError):
    def __init__(self, message: str, details: Any = None):
        super().__init__(400, "validation_error", message, details)


class NotFoundError(AppError):
    def __init__(self, resource: str):
        super().__init__(404, "not_found", f"{resource} not found")


class DatabaseError(AppError):
    def __init__(self, message: str, details: Any = None):
        super().__init__(503, "database_error", message, details)


class UnauthorizedError(AppError):
    def __init__(self, message: str = "Unauthorized"):
        super().__init__(401, "unauthorized", message)


# ── FastAPI exception handlers ────────────────────────────────────────────────

async def app_error_handler(request: Request, exc: AppError) -> JSONResponse:
    print(f"[API Error] {exc.code}: {exc.message} — {request.method} {request.url.path}")
    return JSONResponse(
        status_code=exc.status_code,
        content={
            "error": {
                "code": exc.code,
                "message": exc.message,
                "details": exc.details,
            }
        },
    )


async def generic_error_handler(request: Request, exc: Exception) -> JSONResponse:
    # Always log full details server-side
    print(f"[API Error] Unhandled: {type(exc).__name__} — {request.method} {request.url.path}")
    traceback.print_exc()

    # Never expose internal error details to clients
    return JSONResponse(
        status_code=500,
        content={
            "error": {
                "code": "internal_error",
                "message": "An unexpected error occurred. Please try again or contact support.",
            }
        },
    )
