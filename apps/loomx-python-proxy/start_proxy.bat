@echo off
REM LoomX Python Proxy Startup Script
REM Configuration is loaded from .env file in this directory

echo Starting Python Proxy...
echo.

cd /d "%~dp0"

REM Activate virtual environment if it exists
if exist venv\Scripts\activate.bat (
    echo Activating virtual environment...
    call venv\Scripts\activate.bat
    echo Virtual environment activated.
) else (
    echo WARNING: Virtual environment not found at venv\Scripts\activate.bat
    echo Using global Python installation.
    echo.
)

REM Start the proxy (reads config from .env)
echo Starting proxy...
python proxy.py
