"""Kaveon API — centralised configuration via Pydantic BaseSettings."""

import os
from pydantic_settings import BaseSettings, SettingsConfigDict

_ENV_FILE = os.path.join(os.path.dirname(__file__), "../.env")


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
    # The Studio Next.js proxy authenticates the NextAuth session server-side
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

    # ── Demo posture ──────────────────────────────────────────────────────────
    # True makes every mutating platform route read-only for any role below
    # Admin (403 `demo_read_only`) and confines submitted SQL to read
    # statements; see middleware/demo.py. The Engine's per-principal live-read
    # quota is configured on the coordinator (resource groups, `demo.enabled`).
    # Off by default: a self-hosted install is unaffected.
    KAVEON_DEMO_MODE: bool = False

    # ── Read warming ──────────────────────────────────────────────────────────
    # Fills the caches behind the hot reads before a reader asks for them, on
    # an interval, for the principals who are going to read them
    # (services/warm_reads.py). On by default: with it off, the first visit
    # after a restart and the first visit after any invalidation pay the full
    # cost of every read the page makes, which is defect D-27.
    #
    # KAVEON_WARM_PRINCIPALS states who to warm for before anybody has been
    # seen — "email:Role, email:Role"; a bare email is taken as Viewer. Leave
    # it empty and only principals this replica has already served are warmed,
    # so the very first visit after a restart is still cold.
    KAVEON_READ_WARMER_ENABLED: bool = True
    KAVEON_WARM_PRINCIPALS: str = ""
    # Seconds after startup before the first cycle. Not zero: warming must not
    # compete with the application's own startup, and readiness must never
    # depend on the Engine being reachable.
    KAVEON_WARM_START_DELAY_SECONDS: float = 5.0
    # Seconds between cycles. Clamped to half the tightest staleness bound it
    # is keeping an entry inside, so a longer value cannot silently let an
    # entry age out between cycles.
    KAVEON_WARM_INTERVAL_SECONDS: float = 60.0

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
