"""Create deterministic, explicitly synthetic Kaveon product-usage Parquet.

No real telemetry is read. Writes only /work for the AKS job; the separate
uploader places immutable outputs below the configured OpenSource snapshot.
"""
from __future__ import annotations
import json
from datetime import date, timedelta
from contextlib import ExitStack
from pathlib import Path
import pyarrow as pa
import pyarrow.parquet as pq
USERS, DAYS = (44000, 230)
ROOT = Path('/work/kaveon_product')
GEOS = [('en-US', 'United States', 'North America'), ('en-CA', 'Canada', 'North America'), ('es-MX', 'Mexico', 'North America'), ('en-GB', 'United Kingdom', 'Europe'), ('de-DE', 'Germany', 'Europe'), ('fr-FR', 'France', 'Europe'), ('nl-NL', 'Netherlands', 'Europe'), ('es-ES', 'Spain', 'Europe'), ('sv-SE', 'Sweden', 'Europe'), ('it-IT', 'Italy', 'Europe'), ('hi-IN', 'India', 'Asia'), ('ja-JP', 'Japan', 'Asia'), ('en-SG', 'Singapore', 'Asia'), ('ko-KR', 'South Korea', 'Asia'), ('id-ID', 'Indonesia', 'Asia'), ('zh-CN', 'China', 'Asia'), ('pt-BR', 'Brazil', 'South America'), ('es-AR', 'Argentina', 'South America'), ('es-CO', 'Colombia', 'South America'), ('es-CL', 'Chile', 'South America'), ('en-NG', 'Nigeria', 'Africa'), ('en-ZA', 'South Africa', 'Africa'), ('sw-KE', 'Kenya', 'Africa'), ('ar-EG', 'Egypt', 'Africa'), ('en-AU', 'Australia', 'Oceania'), ('en-NZ', 'New Zealand', 'Oceania')]
PLATFORMS = [('desktop-win', 'Desktop', 'Windows'), ('desktop-mac', 'Desktop', 'macOS'), ('desktop-linux', 'Desktop', 'Linux'), ('mobile-ios', 'Mobile', 'iOS'), ('mobile-android', 'Mobile', 'Android'), ('web-win', 'Web', 'Windows'), ('web-mac', 'Web', 'macOS'), ('web-linux', 'Web', 'Linux'), ('web-chromeos', 'Web', 'ChromeOS')]
LOCALES = 'en-US,en-US,en-US,en-US,en-CA,es-MX,en-GB,en-GB,de-DE,fr-FR,nl-NL,es-ES,sv-SE,it-IT,hi-IN,hi-IN,ja-JP,en-SG,ko-KR,id-ID,zh-CN,pt-BR,es-AR,es-CO,es-CL,en-NG,en-ZA,sw-KE,ar-EG,en-AU,en-NZ'.split(',')
PLATFORM_POOL = 'desktop-win,desktop-win,desktop-win,desktop-mac,desktop-mac,desktop-linux,mobile-ios,mobile-ios,mobile-android,mobile-android,mobile-android,web-win,web-win,web-mac,web-linux,web-chromeos'.split(',')
LICENSES = 'Free,Free,Free,Standard,Standard,Professional,Professional,Enterprise'.split(',')
AUDIENCES = 'Consumer,Consumer,Consumer,Commercial,Commercial,Commercial,Education,Government'.split(',')
SEGMENTS = 'Enterprise,Enterprise,Mid-Market,Mid-Market,Mid-Market,SMB,SMB,Startup,Startup'.split(',')
INDUSTRIES = 'Technology,Technology,Technology,Healthcare,Financial Services,Manufacturing,Retail,Education,Media,Energy,Government,Logistics,Real Estate,Professional Services'.split(',')
TEAMS = 'Solo,Solo,Small,Small,Small,Medium,Medium,Large,Enterprise'.split(',')
DEPLOYMENTS = 'Cloud,Cloud,Cloud,Cloud,Hybrid,On-Premise'.split(',')
ACQUISITIONS = 'Organic,Organic,Organic,Referral,Referral,Partner,Paid,Paid,Direct'.split(',')

def stable(value: int) -> int:
    return value * 1103515245 + 12345 & 2147483647

def write(name: str, columns: dict[str, list], rows: int, parts: list[dict]) -> None:
    path = ROOT / name / 'part-00000.parquet'
    path.parent.mkdir(parents=True, exist_ok=True)
    table = pa.table(columns)
    pq.write_table(table, path, compression='zstd')
    types = {field.name: str(field.type).replace('int64', 'Int64').replace('double', 'Float64').replace('string', 'Utf8') for field in table.schema}
    parts.append({'schema': 'kaveon_product', 'name': name, 'location': str(path.relative_to(ROOT.parent)).replace('\\', '/'), 'row_count': rows, 'columns': [{'name': k, 'data_type': v, 'nullable': True} for k, v in types.items()]})

def main() -> None:
    parts = []
    write('dim_geography', {'locale': [x[0] for x in GEOS], 'country': [x[1] for x in GEOS], 'region': [x[2] for x in GEOS]}, len(GEOS), parts)
    write('dim_platform', {'platform_key': [x[0] for x in PLATFORMS], 'platform': [x[1] for x in PLATFORMS], 'os': [x[2] for x in PLATFORMS]}, len(PLATFORMS), parts)
    users = {'user_id': [], 'locale': [], 'platform_key': [], 'license': [], 'audience': [], 'segment': [], 'industry': [], 'team_size': [], 'deployment': [], 'acquisition_channel': [], 'signup_date': []}
    for user in range(1, USERS + 1):
        n = stable(user)
        users['user_id'].append(user)
        users['locale'].append(LOCALES[n % len(LOCALES)])
        users['platform_key'].append(PLATFORM_POOL[(n >> 3) % len(PLATFORM_POOL)])
        users['license'].append(LICENSES[(n >> 6) % len(LICENSES)])
        users['audience'].append(AUDIENCES[(n >> 9) % len(AUDIENCES)])
        users['segment'].append(SEGMENTS[(n >> 12) % len(SEGMENTS)])
        users['industry'].append(INDUSTRIES[(n >> 15) % len(INDUSTRIES)])
        users['team_size'].append(TEAMS[(n >> 18) % len(TEAMS)])
        users['deployment'].append(DEPLOYMENTS[(n >> 21) % len(DEPLOYMENTS)])
        users['acquisition_channel'].append(ACQUISITIONS[(n >> 24) % len(ACQUISITIONS)])
        users['signup_date'].append(str(date(2024, 1, 1) + timedelta(days=n % 941)))
    write('kaveon_users', users, USERS, parts)
    usage_root = ROOT / 'kaveon_usage_daily'
    usage_root.mkdir(parents=True, exist_ok=True)
    analytics_root = ROOT / 'kaveon_product_analytics'
    analytics_root.mkdir(parents=True, exist_ok=True)
    geography = {locale: (country, region) for locale, country, region in GEOS}
    platform = {key: (name, os_name) for key, name, os_name in PLATFORMS}
    total = 0
    with ExitStack() as writers:
        for day in range(DAYS):
            values = {k: [] for k in ['usage_date', 'user_id', 'queries_run', 'nl_queries', 'sql_lab_runs', 'dashboards_viewed', 'charts_created', 'datasets_accessed', 'exports', 'active_minutes', 'sessions', 'api_calls', 'data_processed_mb', 'errors', 'feedback_positive', 'feedback_negative']}
            for user in range(1, USERS + 1):
                n = stable(user * 239 + day)
                weight = {'Free': 1, 'Standard': 2, 'Professional': 4, 'Enterprise': 8}[users['license'][user - 1]]
                q = n % 5 * weight
                values['usage_date'].append(str(date(2026, 1, 1) + timedelta(days=day)))
                values['user_id'].append(user)
                values['queries_run'].append(q)
                values['nl_queries'].append(q // 2)
                values['sql_lab_runs'].append(q // 3)
                values['dashboards_viewed'].append(n % 8)
                values['charts_created'].append(n % 3)
                values['datasets_accessed'].append(1 + n % 4)
                values['exports'].append(n % 3)
                values['active_minutes'].append(float(5 + n % 45))
                values['sessions'].append(1 + n % 5)
                values['api_calls'].append(n % 20)
                values['data_processed_mb'].append(float(n % 500) / 10)
                values['errors'].append(1 if n % 20 == 0 else 0)
                values['feedback_positive'].append(1 if n % 7 == 0 else 0)
                values['feedback_negative'].append(1 if n % 33 == 0 else 0)
            usage = pa.table(values)
            if day == 0:
                usage_writer = writers.enter_context(pq.ParquetWriter(usage_root / 'combined-v1.parquet', usage.schema, compression='zstd'))
            usage_writer.write_table(usage)
            analytics = {name: list(column) for name, column in values.items()}
            for name in ('license', 'audience', 'segment', 'industry', 'team_size', 'deployment', 'acquisition_channel', 'locale', 'platform_key'):
                analytics[name] = list(users[name])
            analytics['country'] = [geography[value][0] for value in users['locale']]
            analytics['region'] = [geography[value][1] for value in users['locale']]
            analytics['platform'] = [platform[value][0] for value in users['platform_key']]
            analytics['os'] = [platform[value][1] for value in users['platform_key']]
            analytics_table = pa.table(analytics)
            if day == 0:
                analytics_writer = writers.enter_context(pq.ParquetWriter(analytics_root / 'combined-v1.parquet', analytics_table.schema, compression='zstd'))
            analytics_writer.write_table(analytics_table)
            total += USERS
            if day % 25 == 0 or day == DAYS - 1:
                print(f'Generated {day + 1}/{DAYS} days: {total} synthetic rows per fact table', flush=True)
    schema = pa.table(values).schema
    parts.append({'schema': 'kaveon_product', 'name': 'kaveon_usage_daily', 'location': 'kaveon_product/kaveon_usage_daily/combined-v1.parquet', 'row_count': total, 'columns': [{'name': f.name, 'data_type': 'Float64' if pa.types.is_floating(f.type) else 'Int64' if pa.types.is_integer(f.type) else 'Utf8', 'nullable': True} for f in schema]})
    analytics_schema = pa.table(analytics).schema
    parts.append({'schema': 'kaveon_product', 'name': 'kaveon_product_analytics', 'location': 'kaveon_product/kaveon_product_analytics/combined-v1.parquet', 'row_count': total, 'columns': [{'name': f.name, 'data_type': 'Float64' if pa.types.is_floating(f.type) else 'Int64' if pa.types.is_integer(f.type) else 'Utf8', 'nullable': True} for f in analytics_schema]})
    (ROOT.parent / 'kaveon-product-singlefile-manifest.json').write_text(json.dumps({'synthetic': True, 'disclaimer': 'Deterministic demo data; not production telemetry.', 'seed': 20260909, 'tables': parts}, indent=2), encoding='utf-8')
if __name__ == '__main__':
    main()
