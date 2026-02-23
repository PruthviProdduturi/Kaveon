# LOOMX Python Proxy Service

**Live Operational Outcomes & Metrics eXperience**

This Python service uses **pyodbc + ODBC Driver 18** to connect to Microsoft Fabric SQL, providing a REST API that the Node.js backend can call.

## Why This Exists

**Problem:** Node.js Tedious library cannot authenticate with Fabric SQL using Azure AD tokens.

**Solution:** Use Python pyodbc with Azure AD authentication via a lightweight proxy service.

## Prerequisites

1. **Python 3.10+** installed
2. **ODBC Driver 18 for SQL Server** installed
3. **Azure credentials** configured (DefaultAzureCredential)

## Installation

```bash
cd D:\Repos\IDEASFabric\sources\dev\Tools\LOOMX\apps\loomx-python-proxy

# Create virtual environment
python -m venv venv

# Activate virtual environment
venv\Scripts\activate

# Install dependencies
pip install -r requirements.txt
```

## Configuration

**This service reads from the root `.env` file** at the repository root (`../../.env`).

No need to create a local `.env` file! All configuration is centralized in the root `.env` file.

See: `../../.env.example` for the template.

## Running the Service

### Quick Start (Recommended)

Use the provided batch script:

```bash
start_proxy.bat
```

This will automatically:
- Activate the virtual environment
- Load environment variables from root `.env` file
- Start the proxy on http://localhost:5001

### Manual Start

```bash
# Activate virtual environment
venv\Scripts\activate

# Make sure python-dotenv is installed
pip install python-dotenv

# Run the proxy (it will load root .env automatically)
python proxy.py
```

The service will start on **http://localhost:5001**

## API Endpoints

### Health Check
```
GET http://localhost:5001/health
```

### Get Tables
```
GET http://localhost:5001/api/v1/tables?database=YourDatabase
```

### Get Table Columns
```
GET http://localhost:5001/api/v1/tables/schema.tablename/columns?database=YourDatabase
```

### Execute SQL Query
```
POST http://localhost:5001/api/v1/execute
Content-Type: application/json

{
  "sql": "SELECT TOP 10 * FROM schema.table",
  "database": "YourDatabase"
}
```

## Integration with LOOMX API

Update your Node.js API (`apps/loomx-api`) to proxy requests to this service instead of using Tedious directly.

## Architecture

```
LOOMX Frontend → LOOMX API (Node.js :8080) → Python Proxy (:5001) → Fabric SQL
                                              (pyodbc + ODBC Driver 18)
```

## Testing

```bash
# Test health check
curl http://localhost:5001/health

# Test tables endpoint
curl http://localhost:5001/api/v1/tables
```
