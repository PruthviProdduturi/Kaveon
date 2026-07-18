# ── Weft API — Python/FastAPI + ODBC Driver 18 ───────────────────────────────
#
# Replaces both the Node.js/Express API and the Python Flask proxy with a
# single service that handles all authentication, DB connectivity, and REST
# endpoints.
#
# ODBC Driver 18 for SQL Server is installed from Microsoft's official APT
# repository for Debian 12 (bookworm) — identical to the former proxy image.
FROM python:3.11-slim

# Install ODBC Driver 18 for SQL Server
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl gnupg2 \
    && curl -fsSL https://packages.microsoft.com/keys/microsoft.asc \
       | gpg --dearmor -o /usr/share/keyrings/microsoft-prod.gpg \
    && curl -fsSL https://packages.microsoft.com/config/debian/12/prod.list \
       | tee /etc/apt/sources.list.d/mssql-release.list \
    && apt-get update \
    && ACCEPT_EULA=Y apt-get install -y --no-install-recommends \
       msodbcsql18 \
       unixodbc-dev \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Install Python dependencies
COPY apps/weft-api/requirements.txt ./
RUN pip install --no-cache-dir -r requirements.txt

# Copy application source
COPY apps/weft-api/ ./

ENV ENV=production
ENV PORT=8080

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:8080/api/health')" || exit 1

# Uvicorn workers managed by Gunicorn for production multi-threading
CMD ["gunicorn", "--bind", "0.0.0.0:8080", "--workers", "4", "--threads", "8", \
     "--timeout", "120", "--worker-class", "uvicorn.workers.UvicornWorker", "main:app"]
