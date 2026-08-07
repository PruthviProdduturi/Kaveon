"""
Kaveon — Migrate Neon Postgres → Azure Postgres
Run as a one-time Container App job or from Azure Cloud Shell.

Usage:
  az containerapp job create --name kaveon-migrate --resource-group kaveon-rg \
    --environment kaveon-env --image kaveonacr.azurecr.io/kaveon-api:latest \
    --registry-server kaveonacr.azurecr.io --registry-username kaveonacr \
    --registry-password <ACR_PASSWORD> \
    --trigger-type Manual --replica-timeout 600 --replica-retry-limit 0 \
    --cpu 0.5 --memory 1.0Gi \
    --command "python" "/app/scripts/migrate-neon-to-azure.py"
"""

import os
import psycopg2
import sys

# Credentials come from the environment / Key Vault — never hardcode them here.
# Source these from `kaveon-kv` (neon-db-password, azure-db-password) before running,
# e.g. as a Container App job with secretRefs, or locally via env vars.
SRC = dict(
    host=os.environ["NEON_HOST"],
    database=os.environ.get("NEON_DB", "neondb"),
    user=os.environ.get("NEON_USER", "neondb_owner"),
    password=os.environ["NEON_PASSWORD"],
    sslmode="require",
)

DST = dict(
    host=os.environ.get("AZ_PG_HOST", "kaveon-db.postgres.database.azure.com"),
    database=os.environ.get("AZ_PG_DB", "kaveon"),
    user=os.environ.get("AZ_PG_USER", "kaveon_admin"),
    password=os.environ["AZ_PG_PASSWORD"],
    sslmode="require",
)

def migrate():
    print("Connecting to Neon (source)...")
    src = psycopg2.connect(**SRC)
    print("Connecting to Azure Postgres (destination)...")
    dst = psycopg2.connect(**DST)
    src.autocommit = True
    sc = src.cursor()
    dc = dst.cursor()

    # Get all tables
    sc.execute("""
        SELECT table_name FROM information_schema.tables
        WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
        ORDER BY table_name
    """)
    tables = [r[0] for r in sc.fetchall()]
    print(f"Found {len(tables)} tables: {', '.join(tables)}")

    # Step 1: Recreate schema
    print("\n--- Creating tables ---")
    for t in tables:
        sc.execute(f"""
            SELECT column_name, data_type, character_maximum_length,
                   is_nullable, column_default, numeric_precision, numeric_scale
            FROM information_schema.columns
            WHERE table_name = '{t}' AND table_schema = 'public'
            ORDER BY ordinal_position
        """)
        cols = sc.fetchall()

        col_defs = []
        for name, dtype, max_len, nullable, default, num_prec, num_scale in cols:
            # Map types
            if default and "nextval" in str(default):
                d = "SERIAL" if dtype == "integer" else "BIGSERIAL"
            elif dtype == "character varying":
                d = f"varchar({max_len})" if max_len else "text"
            elif dtype == "numeric" and num_prec:
                d = f"numeric({num_prec},{num_scale or 0})"
            elif dtype == "timestamp without time zone":
                d = "timestamp"
            elif dtype == "timestamp with time zone":
                d = "timestamptz"
            elif dtype == "double precision":
                d = "double precision"
            elif dtype == "USER-DEFINED":
                d = "text"  # fallback for enums etc
            else:
                d = dtype

            parts = [f'"{name}" {d}']
            if "nextval" not in str(default or ""):
                if nullable == "NO":
                    parts.append("NOT NULL")
                if default:
                    parts.append(f"DEFAULT {default}")
            col_defs.append(" ".join(parts))

        # Primary key
        sc.execute(f"""
            SELECT kcu.column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
            WHERE tc.table_name = '{t}'
              AND tc.constraint_type = 'PRIMARY KEY'
            ORDER BY kcu.ordinal_position
        """)
        pk_cols = [r[0] for r in sc.fetchall()]

        create_sql = f'CREATE TABLE IF NOT EXISTS "{t}" (\n  '
        create_sql += ",\n  ".join(col_defs)
        if pk_cols:
            pk_str = ", ".join(f'"{c}"' for c in pk_cols)
            create_sql += f",\n  PRIMARY KEY ({pk_str})"
        create_sql += "\n)"

        try:
            dc.execute(f'DROP TABLE IF EXISTS "{t}" CASCADE')
            dc.execute(create_sql)
            dst.commit()
            print(f"  ✓ {t}")
        except Exception as e:
            print(f"  ✗ {t}: {e}")
            dst.rollback()

    # Step 2: Copy data
    print("\n--- Copying data ---")
    total_rows = 0
    for t in tables:
        sc.execute(f'SELECT * FROM "{t}"')
        rows = sc.fetchall()
        if not rows:
            print(f"  {t}: 0 rows")
            continue

        sc.execute(f"""
            SELECT column_name FROM information_schema.columns
            WHERE table_name = '{t}' AND table_schema = 'public'
            ORDER BY ordinal_position
        """)
        col_names = [r[0] for r in sc.fetchall()]
        placeholders = ",".join(["%s"] * len(col_names))
        cols_quoted = ",".join(f'"{c}"' for c in col_names)

        try:
            # Batch insert in chunks of 500
            for i in range(0, len(rows), 500):
                chunk = rows[i : i + 500]
                dc.executemany(
                    f'INSERT INTO "{t}" ({cols_quoted}) VALUES ({placeholders})',
                    chunk,
                )
            dst.commit()
            total_rows += len(rows)
            print(f"  {t}: {len(rows)} rows ✓")
        except Exception as e:
            print(f"  {t}: ERROR — {e}")
            dst.rollback()

    # Step 3: Fix sequences
    print("\n--- Fixing sequences ---")
    for t in tables:
        sc.execute(f"""
            SELECT column_name, column_default
            FROM information_schema.columns
            WHERE table_name = '{t}' AND column_default LIKE 'nextval%%'
        """)
        for col, default in sc.fetchall():
            try:
                dc.execute(f'SELECT COALESCE(MAX("{col}"), 0) FROM "{t}"')
                max_val = dc.fetchone()[0]
                seq_name = default.split("'")[1]
                dc.execute(f"SELECT setval('{seq_name}', {max_val + 1}, false)")
                dst.commit()
                print(f"  {t}.{col} → {max_val + 1}")
            except Exception as e:
                print(f"  {t}.{col}: {e}")
                dst.rollback()

    src.close()
    dst.close()
    print(f"\n{'='*50}")
    print(f"Migration complete! {total_rows} total rows across {len(tables)} tables.")
    print(f"{'='*50}")


if __name__ == "__main__":
    try:
        migrate()
    except Exception as e:
        print(f"FATAL: {e}", file=sys.stderr)
        sys.exit(1)
