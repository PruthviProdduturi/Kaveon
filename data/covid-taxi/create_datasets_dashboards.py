"""Create COVID + Taxi datasets and dashboards."""
import os, psycopg2, json, uuid, random, string

conn = psycopg2.connect(
    host="kaveon-db.postgres.database.azure.com", dbname="kaveon",
    user="kaveon_admin", password=os.environ["PGPASSWORD"], sslmode="require")
conn.autocommit = True
cur = conn.cursor()
U = "pruthvi.prodduturi@gmail.com"

def uid():
    return "c" + "".join(random.choices(string.hexdigits[:16], k=8))

def chart(name, ctype, qc, vc=None):
    cur.execute("INSERT INTO charts (name,chart_type,query_config,viz_config,visibility,created_by) VALUES (%s,%s,%s,%s,'internal',%s) RETURNING id",
        (name, ctype, json.dumps(qc), json.dumps(vc or {}), U))
    cid = cur.fetchone()[0]
    print(f"  Chart {cid}: {name}")
    return cid

# COVID Dataset
cur.execute("INSERT INTO datasets (dataset_name,description,fact_table,schema_name,database_name,date_column,created_by) VALUES (%s,%s,%s,'public','kaveon','dt',%s) RETURNING id",
    ("COVID-19 Global", "Global COVID-19 cases and deaths. 234 countries, daily 2024. Source: Our World in Data.", "covid_global", U))
covid_ds = cur.fetchone()[0]
for c in [("country","varchar",True,False,"dimension"),("iso_code","varchar",True,False,"dimension"),("continent","varchar",True,False,"dimension"),("dt","date",False,False,"time"),("total_cases","bigint",False,True,"metric"),("new_cases","integer",False,True,"metric"),("total_deaths","bigint",False,True,"metric"),("new_deaths","integer",False,True,"metric"),("population","bigint",False,True,"metric")]:
    cur.execute("INSERT INTO dataset_columns (dataset_id,table_name,column_name,data_type,is_dimension,is_metric,semantic_type) VALUES (%s,'covid_global',%s,%s,%s,%s,%s)", (covid_ds, c[0], c[1], c[2], c[3], c[4]))
for m in [("Total Cases","MAX(total_cases)","max"),("New Cases","SUM(new_cases)","sum"),("Total Deaths","MAX(total_deaths)","max"),("New Deaths","SUM(new_deaths)","sum")]:
    cur.execute("INSERT INTO dataset_metrics (dataset_id,metric_name,expression,metric_type) VALUES (%s,%s,%s,%s)", (covid_ds, m[0], m[1], m[2]))
print(f"COVID dataset: {covid_ds}")

# Taxi Dataset
cur.execute("INSERT INTO datasets (dataset_name,description,fact_table,schema_name,database_name,created_by) VALUES (%s,%s,%s,'public','kaveon',%s) RETURNING id",
    ("NYC Yellow Taxi", "3M yellow taxi trips, Jan 2023. By borough. Source: NYC TLC.", "nyc_taxi_borough", U))
taxi_ds = cur.fetchone()[0]
for c in [("borough","varchar",True,False,"dimension"),("trips","integer",False,True,"metric"),("revenue","numeric",False,True,"metric"),("avg_fare","numeric",False,True,"metric"),("avg_distance","numeric",False,True,"metric")]:
    cur.execute("INSERT INTO dataset_columns (dataset_id,table_name,column_name,data_type,is_dimension,is_metric,semantic_type) VALUES (%s,'nyc_taxi_borough',%s,%s,%s,%s,%s)", (taxi_ds, c[0], c[1], c[2], c[3], c[4]))
for m in [("Total Trips","SUM(trips)","sum"),("Total Revenue","SUM(revenue)","sum"),("Avg Fare","AVG(avg_fare)","avg")]:
    cur.execute("INSERT INTO dataset_metrics (dataset_id,metric_name,expression,metric_type) VALUES (%s,%s,%s,%s)", (taxi_ds, m[0], m[1], m[2]))
print(f"Taxi dataset: {taxi_ds}")

# COVID Dashboard
print("\n=== COVID Dashboard ===")
c1 = chart("Total Cases", "big_number", {"dataset_id": covid_ds, "datasource": "kaveon.public.covid_global", "query_mode": "aggregate", "metrics": [{"column": "total_cases", "aggregate": "MAX", "label": "Cases"}]})
c2 = chart("Total Deaths", "big_number", {"dataset_id": covid_ds, "datasource": "kaveon.public.covid_global", "query_mode": "aggregate", "metrics": [{"column": "total_deaths", "aggregate": "MAX", "label": "Deaths"}]})
c3 = chart("Countries", "big_number", {"dataset_id": covid_ds, "datasource": "kaveon.public.covid_global", "query_mode": "aggregate", "metrics": [{"column": "country", "aggregate": "COUNT_DISTINCT", "label": "Countries"}]})
c4 = chart("Cases by Country", "world_map", {"dataset_id": covid_ds, "datasource": "kaveon.public.covid_global", "query_mode": "aggregate", "template": "world_map", "metrics": [{"column": "total_cases", "aggregate": "MAX", "label": "Cases"}], "groupby": ["country"], "map_code_column": "iso_code"}, {"color": ["#fef3c7", "#f59e0b", "#ef4444", "#991b1b"], "chartTypeOptions": {"mapRegion": "world"}})
c5 = chart("Daily New Cases", "line", {"dataset_id": covid_ds, "datasource": "kaveon.public.covid_global", "query_mode": "aggregate", "metrics": [{"column": "new_cases", "aggregate": "SUM", "label": "New Cases"}], "groupby": ["dt"], "sort_by": {"column": "dt", "direction": "asc"}})
c6 = chart("Deaths by Continent", "bar", {"dataset_id": covid_ds, "datasource": "kaveon.public.covid_global", "query_mode": "aggregate", "metrics": [{"column": "total_deaths", "aggregate": "MAX", "label": "Deaths"}], "groupby": ["continent"], "sort_by": {"column": "total_deaths", "direction": "desc"}}, {"echarts_option": {"color": ["#ef4444"]}, "chartTypeOptions": {"horizontal": True}})

did1 = uuid.uuid4().hex
cur.execute("INSERT INTO dashboards (id,name,description,layout,charts,theme,is_published,visibility,created_by) VALUES (%s,%s,%s,%s,%s,'dark',true,'internal',%s)",
    (did1, "COVID-19 Global Overview", "Pandemic snapshot: cases, deaths, world map, trends. 234 countries, 2024.",
     json.dumps([
        {"i":uid(),"type":"chart","chartId":c1,"x":0,"y":0,"w":4,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":c2,"x":4,"y":0,"w":4,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":c3,"x":8,"y":0,"w":4,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":c4,"x":0,"y":4,"w":7,"h":8,"minW":4,"minH":4,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":c6,"x":7,"y":4,"w":5,"h":8,"minW":3,"minH":4,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":c5,"x":0,"y":12,"w":12,"h":6,"minW":4,"minH":4,"maxW":12,"maxH":40},
     ]), json.dumps([c1,c2,c3,c4,c5,c6]), U))
print(f"Dashboard: {did1}")

# Taxi Dashboard
print("\n=== Taxi Dashboard ===")
t1 = chart("Total Trips", "big_number", {"dataset_id": taxi_ds, "datasource": "kaveon.public.nyc_taxi_borough", "query_mode": "aggregate", "metrics": [{"column": "trips", "aggregate": "SUM", "label": "Trips"}]})
t2 = chart("Total Revenue", "big_number", {"dataset_id": taxi_ds, "datasource": "kaveon.public.nyc_taxi_borough", "query_mode": "aggregate", "metrics": [{"column": "revenue", "aggregate": "SUM", "label": "$"}]})
t3 = chart("Avg Fare", "big_number", {"dataset_id": taxi_ds, "datasource": "kaveon.public.nyc_taxi_borough", "query_mode": "aggregate", "metrics": [{"column": "avg_fare", "aggregate": "AVG", "label": "$"}]})
t4 = chart("Trips by Borough", "bar", {"dataset_id": taxi_ds, "datasource": "kaveon.public.nyc_taxi_borough", "query_mode": "aggregate", "metrics": [{"column": "trips", "aggregate": "SUM", "label": "Trips"}], "groupby": ["borough"], "sort_by": {"column": "trips", "direction": "desc"}}, {"echarts_option": {"color": ["#4A9EE8"]}, "chartTypeOptions": {"horizontal": True}})
t5 = chart("Revenue by Borough", "pie", {"dataset_id": taxi_ds, "datasource": "kaveon.public.nyc_taxi_borough", "query_mode": "aggregate", "metrics": [{"column": "revenue", "aggregate": "SUM", "label": "Revenue"}], "groupby": ["borough"]})

did2 = uuid.uuid4().hex
cur.execute("INSERT INTO dashboards (id,name,description,layout,charts,theme,is_published,visibility,created_by) VALUES (%s,%s,%s,%s,%s,'dark',true,'internal',%s)",
    (did2, "NYC Yellow Taxi", "3M taxi trips, Jan 2023. Borough breakdown, revenue, fares. Source: NYC TLC.",
     json.dumps([
        {"i":uid(),"type":"chart","chartId":t1,"x":0,"y":0,"w":4,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":t2,"x":4,"y":0,"w":4,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":t3,"x":8,"y":0,"w":4,"h":4,"minW":2,"minH":2,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":t4,"x":0,"y":4,"w":7,"h":8,"minW":3,"minH":4,"maxW":12,"maxH":40},
        {"i":uid(),"type":"chart","chartId":t5,"x":7,"y":4,"w":5,"h":8,"minW":3,"minH":4,"maxW":12,"maxH":40},
     ]), json.dumps([t1,t2,t3,t4,t5]), U))
print(f"Dashboard: {did2}")

print("\n=== FINAL ===")
cur.execute("SELECT name FROM dashboards ORDER BY name")
for r in cur.fetchall(): print(f"  {r[0]}")
cur.execute("SELECT COUNT(*) FROM datasets")
print(f"Datasets: {cur.fetchone()[0]}")
conn.close()
print("Done!")
