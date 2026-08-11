-- Climate × Energy schema
-- Kaveon cross-domain showcase: how climate patterns affect global energy consumption
-- Schema: climate_energy (separate from public where COVID/Taxi data lives)

CREATE SCHEMA IF NOT EXISTS climate_energy;

-- Monthly temperature anomaly by country (FAO/GISTEMP baseline: 1951-1980)
-- Source: FAO FAOSTAT Environment - Temperature Change
CREATE TABLE climate_energy.temperature_monthly (
    id              BIGSERIAL PRIMARY KEY,
    country         VARCHAR(100) NOT NULL,
    country_code    VARCHAR(10),
    year            SMALLINT NOT NULL,
    month           SMALLINT NOT NULL CHECK (month BETWEEN 1 AND 12),
    temp_change_c   DECIMAL(6,3),           -- temperature anomaly in °C vs 1951-1980 baseline
    UNIQUE (country, year, month)
);

CREATE INDEX idx_temp_country ON climate_energy.temperature_monthly(country);
CREATE INDEX idx_temp_year_month ON climate_energy.temperature_monthly(year, month);

-- Annual energy data by country
-- Source: Our World in Data (OWID) Energy Dataset
CREATE TABLE climate_energy.energy_annual (
    id                          BIGSERIAL PRIMARY KEY,
    country                     VARCHAR(100) NOT NULL,
    year                        SMALLINT NOT NULL,
    iso_code                    CHAR(3),
    population                  BIGINT,
    gdp                         DECIMAL(18,2),

    -- Primary energy
    primary_energy_consumption  DECIMAL(14,4),      -- TWh
    energy_per_capita           DECIMAL(14,4),      -- kWh per person
    energy_per_gdp              DECIMAL(14,8),      -- kWh per $ GDP

    -- Electricity
    electricity_generation      DECIMAL(14,4),      -- TWh
    electricity_demand          DECIMAL(14,4),      -- TWh

    -- Fossil fuels
    fossil_fuel_consumption     DECIMAL(14,4),      -- TWh
    fossil_share_energy         DECIMAL(8,4),       -- %

    -- Renewables
    renewables_consumption      DECIMAL(14,4),      -- TWh
    renewables_share_energy     DECIMAL(8,4),       -- %
    renewables_electricity      DECIMAL(14,4),      -- TWh
    solar_electricity           DECIMAL(14,4),      -- TWh
    wind_electricity            DECIMAL(14,4),      -- TWh
    hydro_electricity           DECIMAL(14,4),      -- TWh
    nuclear_electricity         DECIMAL(14,4),      -- TWh

    -- Fossil electricity breakdown
    coal_electricity            DECIMAL(14,4),      -- TWh
    gas_electricity             DECIMAL(14,4),      -- TWh
    oil_electricity             DECIMAL(14,4),      -- TWh

    -- Carbon
    carbon_intensity_elec       DECIMAL(10,4),      -- gCO2/kWh
    greenhouse_gas_emissions    DECIMAL(14,4),      -- Mt CO2e
    low_carbon_share_energy     DECIMAL(8,4),       -- %
    low_carbon_electricity      DECIMAL(14,4),      -- TWh

    UNIQUE (country, year)
);

CREATE INDEX idx_energy_country ON climate_energy.energy_annual(country);
CREATE INDEX idx_energy_year ON climate_energy.energy_annual(year);
CREATE INDEX idx_energy_iso ON climate_energy.energy_annual(iso_code);

-- View: combined climate + energy by country and year
-- This is the semantic join layer — the showcase for cross-dataset analysis
CREATE OR REPLACE VIEW climate_energy.climate_x_energy AS
SELECT
    e.country,
    e.iso_code,
    e.year,
    e.population,
    e.gdp,

    -- Climate (annual avg of monthly anomalies)
    t.avg_temp_change_c,
    t.max_temp_change_c,
    t.min_temp_change_c,

    -- Energy
    e.primary_energy_consumption,
    e.energy_per_capita,
    e.electricity_generation,
    e.electricity_demand,

    -- Mix
    e.fossil_share_energy,
    e.renewables_share_energy,
    e.renewables_electricity,
    e.solar_electricity,
    e.wind_electricity,
    e.nuclear_electricity,

    -- Carbon
    e.carbon_intensity_elec,
    e.greenhouse_gas_emissions,
    e.low_carbon_share_energy
FROM climate_energy.energy_annual e
LEFT JOIN (
    SELECT
        country,
        year,
        ROUND(AVG(temp_change_c)::numeric, 3) AS avg_temp_change_c,
        MAX(temp_change_c) AS max_temp_change_c,
        MIN(temp_change_c) AS min_temp_change_c
    FROM climate_energy.temperature_monthly
    GROUP BY country, year
) t ON e.country = t.country AND e.year = t.year;
