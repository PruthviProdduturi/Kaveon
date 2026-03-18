"""
LoomX API — centralised configuration.
All environment variables are loaded here via Pydantic BaseSettings.
"""

import os
from functools import lru_cache
from pydantic_settings import BaseSettings
from dotenv import load_dotenv

# Load root .env (two levels up from apps/loomx-api/)
_env_path = os.path.join(os.path.dirname(__file__), "../../.env")
load_dotenv(dotenv_path=_env_path)


class Settings(BaseSettings):
    # Server
    API_PORT: int = 8080
    ENV: str = "production"

    # CORS
    WEB_URL: str = "http://localhost:3000"

    # Azure AD — used for JWT signature verification
    AZURE_CLIENT_ID: str = ""
    AZURE_TENANT_ID: str = ""

    # Metadata database
    METADATA_DB_TYPE: str = "fabric_sql"   # fabric_sql | azure_sql | postgresql | mysql
    METADATA_ENDPOINT: str = ""             # Fabric SQL / Azure SQL server FQDN
    METADATA_DATABASE: str = ""
    METADATA_HOST: str = ""                 # PostgreSQL / MySQL host
    METADATA_PORT: int = 0                  # 0 = use driver default

    # Default datawarehouse (backward-compat, optional)
    DATAWAREHOUSE_ENDPOINT: str = ""
    DATAWAREHOUSE_DATABASE: str = ""

    # Connection pool tuning
    MAX_POOL_SIZE_METADATA: int = 20
    MAX_POOL_SIZE_DATAWAREHOUSE: int = 10
    CONNECTION_TIMEOUT_METADATA: int = 30
    CONNECTION_TIMEOUT_DATAWAREHOUSE: int = 60

    class Config:
        env_file = "../../.env"
        extra = "ignore"


@lru_cache()
def get_settings() -> Settings:
    return Settings()


settings = get_settings()
