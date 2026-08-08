"""Fix data sources after Neon→Azure migration.
Run inside the container: python scripts/fix-datasources.py
"""
import os, psycopg2

conn = psycopg2.connect(
    host=os.environ.get("METADATA_HOST", "kaveon-db.postgres.database.azure.com"),
    database=os.environ.get("METADATA_DATABASE", "kaveon"),
    user=os.environ.get("METADATA_USER", "kaveon_admin"),
    password=os.environ.get("METADATA_PASSWORD", ""),
    sslmode="require",
)
cur = conn.cursor()

# List current data sources
cur.execute("SELECT id, name, database_name, type, connection_string FROM data_sources")
rows = cur.fetchall()
print(f"Found {len(rows)} data sources:")
for r in rows:
    cs = str(r[4] or "")[:80]
    print(f"  ID:{r[0]} Name:{r[1]} DB:{r[2]} Type:{r[3]} Conn:{cs}...")

# Update connection strings from Neon to Azure Postgres
# The data tables live in the same Azure PG database as metadata
az_host = os.environ.get("METADATA_HOST", "kaveon-db.postgres.database.azure.com")
az_db = os.environ.get("METADATA_DATABASE", "kaveon")
az_user = os.environ.get("METADATA_USER", "kaveon_admin")
az_pass = os.environ.get("METADATA_PASSWORD", "")

new_conn_str = f"postgresql://{az_user}:{az_pass}@{az_host}:5432/{az_db}?sslmode=require"

for r in rows:
    ds_id, name, db_name, ds_type, old_cs = r
    old_cs = str(old_cs or "")
    # Only update PostgreSQL sources that point to Neon
    if "neon" in old_cs.lower() or "neondb" in str(db_name or "").lower():
        print(f"\n  Updating ID:{ds_id} ({name}) from Neon to Azure PG...")
        cur.execute(
            "UPDATE data_sources SET connection_string = %s, database_name = %s WHERE id = %s",
            (new_conn_str, az_db, ds_id),
        )
        print(f"  Done: {name} → {az_host}/{az_db}")

conn.commit()
cur.close()
conn.close()
print("\nAll data sources updated.")
