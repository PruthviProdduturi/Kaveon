"""Create 3 separate Climate/Energy dashboards in Kaveon."""
import os, psycopg2, json, uuid, random, string

conn = psycopg2.connect(
    host=os.environ.get("PGHOST", "kaveon-db.postgres.database.azure.com"),
    dbname=os.environ.get("PGDATABASE", "kaveon"),
    user=os.environ["PGUSER"], password=os.environ["PGPASSWORD"], sslmode="require",
)
conn.autocommit = True
cur = conn.cursor()

def uid(): return "c" + "".join(random.choices(string.hexdigits[:16], k=8))

def chart(name, ctype, qc, vc=None):
    cur.execute("INSERT INTO charts (name,chart_type,query_config,viz_config,visibility,created_by) VALUES (%s,%s,%s,%s,'internal','pruthvi.prodduturi@gmail.com') RETURNING id",
        (name, ctype, json.dumps(qc), json.dumps(vc or {})))
    cid = cur.fetchone()[0]
    print(f"  {cid}: {name}")
    return cid

def dash(name, desc, cids, layout):
    did = uuid.uuid4().hex
    cur.execute("INSERT INTO dashboards (id,name,description,layout,charts,theme,is_published,visibility,created_by) VALUES (%s,%s,%s,%s,%s,'dark',true,'internal','pruthvi.prodduturi@gmail.com')",
        (did, name, desc, json.dumps(layout), json.dumps(cids)))
    print(f"  Dashboard: {name}")
    return did

CE = "kaveon.climate_energy"

# ═══ DASHBOARD 1: GLOBAL CLIMATE ═══
print("=== GLOBAL CLIMATE ===")
c1 = chart("Countries Tracked", "big_number", {"dataset_id":133,"datasource":f"{CE}.temperature_monthly","query_mode":"aggregate","metrics":[{"column":"country","aggregate":"COUNT_DISTINCT","label":"Countries"}]})
c2 = chart("Avg Temp Anomaly 2023", "big_number", {"dataset_id":133,"datasource":f"{CE}.temperature_monthly","query_mode":"aggregate","metrics":[{"column":"temp_change_c","aggregate":"AVG","label":"C"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
c3 = chart("Max Temp Anomaly 2023", "big_number", {"dataset_id":133,"datasource":f"{CE}.temperature_monthly","query_mode":"aggregate","metrics":[{"column":"temp_change_c","aggregate":"MAX","label":"C"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
c4 = chart("Data Coverage", "big_number", {"dataset_id":133,"datasource":f"{CE}.temperature_monthly","query_mode":"aggregate","metrics":[{"column":"year","aggregate":"COUNT_DISTINCT","label":"Years"}]})
c5 = chart("Temperature Anomaly Map", "world_map", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"aggregate","template":"world_map","metrics":[{"column":"avg_tc","aggregate":"AVG","label":"Temp C"}],"groupby":["country"],"map_code_column":"iso_code","filters":[{"column":"year","op":"=","value":"2023"}]},
    {"color":["#fef3c7","#fbbf24","#f59e0b","#ef4444","#991b1b"],"chartTypeOptions":{"mapRegion":"world"}})
c6 = chart("Warming Trend - Arctic Nations", "line", {"dataset_id":133,"datasource":f"{CE}.temperature_monthly","query_mode":"aggregate","metrics":[{"column":"temp_change_c","aggregate":"AVG","label":"C"}],"groupby":["year"],"series_groupby":["country"],"filters":[{"column":"country","op":"IN","value":["Russia","Canada","Finland","Norway","Sweden"]}],"sort_by":{"column":"year","direction":"asc"}})
c7 = chart("Monthly Pattern 2023", "bar", {"dataset_id":133,"datasource":f"{CE}.temperature_monthly","query_mode":"aggregate","metrics":[{"column":"temp_change_c","aggregate":"AVG","label":"C"}],"groupby":["month"],"filters":[{"column":"year","op":"=","value":"2023"}],"sort_by":{"column":"month","direction":"asc"}},{"echarts_option":{"color":["#ef4444"]}})

cc = [c1,c2,c3,c4,c5,c6,c7]
dash("Global Climate", "Temperature anomaly by country. 288 countries, monthly 2020-2025 vs 1951-1980 baseline. Source: FAO.", cc, [
    {"i":uid(),"type":"chart","chartId":c1,"x":0,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":c2,"x":3,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":c3,"x":6,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":c4,"x":9,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":c5,"x":0,"y":4,"w":7,"h":8,"minW":4,"minH":4,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":c6,"x":7,"y":4,"w":5,"h":8,"minW":3,"minH":4,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":c7,"x":0,"y":12,"w":12,"h":6,"minW":4,"minH":4,"maxW":12,"maxH":40},
])

# ═══ DASHBOARD 2: GLOBAL ENERGY ═══
print("\n=== GLOBAL ENERGY ===")
e1 = chart("Total Energy 2023", "big_number", {"dataset_id":132,"datasource":f"{CE}.energy_annual","query_mode":"aggregate","metrics":[{"column":"primary_energy_consumption","aggregate":"SUM","label":"TWh"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
e2 = chart("Avg Renewables %", "big_number", {"dataset_id":132,"datasource":f"{CE}.energy_annual","query_mode":"aggregate","metrics":[{"column":"renewables_share_energy","aggregate":"AVG","label":"%"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
e3 = chart("Avg Carbon Intensity", "big_number", {"dataset_id":132,"datasource":f"{CE}.energy_annual","query_mode":"aggregate","metrics":[{"column":"carbon_intensity_elec","aggregate":"AVG","label":"gCO2/kWh"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
e4 = chart("Total GHG 2023", "big_number", {"dataset_id":132,"datasource":f"{CE}.energy_annual","query_mode":"aggregate","metrics":[{"column":"greenhouse_gas_emissions","aggregate":"SUM","label":"Mt CO2e"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
e5 = chart("Energy Consumption Map", "world_map", {"dataset_id":132,"datasource":f"{CE}.energy_annual","query_mode":"aggregate","template":"world_map","metrics":[{"column":"primary_energy_consumption","aggregate":"SUM","label":"TWh"}],"groupby":["country"],"map_code_column":"iso_code","filters":[{"column":"year","op":"=","value":"2023"}]},
    {"color":["#eff6ff","#93c5fd","#3b82f6","#1d4ed8","#1e3a8a"],"chartTypeOptions":{"mapRegion":"world"}})
e6 = chart("Top 15 Carbon Intensity", "bar", {"dataset_id":132,"datasource":f"{CE}.energy_annual","query_mode":"aggregate","metrics":[{"column":"carbon_intensity_elec","aggregate":"AVG","label":"gCO2/kWh"}],"groupby":["country"],"filters":[{"column":"year","op":"=","value":"2023"}],"sort_by":{"column":"carbon_intensity_elec","direction":"desc"},"row_limit":15},{"echarts_option":{"color":["#ef4444"]},"chartTypeOptions":{"horizontal":True}})
e7 = chart("Energy Mix - Top 10", "stacked_bar", {"dataset_id":132,"datasource":f"{CE}.energy_annual","query_mode":"aggregate","metrics":[{"column":"solar_electricity","aggregate":"SUM","label":"Solar"},{"column":"wind_electricity","aggregate":"SUM","label":"Wind"},{"column":"hydro_electricity","aggregate":"SUM","label":"Hydro"},{"column":"nuclear_electricity","aggregate":"SUM","label":"Nuclear"},{"column":"coal_electricity","aggregate":"SUM","label":"Coal"},{"column":"gas_electricity","aggregate":"SUM","label":"Gas"}],"groupby":["country"],"filters":[{"column":"year","op":"=","value":"2023"}],"sort_by":{"column":"primary_energy_consumption","direction":"desc"},"row_limit":10},{"echarts_option":{"color":["#facc15","#22d3ee","#3b82f6","#a855f7","#6b7280","#f97316"],"showAsPercentage":True}})
e8 = chart("GHG Trend - Top 5", "line", {"dataset_id":132,"datasource":f"{CE}.energy_annual","query_mode":"aggregate","metrics":[{"column":"greenhouse_gas_emissions","aggregate":"SUM","label":"Mt CO2e"}],"groupby":["year"],"series_groupby":["country"],"filters":[{"column":"country","op":"IN","value":["China","United States","India","Russia","Japan"]}],"sort_by":{"column":"year","direction":"asc"}})

ec = [e1,e2,e3,e4,e5,e6,e7,e8]
dash("Global Energy", "220 countries - consumption, generation, fossil vs renewable mix, carbon intensity, GHG emissions. Source: Our World in Data.", ec, [
    {"i":uid(),"type":"chart","chartId":e1,"x":0,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":e2,"x":3,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":e3,"x":6,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":e4,"x":9,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":e5,"x":0,"y":4,"w":7,"h":8,"minW":4,"minH":4,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":e6,"x":7,"y":4,"w":5,"h":8,"minW":3,"minH":4,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":e7,"x":0,"y":12,"w":6,"h":7,"minW":3,"minH":4,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":e8,"x":6,"y":12,"w":6,"h":7,"minW":3,"minH":4,"maxW":12,"maxH":40},
])

# ═══ DASHBOARD 3: CLIMATE x ENERGY IMPACT ═══
print("\n=== CLIMATE x ENERGY IMPACT ===")
x1 = chart("Avg Warming 2023", "big_number", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"aggregate","metrics":[{"column":"avg_tc","aggregate":"AVG","label":"C"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
x2 = chart("Countries in Study", "big_number", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"aggregate","metrics":[{"column":"country","aggregate":"COUNT","label":""}],"filters":[{"column":"year","op":"=","value":"2023"}]})
x3 = chart("Avg Fossil Share", "big_number", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"aggregate","metrics":[{"column":"fossil_share_energy","aggregate":"AVG","label":"%"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
x4 = chart("Total Emissions 2023", "big_number", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"aggregate","metrics":[{"column":"greenhouse_gas_emissions","aggregate":"SUM","label":"Mt CO2e"}],"filters":[{"column":"year","op":"=","value":"2023"}]})
x5 = chart("Warming vs Renewables Adoption", "scatter", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"raw","columns":["country","avg_tc","renewables_share_energy","primary_energy_consumption"],"x_axis":"avg_tc","y_axis":"renewables_share_energy","size_column":"primary_energy_consumption","label_column":"country","filters":[{"column":"year","op":"=","value":"2023"}]})
x6 = chart("Warming vs Carbon Intensity", "scatter", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"raw","columns":["country","avg_tc","carbon_intensity_elec","primary_energy_consumption"],"x_axis":"avg_tc","y_axis":"carbon_intensity_elec","size_column":"primary_energy_consumption","label_column":"country","filters":[{"column":"year","op":"=","value":"2023"}]})
x7 = chart("Hottest Countries by Energy Use", "bar", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"aggregate","metrics":[{"column":"primary_energy_consumption","aggregate":"SUM","label":"TWh"}],"groupby":["country"],"filters":[{"column":"year","op":"=","value":"2023"}],"sort_by":{"column":"avg_tc","direction":"desc"},"row_limit":20},{"echarts_option":{"color":["#f59e0b"]},"chartTypeOptions":{"horizontal":True}})
x8 = chart("Top Emitters by GHG", "bar", {"dataset_id":134,"datasource":f"{CE}.climate_x_energy","query_mode":"aggregate","metrics":[{"column":"greenhouse_gas_emissions","aggregate":"SUM","label":"Mt CO2e"}],"groupby":["country"],"filters":[{"column":"year","op":"=","value":"2023"}],"sort_by":{"column":"greenhouse_gas_emissions","direction":"desc"},"row_limit":15},{"echarts_option":{"color":["#ef4444"]},"chartTypeOptions":{"horizontal":True}})

xc = [x1,x2,x3,x4,x5,x6,x7,x8]
dash("Climate x Energy Impact", "Cross-domain: does warming correlate with energy use? Temperature anomaly vs fossil consumption, renewables adoption, carbon intensity. 188 countries joined by country+year.", xc, [
    {"i":uid(),"type":"chart","chartId":x1,"x":0,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":x2,"x":3,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":x3,"x":6,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":x4,"x":9,"y":0,"w":3,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":x5,"x":0,"y":4,"w":6,"h":8,"minW":3,"minH":4,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":x6,"x":6,"y":4,"w":6,"h":8,"minW":3,"minH":4,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":x7,"x":0,"y":12,"w":6,"h":7,"minW":3,"minH":4,"maxW":12,"maxH":40},
    {"i":uid(),"type":"chart","chartId":x8,"x":6,"y":12,"w":6,"h":7,"minW":3,"minH":4,"maxW":12,"maxH":40},
])

print("\n=== FINAL ===")
cur.execute("SELECT name, is_published FROM dashboards ORDER BY name")
for r in cur.fetchall(): print(f"  {'Published' if r[1] else 'Draft':10} {r[0]}")
cur.execute("SELECT COUNT(*) FROM charts")
print(f"  Total charts: {cur.fetchone()[0]}")
conn.close()
print("Done!")
