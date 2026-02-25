# ── Python proxy — Flask + pyodbc + ODBC Driver 18 ────────────────────────────
#
# ODBC Driver 18 for SQL Server is required for Microsoft Fabric SQL connectivity.
# It is installed from Microsoft's official APT repository for Debian 12 (bookworm).
FROM python:3.11-slim AS runner

# Install ODBC Driver 18 for SQL Server
# ACCEPT_EULA=Y is required by Microsoft's installer
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
COPY apps/loomx-python-proxy/requirements.txt ./
RUN pip install --no-cache-dir -r requirements.txt

# Copy proxy source
COPY apps/loomx-python-proxy/proxy.py ./

ENV FLASK_ENV=production
ENV PORT=5001

EXPOSE 5001

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:5001/health')" || exit 1

# Use gunicorn in production for proper multi-threading
# (Flask's dev server is single-threaded and not suitable for production)
RUN pip install --no-cache-dir gunicorn

CMD ["gunicorn", "--bind", "0.0.0.0:5001", "--workers", "4", "--threads", "8", "--timeout", "120", "proxy:app"]
