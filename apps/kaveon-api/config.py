"""Kaveon API — centralised configuration via Pydantic BaseSettings."""

import os
from pydantic_settings import BaseSettings, SettingsConfigDict

_ENV_FILE = os.path.join(os.path.dirname(__file__), "../../.env")


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=_ENV_FILE, extra="ignore")

    # ── Server ────────────────────────────────────────────────────────────────
    API_PORT: int = 8080
    NODE_ENV: str = "development"
    WEB_URL: str = "http://localhost:3000"

    # ── Azure AD ──────────────────────────────────────────────────────────────
    AZURE_TENANT_ID: str = ""
    AZURE_CLIENT_ID: str = ""

    # ── Proxy-injected identity (NextAuth flow, à la Forge) ────────────────────
    # The kaveon-web Next.js proxy authenticates the NextAuth session server-side
    # and forwards requests with X-User-Email / X-User-Name / X-User-Role headers,
    # stamped with X-Proxy-Secret. kaveon-api trusts those headers ONLY when the
    # secret matches this value — so a browser hitting the API directly cannot
    # spoof an identity. Leave blank to disable the proxy trust path.
    KAVEON_PROXY_SECRET: str = ""

    # Local dev bypass — set to a real email to skip auth entirely (no proxy).
    # Never set in production.
    KAVEON_DEV_USER_EMAIL: str = ""
    KAVEON_DEV_USER_NAME: str = "Dev User"
    KAVEON_DEV_USER_ROLE: str = "Admin"

    # ── Metadata database ─────────────────────────────────────────────────────
    # Supported types: fabric_sql | azure_sql | postgresql | mysql
    METADATA_DB_TYPE: str = ""
    METADATA_ENDPOINT: str = ""   # Fabric SQL / Azure SQL FQDN
    METADATA_DATABASE: str = ""
    METADATA_HOST: str = ""       # PostgreSQL / MySQL host
    METADATA_PORT: int = 0        # 0 = use driver default
    # Standard username/password auth for PostgreSQL / MySQL metadata DBs
    # (e.g. Neon, Supabase, PlanetScale). When USER+PASSWORD are set they are
    # used; otherwise the connection falls back to Azure AD Managed Identity.
    METADATA_USER: str = ""
    METADATA_PASSWORD: str = ""
    METADATA_SSLMODE: str = "require"   # PostgreSQL sslmode; Neon needs "require"

    # ── Data warehouse (optional fallback) ────────────────────────────────────
    DATAWAREHOUSE_ENDPOINT: str = ""
    DATAWAREHOUSE_DATABASE: str = ""

    # ── AI ────────────────────────────────────────────────────────────────────
    # Used to encrypt AI API keys stored in the database.
    # Defaults to a value derived from Azure tenant + client IDs.
    AI_ENCRYPTION_SECRET: str = ""

    # ── TLS ───────────────────────────────────────────────────────────────────
    # Set to True only when connecting to Fabric SQL / Azure SQL via private
    # endpoint where the certificate cannot be verified by the ODBC driver.
    # Must be False in production with a trusted CA-signed certificate.
    SQL_TRUST_SERVER_CERT: bool = True

    # ── Connection pool ───────────────────────────────────────────────────────
    MAX_POOL_SIZE_METADATA: int = 10
    MAX_POOL_SIZE_DATAWAREHOUSE: int = 5
    CONNECTION_TIMEOUT_METADATA: int = 30
    CONNECTION_TIMEOUT_DATAWAREHOUSE: int = 60


settings = Settings()
