"""
AI service — provider key management, schema context building, and AI API calls.

Key resolution order (highest priority wins):
  1. User's personal key  (user_ai_keys table)
  2. Admin global key     (ai_providers table)

Encryption: Fernet symmetric encryption using a key derived from AI_ENCRYPTION_SECRET.
HTTP client: httpx (already in requirements) — no extra SDK needed.
"""

import base64
import hashlib
import json
from typing import Optional

import httpx
from cryptography.fernet import Fernet, InvalidToken

import database.metadata as db
from config import settings


# ── Encryption helpers ────────────────────────────────────────────────────────

def _fernet() -> Fernet:
    """Derive a stable Fernet key from the app's encryption secret."""
    secret = getattr(settings, "AI_ENCRYPTION_SECRET", "") or (
        settings.AZURE_TENANT_ID + settings.AZURE_CLIENT_ID
    )
    raw = hashlib.sha256(secret.encode()).digest()   # 32 bytes
    key = base64.urlsafe_b64encode(raw)
    return Fernet(key)


def _encrypt(plaintext: str) -> str:
    return _fernet().encrypt(plaintext.encode()).decode()


def _decrypt(token: str) -> str:
    try:
        return _fernet().decrypt(token.encode()).decode()
    except (InvalidToken, Exception):
        return ""


def _mask(api_key: str) -> str:
    """Return only last 4 chars for display."""
    if len(api_key) <= 4:
        return "****"
    return "•" * (len(api_key) - 4) + api_key[-4:]


# ── DB table bootstrap ────────────────────────────────────────────────────────

def ensure_tables() -> None:
    """Create AI tables if they don't exist (idempotent)."""
    try:
        db.execute("""
            IF NOT EXISTS (
                SELECT 1 FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'ai_providers'
            )
            CREATE TABLE ai_providers (
                id          INT IDENTITY(1,1) PRIMARY KEY,
                provider    NVARCHAR(50)   NOT NULL,
                label       NVARCHAR(100)  NOT NULL,
                api_key_enc NVARCHAR(2000) NOT NULL,
                model       NVARCHAR(100)  NOT NULL,
                is_active   BIT            DEFAULT 1,
                created_by  NVARCHAR(200),
                created_at  DATETIME2      DEFAULT GETUTCDATE(),
                updated_at  DATETIME2      DEFAULT GETUTCDATE()
            )
        """)
        db.execute("""
            IF NOT EXISTS (
                SELECT 1 FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'user_ai_keys'
            )
            CREATE TABLE user_ai_keys (
                user_email  NVARCHAR(200)  NOT NULL,
                provider    NVARCHAR(50)   NOT NULL,
                api_key_enc NVARCHAR(2000) NOT NULL,
                model       NVARCHAR(100),
                created_at  DATETIME2      DEFAULT GETUTCDATE(),
                updated_at  DATETIME2      DEFAULT GETUTCDATE(),
                PRIMARY KEY (user_email, provider)
            )
        """)
    except Exception as e:
        from fastapi import HTTPException
        if isinstance(e, HTTPException) and e.status_code == 503:
            return  # setup mode — metadata DB not configured yet, skip silently
        print(f"[AI] Warning: could not ensure AI tables — {e}")


# ── Provider CRUD (Admin) ──────────────────────────────────────────────────────

def list_providers() -> list:
    rows = db.query("SELECT id, provider, label, model, is_active, created_by, created_at FROM ai_providers ORDER BY id")["rows"]
    return rows


def get_provider_detail(provider_id: int) -> Optional[dict]:
    return db.query_one("SELECT * FROM ai_providers WHERE id = @param0", [provider_id])


def upsert_provider(provider: str, label: str, api_key: str, model: str, is_active: bool, created_by: str) -> dict:
    enc = _encrypt(api_key)
    # Update existing if same provider label exists, else insert
    existing = db.query_one("SELECT id FROM ai_providers WHERE provider = @param0 AND label = @param1", [provider, label])
    if existing:
        db.execute(
            "UPDATE ai_providers SET api_key_enc=@param0, model=@param1, is_active=@param2, updated_at=GETUTCDATE() WHERE id=@param3",
            [enc, model, 1 if is_active else 0, existing["id"]],
        )
        pid = existing["id"]
    else:
        row = db.query_one(
            "INSERT INTO ai_providers (provider, label, api_key_enc, model, is_active, created_by) "
            "OUTPUT INSERTED.id VALUES (@param0, @param1, @param2, @param3, @param4, @param5)",
            [provider, label, enc, model, 1 if is_active else 0, created_by],
        )
        pid = row["id"] if row else None
    return {"id": pid, "provider": provider, "label": label, "model": model, "is_active": is_active}


def delete_provider(provider_id: int) -> None:
    db.execute("DELETE FROM ai_providers WHERE id = @param0", [provider_id])


# ── User personal key CRUD ────────────────────────────────────────────────────

def get_user_keys(user_email: str) -> list:
    rows = db.query(
        "SELECT provider, model, api_key_enc, created_at FROM user_ai_keys WHERE user_email = @param0",
        [user_email],
    )["rows"]
    return [
        {"provider": r["provider"], "model": r["model"], "api_key_hint": _mask(_decrypt(r["api_key_enc"])), "created_at": r["created_at"]}
        for r in rows
    ]


def upsert_user_key(user_email: str, provider: str, api_key: str, model: Optional[str]) -> None:
    enc = _encrypt(api_key)
    existing = db.query_one(
        "SELECT 1 FROM user_ai_keys WHERE user_email = @param0 AND provider = @param1",
        [user_email, provider],
    )
    if existing:
        db.execute(
            "UPDATE user_ai_keys SET api_key_enc=@param0, model=@param1, updated_at=GETUTCDATE() WHERE user_email=@param2 AND provider=@param3",
            [enc, model, user_email, provider],
        )
    else:
        db.execute(
            "INSERT INTO user_ai_keys (user_email, provider, api_key_enc, model) VALUES (@param0, @param1, @param2, @param3)",
            [user_email, provider, enc, model],
        )


def delete_user_key(user_email: str, provider: str) -> None:
    db.execute(
        "DELETE FROM user_ai_keys WHERE user_email = @param0 AND provider = @param1",
        [user_email, provider],
    )


# ── Key resolution ────────────────────────────────────────────────────────────

def resolve_key(user_email: str) -> Optional[tuple[str, str, str]]:
    """
    Returns (api_key, provider, model) or None.
    Priority: user personal key > admin global key.
    Returns the first active key found.
    """
    # 1. User personal key (try anthropic first, then openai)
    for prov in ("anthropic", "openai"):
        row = db.query_one(
            "SELECT api_key_enc, model FROM user_ai_keys WHERE user_email = @param0 AND provider = @param1",
            [user_email, prov],
        )
        if row:
            key = _decrypt(row["api_key_enc"])
            if key:
                return key, prov, row.get("model") or _default_model(prov)

    # 2. Admin global key
    row = db.query_one(
        "SELECT provider, api_key_enc, model FROM ai_providers WHERE is_active = 1 ORDER BY id",
    )
    if row:
        key = _decrypt(row["api_key_enc"])
        if key:
            return key, row["provider"], row["model"]

    return None


def _default_model(provider: str) -> str:
    if provider == "anthropic":
        return "claude-sonnet-4-6"
    if provider == "github":
        return "gpt-4o"
    return "gpt-4o"


# ── Schema context builder ─────────────────────────────────────────────────────

def build_source_context(data_source_id: int) -> str:
    """Fetch data source info and format for the system prompt."""
    try:
        src = db.query_one(
            "SELECT name, type, database_name, region, description FROM data_sources WHERE id = @param0",
            [data_source_id],
        )
        if not src:
            return ""
        lines = [
            f"Data Source: {src['name']}",
            f"Type: {src['type']}",
            f"Database: {src['database_name']}",
            f"Region: {src.get('region', '')}",
        ]
        if src.get("description"):
            lines.append(f"Description: {src['description']}")
        return "\n".join(l for l in lines if l)
    except Exception:
        return ""


def build_dataset_context(dataset_id: int) -> str:
    """Fetch dataset + column info and format as a schema description for the system prompt."""
    try:
        ds = db.query_one(
            "SELECT name, dataset_name, description, fact_table, schema_name, database_name FROM datasets WHERE id = @param0",
            [dataset_id],
        )
        if not ds:
            return ""
        name = ds.get("dataset_name") or ds.get("name") or f"dataset_{dataset_id}"
        desc = ds.get("description") or ""
        fact = ds.get("fact_table") or ""
        schema = ds.get("schema_name") or "dbo"
        database = ds.get("database_name") or ""

        cols = db.query(
            "SELECT column_name, data_type, description FROM dataset_columns WHERE dataset_id = @param0 ORDER BY id",
            [dataset_id],
        )["rows"]

        metrics = db.query(
            "SELECT metric_name, sql_expression, description FROM dataset_metrics WHERE dataset_id = @param0",
            [dataset_id],
        )["rows"]

        lines = [
            f"Dataset: {name}",
            f"Database: {database}" if database else "",
            f"Fact table: {schema}.{fact}" if fact else "",
            f"Description: {desc}" if desc else "",
            "Columns:" if cols else "",
        ]
        for c in cols:
            lines.append(f"  - {c['column_name']} ({c['data_type']})" + (f" — {c['description']}" if c.get("description") else ""))
        if metrics:
            lines.append("Metrics:")
            for m in metrics:
                lines.append(f"  - {m['metric_name']}: {m['sql_expression']}" + (f" — {m['description']}" if m.get("description") else ""))

        return "\n".join(l for l in lines if l)
    except Exception:
        return ""


# ── AI API callers ────────────────────────────────────────────────────────────

async def _call_anthropic(api_key: str, model: str, system: str, messages: list) -> str:
    async with httpx.AsyncClient(timeout=90.0) as client:
        resp = await client.post(
            "https://api.anthropic.com/v1/messages",
            headers={
                "x-api-key": api_key,
                "anthropic-version": "2023-06-01",
                "content-type": "application/json",
            },
            json={
                "model": model,
                "max_tokens": 4096,
                "system": system,
                "messages": [{"role": m["role"], "content": m["content"]} for m in messages],
            },
        )
        resp.raise_for_status()
        data = resp.json()
        return data["content"][0]["text"]


async def _call_openai_compat(api_key: str, model: str, system: str, messages: list, base_url: str = "https://api.openai.com/v1") -> str:
    """Calls any OpenAI-compatible endpoint (OpenAI, GitHub Models, Azure OpenAI, etc.)."""
    all_messages = [{"role": "system", "content": system}] + [
        {"role": m["role"], "content": m["content"]} for m in messages
    ]
    async with httpx.AsyncClient(timeout=90.0) as client:
        resp = await client.post(
            f"{base_url.rstrip('/')}/chat/completions",
            headers={
                "Authorization": f"Bearer {api_key}",
                "content-type": "application/json",
            },
            json={"model": model, "messages": all_messages, "max_tokens": 4096},
        )
        resp.raise_for_status()
        data = resp.json()
        return data["choices"][0]["message"]["content"]


# ── Main chat function ────────────────────────────────────────────────────────

_SYSTEM_BASE = """You are an AI data assistant built into Lens, a modern analytics platform.
You help users write SQL queries, explore datasets, explain query results, suggest chart types, and design dashboards.

Database dialect: Microsoft Fabric SQL / T-SQL. Use T-SQL syntax in all generated queries.

When generating SQL:
- Wrap code in ```sql ... ``` blocks
- Add a brief explanation after the code block
- Use efficient queries — avoid SELECT * on large tables
- Use TOP N when the user hasn't specified a limit

When suggesting a chart, also output a JSON block like:
```chart
{"chart_type": "bar", "title": "...", "x_axis": "column", "y_axis": "metric"}
```
Supported chart types: bar, line, pie, scatter, area, heatmap, funnel, treemap, world_map.

When suggesting a dashboard layout, output:
```dashboard
{"title": "...", "description": "...", "charts": [{"chart_type": "...", "title": "...", "x_axis": "...", "y_axis": "..."}]}
```

Be concise and actionable. Ask clarifying questions when the request is ambiguous."""


async def chat(messages: list, context: Optional[dict], user_email: str) -> dict:
    """
    Resolve the best available API key for this user, build context, call the AI.
    Returns { reply, provider, model }.
    """
    resolved = resolve_key(user_email)
    if not resolved:
        raise ValueError("No AI provider configured. Ask an Admin to add an API key in Settings → AI Providers, or add your own personal key.")

    api_key, provider, model = resolved

    # Build system prompt with all available context
    system = _SYSTEM_BASE
    ctx = context or {}

    context_sections: list[str] = []

    # 1. Data source — tells the AI which DB engine, catalog name, and region
    if ctx.get("data_source_id"):
        src_ctx = build_source_context(int(ctx["data_source_id"]))
        if src_ctx:
            context_sections.append(f"### Data Source\n{src_ctx}")

    # 2. Dataset — columns, metrics, fact table
    if ctx.get("dataset_id"):
        ds_ctx = build_dataset_context(int(ctx["dataset_id"]))
        if ds_ctx:
            context_sections.append(f"### Dataset Schema\n{ds_ctx}")

    # 3. SQL currently in the editor — explain/optimise queries
    if ctx.get("sql") and ctx["sql"].strip():
        context_sections.append(f"### Current SQL\n```sql\n{ctx['sql'].strip()}\n```")

    if context_sections:
        system += "\n\n## Context\n" + "\n\n".join(context_sections)

    msgs = [{"role": m["role"], "content": m["content"]} for m in messages]

    if provider == "anthropic":
        reply = await _call_anthropic(api_key, model, system, msgs)
    elif provider == "openai":
        reply = await _call_openai_compat(api_key, model, system, msgs)
    elif provider == "github":
        # GitHub Models — OpenAI-compatible endpoint, auth via GitHub PAT
        reply = await _call_openai_compat(api_key, model, system, msgs, base_url="https://models.inference.ai.azure.com")
    else:
        raise ValueError(f"Unknown provider: {provider}")

    return {"reply": reply, "provider": provider, "model": model}
