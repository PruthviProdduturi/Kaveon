"""Create Climate × Energy dashboard with charts in Kaveon."""
import psycopg2
import json
import uuid

conn = psycopg2.connect(
    host="kaveon-db.postgres.database.azure.com",
    dbname="kaveon", user="kaveon_admin",
    password="Kav30n!Db2026#S3cure", sslmode="require",
)
conn.autocommit = True
cur = conn.cursor()

DS = "kaveon.climate_energy"


def chart(name, chart_type, query_config, viz_config=None):
    qc = json.dumps(query_config)
    vc = json.dumps(viz_config or {})
    cur.execute(
        """INSERT INTO charts (name, chart_type, query_config, viz_config,
           visibility, created_by)
        VALUES (%s, %s, %s, %s, 'internal', 'pruthvi.prodduturi@gmail.com')
        RETURNING id""",
        (name, chart_type, qc, vc),
    )
    cid = cur.fetchone()[0]
    print(f"  Chart {cid}: {name} ({chart_type})")
    return cid


print("=== Creating charts ===")

# KPI 1: Total Energy
c1 = chart("Total Energy Consumption", "big_number", {
    "dataset_id": 132,
    "datasource": f"{DS}.energy_annual",
    "query_mode": "aggregate",
    "metrics": [{"column": "primary_energy_consumption", "aggregate": "SUM", "label": "TWh"}],
    "filters": [{"column": "year", "op": "=", "value": "2023"}],
})

# KPI 2: Avg Temp Anomaly
c2 = chart("Avg Temperature Anomaly", "big_number", {
    "dataset_id": 134,
    "datasource": f"{DS}.climate_x_energy",
    "query_mode": "aggregate",
    "metrics": [{"column": "avg_tc", "aggregate": "AVG", "label": "°C"}],
    "filters": [{"column": "year", "op": "=", "value": "2023"}],
})

# KPI 3: Renewables Share
c3 = chart("Renewables Share", "big_number", {
    "dataset_id": 132,
    "datasource": f"{DS}.energy_annual",
    "query_mode": "aggregate",
    "metrics": [{"column": "renewables_share_energy", "aggregate": "AVG", "label": "%"}],
    "filters": [{"column": "year", "op": "=", "value": "2023"}],
})

# KPI 4: Carbon Intensity
c4 = chart("Carbon Intensity", "big_number", {
    "dataset_id": 132,
    "datasource": f"{DS}.energy_annual",
    "query_mode": "aggregate",
    "metrics": [{"column": "carbon_intensity_elec", "aggregate": "AVG", "label": "gCO₂/kWh"}],
    "filters": [{"column": "year", "op": "=", "value": "2023"}],
})

# World Map: Energy consumption
c5 = chart("Energy Consumption by Country", "world_map", {
    "dataset_id": 132,
    "datasource": f"{DS}.energy_annual",
    "query_mode": "aggregate",
    "template": "world_map",
    "metrics": [{"column": "primary_energy_consumption", "aggregate": "SUM", "label": "Energy (TWh)"}],
    "groupby": ["country"],
    "map_code_column": "iso_code",
    "filters": [{"column": "year", "op": "=", "value": "2023"}],
    "sort_by": {"column": "primary_energy_consumption", "direction": "desc"},
}, {
    "color": ["#eff6ff", "#93c5fd", "#3b82f6", "#1d4ed8", "#1e3a8a"],
    "chartTypeOptions": {"mapRegion": "world"},
})

# World Map: Temperature anomaly
c6 = chart("Temperature Anomaly by Country", "world_map", {
    "dataset_id": 134,
    "datasource": f"{DS}.climate_x_energy",
    "query_mode": "aggregate",
    "template": "world_map",
    "metrics": [{"column": "avg_tc", "aggregate": "AVG", "label": "Temp Anomaly (°C)"}],
    "groupby": ["country"],
    "map_code_column": "iso_code",
    "filters": [{"column": "year", "op": "=", "value": "2023"}],
    "sort_by": {"column": "avg_tc", "direction": "desc"},
}, {
    "color": ["#fef3c7", "#fbbf24", "#f59e0b", "#ef4444", "#991b1b"],
    "chartTypeOptions": {"mapRegion": "world"},
})

# Top 15 Carbon Intensity - bar
c7 = chart("Top 15 · Carbon Intensity", "bar", {
    "dataset_id": 132,
    "datasource": f"{DS}.energy_annual",
    "query_mode": "aggregate",
    "metrics": [{"column": "carbon_intensity_elec", "aggregate": "AVG", "label": "gCO₂/kWh"}],
    "groupby": ["country"],
    "filters": [{"column": "year", "op": "=", "value": "2023"}],
    "sort_by": {"column": "carbon_intensity_elec", "direction": "desc"},
    "row_limit": 15,
}, {
    "echarts_option": {"color": ["#ef4444"]},
    "chartTypeOptions": {"horizontal": True},
})

# Energy Mix - stacked bar
c8 = chart("Energy Mix · Top 10 Consumers", "bar", {
    "dataset_id": 132,
    "datasource": f"{DS}.energy_annual",
    "query_mode": "aggregate",
    "metrics": [
        {"column": "solar_electricity", "aggregate": "SUM", "label": "Solar"},
        {"column": "wind_electricity", "aggregate": "SUM", "label": "Wind"},
        {"column": "hydro_electricity", "aggregate": "SUM", "label": "Hydro"},
        {"column": "nuclear_electricity", "aggregate": "SUM", "label": "Nuclear"},
        {"column": "coal_electricity", "aggregate": "SUM", "label": "Coal"},
        {"column": "gas_electricity", "aggregate": "SUM", "label": "Gas"},
    ],
    "groupby": ["country"],
    "filters": [{"column": "year", "op": "=", "value": "2023"}],
    "sort_by": {"column": "primary_energy_consumption", "direction": "desc"},
    "row_limit": 10,
}, {
    "echarts_option": {"color": ["#facc15", "#22d3ee", "#3b82f6", "#a855f7", "#6b7280", "#f97316"]},
    "chartTypeOptions": {"stacked": True},
})

# GHG Trend - line
c9 = chart("GHG Emissions Trend · Top 5", "line", {
    "dataset_id": 132,
    "datasource": f"{DS}.energy_annual",
    "query_mode": "aggregate",
    "metrics": [{"column": "greenhouse_gas_emissions", "aggregate": "SUM", "label": "Mt CO₂e"}],
    "groupby": ["year"],
    "series_groupby": ["country"],
    "filters": [{"column": "country", "op": "IN", "value": ["China", "United States", "India", "Russia", "Japan"]}],
    "sort_by": {"column": "year", "direction": "asc"},
})

# Scatter: temp vs renewables
c10 = chart("Temperature vs Renewables Adoption", "scatter", {
    "dataset_id": 134,
    "datasource": f"{DS}.climate_x_energy",
    "query_mode": "raw",
    "columns": ["country", "avg_tc", "renewables_share_energy", "primary_energy_consumption"],
    "filters": [
        {"column": "year", "op": "=", "value": "2023"},
        {"column": "renewables_share_energy", "op": "IS NOT NULL"},
        {"column": "avg_tc", "op": "IS NOT NULL"},
    ],
    "x_axis": "avg_tc",
    "y_axis": "renewables_share_energy",
    "size_column": "primary_energy_consumption",
    "label_column": "country",
})

print("\n=== Creating dashboard ===")

dash_id = uuid.uuid4().hex
chart_ids = [c1, c2, c3, c4, c5, c6, c7, c8, c9, c10]

layout = json.dumps([
    {"type": "row", "children": [
        {"type": "chart", "chartId": c1, "width": 25},
        {"type": "chart", "chartId": c2, "width": 25},
        {"type": "chart", "chartId": c3, "width": 25},
        {"type": "chart", "chartId": c4, "width": 25},
    ]},
    {"type": "row", "children": [
        {"type": "chart", "chartId": c5, "width": 50},
        {"type": "chart", "chartId": c6, "width": 50},
    ]},
    {"type": "row", "children": [
        {"type": "chart", "chartId": c10, "width": 50},
        {"type": "chart", "chartId": c7, "width": 50},
    ]},
    {"type": "row", "children": [
        {"type": "chart", "chartId": c8, "width": 50},
        {"type": "chart", "chartId": c9, "width": 50},
    ]},
])

cur.execute(
    """INSERT INTO dashboards (id, name, description, layout, charts, theme,
       is_published, visibility, created_by)
    VALUES (%s, %s, %s, %s, %s, 'dark', true, 'internal', 'pruthvi.prodduturi@gmail.com')""",
    (
        dash_id,
        "Global Energy & Climate",
        "Cross-domain: temperature anomalies vs energy consumption, renewable adoption, and carbon intensity across 188 countries. Source: Our World in Data + FAO.",
        layout,
        json.dumps(chart_ids),
    ),
)

print(f"Dashboard: {dash_id}")
print(f"Charts: {chart_ids}")
print(f"\nView: kaveon.vercel.app/dashboards/{dash_id}/view")

conn.close()
print("Done!")
