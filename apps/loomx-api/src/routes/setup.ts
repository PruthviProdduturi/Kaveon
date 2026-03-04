/**
 * Setup Route
 *
 * Handles first-run metadata database configuration.  After a user signs in,
 * the frontend calls GET /api/v1/setup/status.  If the metadata DB is not
 * reachable or has not been initialised, a setup wizard is shown that guides
 * the user through providing a Fabric SQL connection and running the LoomX
 * schema.
 *
 * Modelled on Apache Superset's database connection API (SIP-40 error format,
 * separate test_connection endpoint, standardised issue codes).
 *
 * Endpoints:
 *   GET  /api/v1/setup/status      – current configuration state
 *   POST /api/v1/setup/test        – probe an arbitrary endpoint/database
 *                                    200 on success, 400 on connection failure
 *   POST /api/v1/setup/initialize  – run schema.sql + persist .env + restart
 */

import { Router, type IRouter } from 'express';
import { asyncHandler } from '../middleware/errorHandler';
import { resolve } from 'path';
import { readFileSync, existsSync, writeFileSync } from 'fs';
import axios from 'axios';

const router: IRouter = Router();

const PROXY_BASE   = process.env.PYTHON_PROXY_URL || 'http://localhost:5001';
const ENV_PATH     = resolve(__dirname, '../../../../.env');
const SCHEMA_PATH  = resolve(__dirname, '../../schema.sql');

// ─── Types ───────────────────────────────────────────────────────────────────

type SetupStatus =
  | 'ok'
  | 'not_configured'
  | 'connection_failed'
  | 'access_denied'
  | 'db_not_found'
  | 'schema_missing';

/**
 * Superset SIP-40-style issue code.
 * Carries a numeric code plus a short human-readable explanation.
 */
interface IssueCode {
  code: number;
  message: string;
}

/**
 * Single structured error, matching Superset's error shape.
 * level: 'error' | 'warning' | 'info'
 */
interface SetupError {
  message: string;
  error_type: string;
  level: 'error' | 'warning' | 'info';
  extra?: {
    issue_codes: IssueCode[];
  };
}

interface ProbeResult {
  success: boolean;
  error_type?: string;
  message?: string;
  results?: Array<{
    success: boolean;
    row_count: number;
    rows: any[][];
    columns: string[];
  }>;
}

// ─── Issue code registry (mirrors Superset's numbering where applicable) ─────

const ISSUE_CODES: Record<string, IssueCode[]> = {
  connection_failed: [
    { code: 1007, message: 'The hostname provided cannot be resolved — check for typos in the endpoint.' },
    { code: 1008, message: 'Port 1433 (required by Fabric SQL) may be blocked by a firewall or VPN.' },
  ],
  timeout: [
    { code: 1008, message: 'Connection timed out — port 1433 is unreachable from this machine.' },
  ],
  access_denied: [
    { code: 1017, message: 'The service identity lacks permission on this database. Grant the Contributor or Member role in the Fabric workspace.' },
  ],
  db_not_found: [
    { code: 1015, message: 'The database was not found. Database names are case-sensitive in Fabric SQL.' },
  ],
};

function toSetupErrors(error_type: string, raw_message: string): SetupError[] {
  return [{
    message: raw_message,
    error_type,
    level: 'error',
    extra: {
      issue_codes: ISSUE_CODES[error_type] ?? [
        { code: 0, message: 'An unexpected error occurred. See the raw message for details.' },
      ],
    },
  }];
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/**
 * Forward a connection probe to the Python proxy.
 * Always resolves — network failures become error_type='connection_failed'.
 */
async function probe(
  endpoint: string,
  database: string,
  statements: string[] = ['SELECT 1 AS test'],
): Promise<ProbeResult> {
  try {
    const res = await axios.post<ProbeResult>(
      `${PROXY_BASE}/api/v1/probe`,
      { endpoint, database, statements },
      { timeout: 90_000 },
    );
    return res.data;
  } catch (err: any) {
    if (err.response?.data) {
      return err.response.data as ProbeResult;
    }
    return {
      success: false,
      error_type: 'connection_failed',
      message: err instanceof Error ? err.message : 'Unable to reach Python proxy',
    };
  }
}

/**
 * Update (or append) key=value pairs in the root .env file.
 * Existing lines for the same key are replaced in-place; new keys are appended.
 */
function upsertEnvFile(updates: Record<string, string>): void {
  let content = existsSync(ENV_PATH) ? readFileSync(ENV_PATH, 'utf-8') : '';

  for (const [key, value] of Object.entries(updates)) {
    const safe = /[\s#"']/.test(value) ? `"${value.replace(/"/g, '\\"')}"` : value;
    const line = `${key}=${safe}`;
    const re   = new RegExp(`^(\\s*#?\\s*)${key}\\s*=.*$`, 'm');

    if (re.test(content)) {
      content = content.replace(re, line);
    } else {
      content = content.trimEnd() + '\n' + line + '\n';
    }
  }

  writeFileSync(ENV_PATH, content, 'utf-8');
}

// ─── Routes ──────────────────────────────────────────────────────────────────

/**
 * GET /api/v1/setup/status
 *
 * Returns the current metadata database configuration state.
 * Sends connection test AND schema check in a single probe call to minimise
 * round-trip latency (mirrors Superset's single test_connection call).
 *
 * Response: { status, endpoint?, database?, errors? }
 */
router.get('/status', asyncHandler(async (req, res) => {
  const endpoint = process.env.FABRIC_METADATA_ENDPOINT;
  const database = process.env.FABRIC_METADATA_DATABASE;

  if (!endpoint || !database) {
    res.json({ status: 'not_configured' as SetupStatus });
    return;
  }

  // Single probe call: first statement tests connectivity, second checks schema.
  // If the first fails, error_type is set and we classify it immediately.
  // If both succeed, inspect row[0][0] of results[1] to decide schema state.
  const p = await probe(endpoint, database, [
    'SELECT 1 AS connection_test',
    `SELECT COUNT(*) AS cnt FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'datasets'`,
  ]);

  if (!p.success) {
    const errType = p.error_type || 'connection_failed';
    res.json({
      status: errType as SetupStatus,
      endpoint,
      database,
      errors: toSetupErrors(errType, p.message ?? 'Connection failed'),
    });
    return;
  }

  // results[1] holds the schema-check query; fall back to schema_missing if absent.
  const cnt = p.results?.[1]?.rows?.[0]?.[0];
  if (cnt === undefined || Number(cnt) === 0) {
    res.json({ status: 'schema_missing' as SetupStatus, endpoint, database });
    return;
  }

  res.json({ status: 'ok' as SetupStatus });
}));

/**
 * POST /api/v1/setup/test
 *
 * Test an arbitrary Fabric SQL endpoint + database combination.
 * Returns 200 on success, 400 on connection failure — matching Superset's
 * POST /api/v1/databases/test_connection/ behaviour.
 *
 * Body:     { endpoint: string, database: string }
 * Success:  200 { success: true }
 * Failure:  400 { success: false, errors: SetupError[] }
 */
router.post('/test', asyncHandler(async (req, res) => {
  const { endpoint, database } = req.body as { endpoint?: string; database?: string };

  if (!endpoint || !database) {
    res.status(400).json({
      success: false,
      errors: [{ message: 'endpoint and database are required', error_type: 'invalid_request', level: 'error' }],
    });
    return;
  }

  const result = await probe(endpoint, database);

  if (result.success) {
    res.status(200).json({ success: true });
  } else {
    const errType = result.error_type || 'connection_failed';
    res.status(400).json({
      success: false,
      errors: toSetupErrors(errType, result.message ?? 'Connection failed'),
    });
  }
}));

/**
 * POST /api/v1/setup/initialize
 *
 * 1. Runs schema.sql against the provided database (idempotent — all DDL
 *    statements are guarded by IF OBJECT_ID(...) IS NULL).
 * 2. Persists the connection details to .env.
 * 3. Updates process.env in-memory.
 * 4. Responds to the client, then restarts the API process so all services
 *    pick up the new configuration (ts-node-dev/nodemon respawn).
 *
 * Body:    { endpoint: string, database: string }
 * Success: 200 { success: true, message: string }
 * Failure: 400 { success: false, errors: SetupError[] }
 */
router.post('/initialize', asyncHandler(async (req, res) => {
  const { endpoint, database } = req.body as { endpoint?: string; database?: string };

  if (!endpoint || !database) {
    res.status(400).json({
      success: false,
      errors: [{ message: 'endpoint and database are required', error_type: 'invalid_request', level: 'error' }],
    });
    return;
  }

  if (!existsSync(SCHEMA_PATH)) {
    res.status(500).json({ success: false, errors: [{ message: 'schema.sql not found on the server', error_type: 'server_error', level: 'error' }] });
    return;
  }

  // Split schema.sql into batches (delimited by GO on its own line).
  // All CREATE TABLE statements use IF OBJECT_ID() IS NULL — safe to re-run.
  const schemaSql = readFileSync(SCHEMA_PATH, 'utf-8');
  const batches = schemaSql
    .split(/^\s*GO\s*$/im)
    .map((b) => b.trim())
    .filter((b) => b.length > 0);

  const initResult = await probe(endpoint, database, batches);

  if (!initResult.success) {
    const errType = initResult.error_type || 'connection_failed';
    res.status(400).json({
      success: false,
      errors: toSetupErrors(errType, initResult.message ?? 'Schema initialisation failed'),
    });
    return;
  }

  // Persist to .env (best-effort — not fatal if write fails).
  try {
    upsertEnvFile({
      FABRIC_METADATA_ENDPOINT: endpoint,
      FABRIC_METADATA_DATABASE: database,
    });
  } catch (err) {
    console.error('[Setup] Failed to update .env:', err);
  }

  // Apply in-memory immediately so health checks succeed before restart.
  process.env.FABRIC_METADATA_ENDPOINT = endpoint;
  process.env.FABRIC_METADATA_DATABASE = database;

  res.json({
    success: true,
    message: 'Metadata database initialised successfully. LoomX API is restarting…',
  });

  // Restart proxy so it re-reads the new .env (best-effort — non-fatal if proxy is down).
  try {
    await axios.post(`${PROXY_BASE}/api/v1/shutdown`, {}, { timeout: 3000 });
  } catch {}

  // ts-node-dev (--respawn) / nodemon brings the process back up automatically.
  setTimeout(() => {
    console.log('[Setup] Restarting API to apply new metadata database configuration…');
    process.exit(0);
  }, 600);
}));

export default router;
