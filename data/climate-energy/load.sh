#!/bin/bash
# Load Climate × Energy data into Azure Postgres
# Run from: D:\Repos\PruthviProdduturi\Kaveon\data\climate-energy
#
# Prerequisites:
#   - psql installed
#   - Port 5432 accessible (run from HOME, not corporate)
#   - Azure PG credentials (managed identity or password)
#
# Usage:
#   export PGHOST=kaveon-db.postgres.database.azure.com
#   export PGDATABASE=kaveon
#   export PGUSER=kaveonadmin
#   export PGPASSWORD=<your-password>
#   export PGSSLMODE=require
#   bash load.sh

set -e

echo "=== Creating schema ==="
psql -f schema.sql

echo "=== Loading temperature data (19K rows) ==="
psql -c "\copy climate_energy.temperature_monthly(country, country_code, year, month, temp_change_c) FROM 'climate_temperature.csv' WITH CSV HEADER"

echo "=== Loading energy data (1.2K rows) ==="
psql -c "\copy climate_energy.energy_annual(country, year, iso_code, population, gdp, primary_energy_consumption, energy_per_capita, energy_per_gdp, electricity_generation, electricity_demand, fossil_fuel_consumption, fossil_share_energy, renewables_consumption, renewables_share_energy, renewables_electricity, solar_electricity, wind_electricity, hydro_electricity, nuclear_electricity, coal_electricity, gas_electricity, oil_electricity, carbon_intensity_elec, greenhouse_gas_emissions, low_carbon_share_energy, low_carbon_electricity) FROM 'energy_global.csv' WITH CSV HEADER NULL ''"

echo "=== Verifying ==="
psql -c "SELECT 'temperature_monthly' AS tbl, COUNT(*) FROM climate_energy.temperature_monthly UNION ALL SELECT 'energy_annual', COUNT(*) FROM climate_energy.energy_annual UNION ALL SELECT 'climate_x_energy (view)', COUNT(*) FROM climate_energy.climate_x_energy;"

echo "=== Sample from cross-domain view ==="
psql -c "SELECT country, year, avg_temp_change_c, primary_energy_consumption, renewables_share_energy, carbon_intensity_elec FROM climate_energy.climate_x_energy WHERE year = 2023 AND primary_energy_consumption IS NOT NULL ORDER BY primary_energy_consumption DESC LIMIT 10;"

echo "=== Done! ==="
