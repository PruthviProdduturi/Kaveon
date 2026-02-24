/**
 * Setup Route
 *
 * Handles first-run metadata database configuration.  After a user signs in,
 * the frontend calls GET /api/v1/setup/status.  If the metadata DB is not
 * reachable or has not been initialised, a setup wizard is shown that guides
 * the user through providing a Fabric SQL connection and running the LoomX
 * schema.
 *
 * Endpoints:
 *   GET  /api/v1/setup/status      – current configuration state
 *   POST /api/v1/setup/test        – probe an arbitrary endpoint/database
 *   POST /api/v1/setup/initialize  – run schema.sql + persist .env + restart
 */

import { Router, type IRouter } from 'express';
import { asyncHandler } from '../middleware/errorHandler';
import { resolve } from 'path';
import { readFileSync, existsSync, writeFileSync } from 'fs';
import axios from 'axios';

const router: IRouter = Router();

const PROXY_BASE = process.env.PYTHON_PROXY_URL || 'http://localhost:5001';
const ENV_PATH   = resolve(__dirname, '../../../../.env');
const SCHEMA_PATH = resolve(__dirname, '../../schema.sql');

// ─── Types ───────────────────────────────────────────────────────────────────

type SetupStatus =
  | 'ok'
  | 'not_configured'
  | 'connection_failed'
  | 'access_denied'
  | 'db_not_found'
  | 'schema_missing';

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

// ─── Helpers ─────────────────────────────────────────────────────────────────

/**
 * Forward a connection probe request to the Python proxy.
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
    // If the proxy returned a structured error body, use it.
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
    // Quote values that contain spaces or comment characters.
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
 * Response shape: { status, endpoint?, database?, message? }
 */
router.get('/status', asyncHandler(async (req, res) => {
  const endpoint = process.env.FABRIC_METADATA_ENDPOINT;
  const database = process.env.FABRIC_METADATA_DATABASE;

  if (!endpoint || !database) {
    res.json({ status: 'not_configured' as SetupStatus });
    return;
  }

  // 1. Check that we can actually connect.
  const connProbe = await probe(endpoint, database);
  if (!connProbe.success) {
    res.json({
      status: (connProbe.error_type || 'connection_failed') as SetupStatus,
      message: connProbe.message,
      endpoint,
      database,
    });
    return;
  }

  // 2. Check whether the LoomX schema already exists.
  const schemaProbe = await probe(endpoint, database, [
    `SELECT COUNT(*) AS cnt FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'datasets'`,
  ]);

  if (schemaProbe.success) {
    const cnt = schemaProbe.results?.[0]?.rows?.[0]?.[0];
    const hasSchema = cnt !== undefined && Number(cnt) > 0;
    if (!hasSchema) {
      res.json({ status: 'schema_missing' as SetupStatus, endpoint, database });
      return;
    }
  }

  res.json({ status: 'ok' as SetupStatus });
}));

/**
 * POST /api/v1/setup/test
 *
 * Test an arbitrary Fabric SQL endpoint + database combination.
 * Body: { endpoint: string, database: string }
 */
router.post('/test', asyncHandler(async (req, res) => {
  const { endpoint, database } = req.body as { endpoint?: string; database?: string };

  if (!endpoint || !database) {
    res.status(400).json({ error: 'endpoint and database are required' });
    return;
  }

  const result = await probe(endpoint, database);
  res.json(result);
}));

/**
 * POST /api/v1/setup/initialize
 *
 * 1. Runs schema.sql against the provided (or already-configured) database.
 * 2. Persists the connection details to .env.
 * 3. Updates process.env in-memory.
 * 4. Responds to the client, then restarts the API process so that all
 *    services pick up the new configuration.
 *
 * Body: { endpoint: string, database: string }
 */
router.post('/initialize', asyncHandler(async (req, res) => {
  const { endpoint, database } = req.body as { endpoint?: string; database?: string };

  if (!endpoint || !database) {
    res.status(400).json({ error: 'endpoint and database are required' });
    return;
  }

  if (!existsSync(SCHEMA_PATH)) {
    res.status(500).json({ error: 'schema.sql not found on the server' });
    return;
  }

  // Split schema.sql into batches (delimited by GO on its own line).
  const schemaSql = readFileSync(SCHEMA_PATH, 'utf-8');
  const batches = schemaSql
    .split(/^\s*GO\s*$/im)
    .map((b) => b.trim())
    .filter((b) => b.length > 0);

  // Execute all batches via the proxy probe endpoint.
  const initResult = await probe(endpoint, database, batches);
  if (!initResult.success) {
    res.json({
      success: false,
      error_type: initResult.error_type,
      message: initResult.message,
    });
    return;
  }

  // Persist connection to .env so the next start-up is automatic.
  try {
    upsertEnvFile({
      FABRIC_METADATA_ENDPOINT: endpoint,
      FABRIC_METADATA_DATABASE: database,
    });
  } catch (err) {
    // Not fatal — the session will work even if the write fails.
    console.error('[Setup] Failed to update .env:', err);
  }

  // Update in-memory env so health checks succeed immediately.
  process.env.FABRIC_METADATA_ENDPOINT = endpoint;
  process.env.FABRIC_METADATA_DATABASE = database;

  // Tell the client we're done before restarting.
  res.json({
    success: true,
    message: 'Metadata database initialised successfully. LoomX API is restarting…',
  });

  // Restart the API process.  ts-node-dev (--respawn) / nodemon will bring it
  // back up automatically so the new .env values are fully applied.
  setTimeout(() => {
    console.log('[Setup] Restarting API to apply new metadata database configuration…');
    process.exit(0);
  }, 600);
}));

export default router;
