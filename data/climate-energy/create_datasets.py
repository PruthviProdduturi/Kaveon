"""Create Climate x Energy datasets in Kaveon metadata database."""
import psycopg2

conn = psycopg2.connect(
    host="kaveon-db.postgres.database.azure.com",
    dbname="kaveon",
    user="kaveon_admin",
    password="Kav30n!Db2026#S3cure",
    sslmode="require",
)
conn.autocommit = True
cur = conn.cursor()


def create_dataset(name, desc, table, schema, columns, metrics):
    cur.execute(
        """INSERT INTO datasets (dataset_name, description, fact_table, schema_name, database_name, date_column, created_by)
        VALUES (%s, %s, %s, %s, 'kaveon', 'year', 'pruthvi.prodduturi@gmail.com') RETURNING id""",
        (name, desc, table, schema),
    )
    ds_id = cur.fetchone()[0]
    for col in columns:
        cur.execute(
            """INSERT INTO dataset_columns (dataset_id, table_name, column_name, data_type, is_dimension, is_metric, semantic_type)
            VALUES (%s, %s, %s, %s, %s, %s, %s)""",
            (ds_id, table, col["name"], col["type"], col.get("dim", False), col.get("met", False), col.get("sem", "")),
        )
    for m in metrics:
        cur.execute(
            """INSERT INTO dataset_metrics (dataset_id, metric_name, expression, metric_type)
            VALUES (%s, %s, %s, %s)""",
            (ds_id, m["name"], m["expr"], m["type"]),
        )
    return ds_id


# ── 1. Global Energy ─────────────────────────────────────────────────────────
eid = create_dataset(
    "Global Energy Consumption",
    "220 countries, annual energy 2020-2025. Consumption, generation, fossil/renewable mix, carbon intensity, GHG. Source: Our World in Data.",
    "energy_annual", "climate_energy",
    columns=[
        {"name": "country", "type": "varchar", "dim": True, "sem": "dimension"},
        {"name": "year", "type": "smallint", "sem": "time"},
        {"name": "iso_code", "type": "char", "dim": True, "sem": "dimension"},
        {"name": "population", "type": "bigint", "met": True, "sem": "metric"},
        {"name": "gdp", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "primary_energy_consumption", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "energy_per_capita", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "electricity_generation", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "electricity_demand", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "fossil_fuel_consumption", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "fossil_share_energy", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "renewables_consumption", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "renewables_share_energy", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "renewables_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "solar_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "wind_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "hydro_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "nuclear_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "coal_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "gas_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "carbon_intensity_elec", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "greenhouse_gas_emissions", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "low_carbon_share_energy", "type": "numeric", "met": True, "sem": "metric"},
    ],
    metrics=[
        {"name": "Total Energy (TWh)", "expr": "SUM(primary_energy_consumption)", "type": "sum"},
        {"name": "Avg Energy Per Capita", "expr": "AVG(energy_per_capita)", "type": "avg"},
        {"name": "Total Electricity (TWh)", "expr": "SUM(electricity_generation)", "type": "sum"},
        {"name": "Avg Renewables %", "expr": "AVG(renewables_share_energy)", "type": "avg"},
        {"name": "Avg Carbon Intensity", "expr": "AVG(carbon_intensity_elec)", "type": "avg"},
        {"name": "Total GHG (Mt CO2e)", "expr": "SUM(greenhouse_gas_emissions)", "type": "sum"},
        {"name": "Total Solar (TWh)", "expr": "SUM(solar_electricity)", "type": "sum"},
        {"name": "Total Wind (TWh)", "expr": "SUM(wind_electricity)", "type": "sum"},
    ],
)
print(f"Global Energy: ID {eid}")

# ── 2. Temperature ────────────────────────────────────────────────────────────
tid = create_dataset(
    "Global Temperature Anomaly",
    "288 countries, monthly temperature change vs 1951-1980 baseline, 2020-2025. Source: FAO FAOSTAT.",
    "temperature_monthly", "climate_energy",
    columns=[
        {"name": "country", "type": "varchar", "dim": True, "sem": "dimension"},
        {"name": "country_code", "type": "varchar", "dim": True, "sem": "dimension"},
        {"name": "year", "type": "smallint", "sem": "time"},
        {"name": "month", "type": "smallint", "sem": "time"},
        {"name": "temp_change_c", "type": "numeric", "met": True, "sem": "metric"},
    ],
    metrics=[
        {"name": "Avg Temp Anomaly (C)", "expr": "AVG(temp_change_c)", "type": "avg"},
        {"name": "Max Temp Anomaly (C)", "expr": "MAX(temp_change_c)", "type": "max"},
        {"name": "Min Temp Anomaly (C)", "expr": "MIN(temp_change_c)", "type": "min"},
    ],
)
print(f"Temperature: ID {tid}")

# ── 3. Climate x Energy (view) ───────────────────────────────────────────────
cid = create_dataset(
    "Climate x Energy",
    "Cross-domain: temperature anomaly joined with energy consumption by country+year. 188 countries. How does warming correlate with energy mix and carbon intensity?",
    "climate_x_energy", "climate_energy",
    columns=[
        {"name": "country", "type": "varchar", "dim": True, "sem": "dimension"},
        {"name": "iso_code", "type": "char", "dim": True, "sem": "dimension"},
        {"name": "year", "type": "smallint", "sem": "time"},
        {"name": "population", "type": "bigint", "met": True, "sem": "metric"},
        {"name": "avg_tc", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "max_tc", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "primary_energy_consumption", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "energy_per_capita", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "electricity_generation", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "fossil_share_energy", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "renewables_share_energy", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "solar_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "wind_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "nuclear_electricity", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "carbon_intensity_elec", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "greenhouse_gas_emissions", "type": "numeric", "met": True, "sem": "metric"},
        {"name": "low_carbon_share_energy", "type": "numeric", "met": True, "sem": "metric"},
    ],
    metrics=[
        {"name": "Avg Temp Anomaly (C)", "expr": "AVG(avg_tc)", "type": "avg"},
        {"name": "Total Energy (TWh)", "expr": "SUM(primary_energy_consumption)", "type": "sum"},
        {"name": "Avg Renewables %", "expr": "AVG(renewables_share_energy)", "type": "avg"},
        {"name": "Avg Carbon Intensity", "expr": "AVG(carbon_intensity_elec)", "type": "avg"},
        {"name": "Total GHG (Mt CO2e)", "expr": "SUM(greenhouse_gas_emissions)", "type": "sum"},
    ],
)
print(f"Climate x Energy: ID {cid}")

conn.close()
print("\nAll 3 datasets created!")
