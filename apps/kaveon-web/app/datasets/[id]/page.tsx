"use client";

import React, { useEffect, useRef, useState } from "react";

import { API_BASE } from "../../../config";
import { LoadingOverlay } from "../../../components/LoadingOverlay";
import { msalFetch } from "../../../utils/msalFetch";
import { DatasetContextPanel } from "../../../components/DatasetContextPanel";
import { useAuth } from "../../../auth/useAuth";
import { useRouter, useParams } from "next/navigation";
import { useRecents } from "../../../hooks/useRecents";

export const dynamic = 'force-dynamic';
export const dynamicParams = true;

interface DatasetDimension {
  dimension_table: string;
  join_condition: string;
  display_name?: string | null;
}

interface DatasetColumn {
  table_name: string;
  column_name: string;
  data_type: string;
  is_dimension: boolean;
  is_metric: boolean;
  semantic_type?: string | null;
}

interface DatasetMetric {
  name: string;
  expression: string;
  metric_type: string;
  format?: string | null;
}

interface DatasetFilter {
  column: string;
  op: string;
  value: string | string[];
}

interface DatasetDetail {
  id: number;
  name: string;
  description?: string | null;
  table_name: string;
  schema_name?: string | null;
  database_name?: string | null;
  favorite?: boolean;
  date_column?: string | null;
  sql_text?: string | null;
  dimensions?: DatasetDimension[];
  columns?: DatasetColumn[];
  metrics?: DatasetMetric[];
  filters?: DatasetFilter[];
}

function quoteIdentifier(name: string): string {
  if (!name) return "";
  const safe = name.replace(/]/g, "]]" );
  return `[${safe}]`;
}

function parseJoinCondition(joinCondition: string): {
  factSchema?: string;
  factTable?: string;
  factKey?: string;
  dimSchema?: string;
  dimTable?: string;
  dimKey?: string;
} {
  if (!joinCondition) return {};
  const regex =
    /\[(?<factSchema>[^\]]+)\]\.\[(?<factTable>[^\]]+)\]\.\[(?<factKey>[^\]]+)\]\s*=\s*\[(?<dimSchema>[^\]]+)\]\.\[(?<dimTable>[^\]]+)\]\.\[(?<dimKey>[^\]]+)\]/;
  const match = joinCondition.match(regex) as
    | (RegExpMatchArray & { groups?: { [key: string]: string } })
    | null;
  if (!match || !match.groups) return {};
  return {
    factSchema: match.groups["factSchema"],
    factTable: match.groups["factTable"],
    factKey: match.groups["factKey"],
    dimSchema: match.groups["dimSchema"],
    dimTable: match.groups["dimTable"],
    dimKey: match.groups["dimKey"],
  };
}

function deriveDimensionAlias(factKey?: string | null, fallback?: string | null): string | null {
  let base = (factKey || "").trim();
  if (!base && fallback) {
    base = fallback.trim().replace(/\s+/g, "");
  }
  if (!base) return null;
  const lower = base.toLowerCase();
  if (lower.endsWith("key")) {
    base = base.slice(0, -3);
  } else if (lower.endsWith("id")) {
    base = base.slice(0, -2);
  }
  return base || null;
}

function buildDatasetPreviewSql(dataset: DatasetDetail, rowLimit: number = 100): string {
  const schema = dataset.schema_name || "dbo";
  const table = dataset.table_name;
  const top = Math.max(1, Math.min(rowLimit, 1000));
  // PostgreSQL uses "schema"."table" LIMIT N; SQL Server uses [schema].[table] TOP N
  const isPg = schema === "public" || schema === "climate_energy" || (dataset.database_name || "").includes("postgres") || (dataset.database_name || "") === "kaveon";

  const filters = dataset.filters || [];
  const whereClauses: string[] = [];

  // OPTIMIZATION: Build WHERE clause using fact table keys when possible
  for (const f of filters) {
    const column = (f.column || "").trim();
    if (!column) continue;
    const op = (f.op || "=").trim().toUpperCase();
    const val = f.value;

    // Check if filter has optimized key column (tier 1/2 filtering)
    const keyColumn = (f as any).keyColumn;
    const valueKey = (f as any).valueKey;

    if (keyColumn && (valueKey !== undefined && valueKey !== null && valueKey !== '')) {
      // Use optimized fact table key filter (no dimension JOIN needed)
      const keyColName = keyColumn.split('.').pop() || keyColumn;
      console.log(`[Dataset Preview] Using optimized filter on fact key: ${keyColName}`);

      if (Array.isArray(valueKey)) {
        if (valueKey.length === 0) continue;
        const safeKeys = valueKey.map((k) => `'${String(k).replace(/'/g, "''")}'`);
        whereClauses.push(`${quoteIdentifier(keyColName)} IN (${safeKeys.join(", ")})`);
      } else {
        const keyStr = `'${String(valueKey).replace(/'/g, "''")}'`;
        whereClauses.push(`${quoteIdentifier(keyColName)} ${op} ${keyStr}`);
      }
    } else {
      // Use standard filter on display column (may need dimension JOIN)
      if (Array.isArray(val)) {
        if (val.length === 0) continue;
        const safeVals = val.map((v) => `'${String(v).replace(/'/g, "''")}'`);
        whereClauses.push(`${quoteIdentifier(column)} ${op.includes("IN") ? op : "IN"} (${safeVals.join(", ")})`);
      } else {
        const valueStr = `'${String(val).replace(/'/g, "''")}'`;
        whereClauses.push(`${quoteIdentifier(column)} ${op} ${valueStr}`);
      }
    }
  }

  // For dataset preview, show ALL columns (dimensions + metrics + time)
  // This gives users the full picture of what their dataset looks like
  let modelColumns = (dataset.columns || []).slice();

  // Ensure time column is included
  if (dataset.date_column) {
    const hasTimeColumn = modelColumns.some(
      (c) =>
        (c.semantic_type && c.semantic_type.toLowerCase() === "time") ||
        c.column_name.toLowerCase() === dataset.date_column!.toLowerCase(),
    );
    if (!hasTimeColumn) {
      modelColumns.push({
        table_name: `${schema}.${table}`,
        column_name: dataset.date_column,
        data_type: "datetime",
        is_dimension: false,
        is_metric: false,
        semantic_type: "time",
      });
    }
  }

  console.log(`[Dataset Preview] Showing all ${modelColumns.length} columns from schema`);

  // Build set of dimension tables needed for these columns
  // OPTIMIZATION: Only JOIN dimensions that have columns being displayed
  const dimsToJoin = new Set<string>();
  modelColumns.forEach((c) => {
    if (c.is_dimension && c.table_name) {
      console.log(`[Dataset Preview] Adding dimension to JOIN set: ${c.table_name} (for column ${c.column_name}, semantic: ${c.semantic_type || 'none'})`);
      dimsToJoin.add(c.table_name);
    }
  });

  console.log(`[Dataset Preview] Total dimensions to join: ${dimsToJoin.size} out of ${(dataset.dimensions || []).length} available`);

  const joinClauses: string[] = [];
  const dims = dataset.dimensions || [];
  const usedAliases = new Set<string>();
  const dimAliases: Record<string, string> = {};

  // Track dimensions by fact key for COALESCE detection
  const dimensionsByFactKey: Record<string, Array<{dim: any, alias: string, tableName: string, semantic?: string, columnName?: string}>> = {};

  // Map semantic_type to alias for correct alias usage
  const semanticToAlias: Record<string, string> = {};

  let joinedCount = 0;
  let skippedCount = 0;

  for (const dim of dims) {
    // OPTIMIZATION: Only join dimensions needed for preview columns
    if (!dimsToJoin.has(dim.dimension_table)) {
      console.log(`[Dataset Preview] SKIPPING dimension: ${dim.dimension_table} (not needed for displayed columns)`);
      skippedCount++;
      continue;
    }
    console.log(`[Dataset Preview] JOINING dimension: ${dim.dimension_table}`);
    const parsed = parseJoinCondition(dim.join_condition || "");

    const dimSchemaRaw = parsed.dimSchema || dim.dimension_table.split(".", 2)[0];
    const dimTableRaw = parsed.dimTable || dim.dimension_table.split(".", 2)[1];
    const dimSchema = dimSchemaRaw || "dbo";
    const dimTable = dimTableRaw || dim.dimension_table;
    const dimRef = `${quoteIdentifier(dimSchema)}.${quoteIdentifier(dimTable)}`;

    const aliasBase = deriveDimensionAlias(parsed.factKey, dim.display_name || null);
    let alias: string | null = null;
    if (aliasBase) {
      let candidate = aliasBase;
      let i = 2;
      while (usedAliases.has(candidate.toLowerCase())) {
        candidate = `${aliasBase}_${i++}`;
      }
      usedAliases.add(candidate.toLowerCase());
      alias = candidate;
      dimAliases[dim.dimension_table] = alias;

      // Find the column and semantic type for this dimension
      const dimColumn = (dataset.columns || []).find(c =>
        c.table_name === dim.dimension_table &&
        c.is_dimension === true &&
        c.semantic_type?.toLowerCase() === aliasBase.toLowerCase()
      );

      if (dimColumn?.semantic_type) {
        semanticToAlias[dimColumn.semantic_type.toLowerCase()] = alias;
        console.log(`[Dataset Preview] Mapped semantic "${dimColumn.semantic_type}" → alias "${alias}"`);
      }
    }

    if (parsed.factSchema && parsed.factTable && parsed.factKey && parsed.dimKey && alias) {
      const factRef = `${quoteIdentifier(parsed.factSchema)}.${quoteIdentifier(parsed.factTable)}`;
      const aliasIdent = quoteIdentifier(alias);
      const onExpr = `${factRef}.${quoteIdentifier(parsed.factKey)} = ${aliasIdent}.${quoteIdentifier(parsed.dimKey)}`;
      joinClauses.push(`LEFT JOIN ${dimRef} AS ${aliasIdent} ON ${onExpr}`);
      joinedCount++;

      // Track this dimension by its fact key
      const factKey = parsed.factKey.toLowerCase();
      if (!dimensionsByFactKey[factKey]) {
        dimensionsByFactKey[factKey] = [];
      }

      // Find the column info for this dimension (including semantic type and column name)
      const dimColumn = (dataset.columns || []).find(c =>
        c.table_name === dim.dimension_table &&
        c.is_dimension === true &&
        c.semantic_type?.toLowerCase() === aliasBase?.toLowerCase()
      );

      dimensionsByFactKey[factKey].push({
        dim,
        alias,
        tableName: dim.dimension_table,
        semantic: dimColumn?.semantic_type || undefined,
        columnName: dimColumn?.column_name || undefined
      });
    } else {
      joinClauses.push(`LEFT JOIN ${dimRef} ON ${dim.join_condition}`);
      joinedCount++;
    }
  }

  console.log(`[Dataset Preview] Using ${joinedCount} of ${dims.length} dimensions (skipped ${skippedCount})`);
  if (skippedCount > 0) {
    console.log(`[Dataset Preview] Performance: ${Math.round((skippedCount / dims.length) * 100)}% reduction in JOINs`);
  }

  // Log fact key groupings for debugging
  console.log(`[Dataset Preview] Dimensions grouped by fact key:`,
    Object.entries(dimensionsByFactKey).map(([factKey, dimGroup]) => ({
      factKey,
      count: dimGroup.length,
      dimensions: dimGroup.map(d => ({ table: d.tableName, alias: d.alias }))
    }))
  );

  // Build map of semantic types to their source expressions (for COALESCE)
  // Format: { semantic: [{ alias, columnName }, ...] }
  const semanticToSources: Record<string, Array<{ alias: string; columnName: string }>> = {};
  Object.entries(dimensionsByFactKey).forEach(([factKey, dimGroup]) => {
    if (dimGroup.length > 1) {
      // Multiple dimensions on same fact key - need COALESCE
      console.log(`[Dataset Preview] Found ${dimGroup.length} dimensions sharing fact key "${factKey}" - checking for COALESCE candidates`);

      dimGroup.forEach(dimInfo => {
        if (dimInfo.semantic && dimInfo.columnName) {
          const semantic = dimInfo.semantic.toLowerCase();
          if (!semanticToSources[semantic]) {
            semanticToSources[semantic] = [];
          }
          semanticToSources[semantic].push({
            alias: dimInfo.alias,
            columnName: dimInfo.columnName
          });
          console.log(`[Dataset Preview] Added source for semantic "${semantic}": alias="${dimInfo.alias}", column="${dimInfo.columnName}"`);
        }
      });
    } else {
      console.log(`[Dataset Preview] Fact key "${factKey}" has only 1 dimension - no COALESCE needed`);
    }
  });

  console.log(`[Dataset Preview] Final COALESCE map:`, Object.entries(semanticToSources).map(([sem, sources]) => ({
    semantic: sem,
    sources: sources.map(s => `${s.alias}.${s.columnName}`)
  })));

  const factRef = isPg ? `${schema}.${table}` : `${quoteIdentifier(schema)}.${quoteIdentifier(table)}`;

  let selectClause: string;
  if (modelColumns.length > 0) {
    const selectExprs: string[] = [];

    // Helper to resolve the correct source alias/table for a column
    const getSourceForColumn = (col: DatasetColumn): string => {
      // First, try to match by semantic type (for dimensions)
      if (col.semantic_type) {
        const semantic = col.semantic_type.toLowerCase();
        const alias = semanticToAlias[semantic];
        if (alias) {
          console.log(`[Dataset Preview] Column "${col.column_name}" (semantic: "${col.semantic_type}") → alias "${alias}"`);
          return quoteIdentifier(alias);
        }
      }

      // Fall back to table name matching
      const tableName = col.table_name || `${schema}.${table}`;
      const [tblSchemaRaw, tblNameRaw] = tableName.split(".", 2);
      const tblSchema = tblSchemaRaw || schema;
      const tblName = tblNameRaw || table;

      const dimAlias = dimAliases[tableName];
      if (dimAlias) {
        return quoteIdentifier(dimAlias);
      }

      if (
        tblSchema.toLowerCase() === schema.toLowerCase() &&
        tblName.toLowerCase() === table.toLowerCase()
      ) {
        return factRef;
      }

      return `${quoteIdentifier(tblSchema)}.${quoteIdentifier(tblName)}`;
    };

    // Build SELECT clause for all schema columns
    const usedDisplayNames = new Set<string>();
    const processedSemantics = new Set<string>(); // Track semantics that have been processed for COALESCE

    for (const col of modelColumns) {
      let displayName =
        col.semantic_type && col.semantic_type.toLowerCase() !== "time"
          ? col.semantic_type
          : col.column_name;

      const semantic = (col.semantic_type || '').toLowerCase();

      // Check if this column needs COALESCE (multiple dimensions on same fact key)
      if (semantic && semantic !== 'time' && semanticToSources[semantic]?.length > 1) {
        // Skip if we've already processed this semantic type
        if (processedSemantics.has(semantic)) {
          console.log(`[Dataset Preview] ⏭ Skipping duplicate semantic "${semantic}" (already has COALESCE)`);
          continue;
        }
        processedSemantics.add(semantic);

        // Build COALESCE expression using correct column name from each dimension
        const sources = semanticToSources[semantic];
        const coalesceParts = sources.map(s => `${quoteIdentifier(s.alias)}.${quoteIdentifier(s.columnName)}`);
        const coalesceExpr = `COALESCE(${coalesceParts.join(', ')}) AS ${quoteIdentifier(displayName)}`;
        selectExprs.push(coalesceExpr);
        console.log(`[Dataset Preview] ✓ COALESCE APPLIED for "${displayName}" (semantic: "${semantic}"):`, coalesceExpr);
        usedDisplayNames.add(displayName.toLowerCase());
      } else {
        // Standard column reference
        // If display name is already used, use the column_name instead to ensure uniqueness
        if (usedDisplayNames.has(displayName.toLowerCase())) {
          displayName = col.column_name;
        }
        usedDisplayNames.add(displayName.toLowerCase());

        const source = getSourceForColumn(col);
        const standardExpr = `${source}.${quoteIdentifier(col.column_name)} AS ${quoteIdentifier(displayName)}`;
        selectExprs.push(standardExpr);
        if (semantic && semantic !== 'time') {
          console.log(`[Dataset Preview] ✗ No COALESCE for "${displayName}" (semantic: "${semantic}"): only ${semanticToSources[semantic]?.length || 1} dimension(s)`);
        }
      }
    }

    selectClause = selectExprs.join(", ");
  } else {
    selectClause = "*";
  }

  let base = isPg
    ? `SELECT ${selectClause} FROM ${factRef}`
    : `SELECT TOP ${top} ${selectClause} FROM ${factRef}`;
  if (joinClauses.length > 0) {
    base = `${base} ${joinClauses.join(" ")}`;
  }
  if (whereClauses.length > 0) {
    base = `${base} WHERE ${whereClauses.join(" AND ")}`;
  }
  if (isPg) {
    base = `${base} LIMIT ${top}`;
  }
  console.log(`[Dataset Preview] Final query built (${joinedCount} JOINs, ${Object.keys(semanticToSources).length} COALESCE groups)`);
  return base;
}

export default function DatasetDetailPage() {
  const router = useRouter();
  const params = useParams();
  const { isAuthenticated, account } = useAuth();

  const [dataset, setDataset] = useState<DatasetDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isTogglingFavorite, setIsTogglingFavorite] = useState(false);
  const [isEditingName, setIsEditingName] = useState(false);
  const [editNameValue, setEditNameValue] = useState("");
  const nameInputRef = useRef<HTMLInputElement>(null);
  const [accessToken] = useState<string>("proxy");
  const { addRecent } = useRecents();
  const [previewColumns, setPreviewColumns] = useState<string[]>([]);
  const [previewRows, setPreviewRows] = useState<any[][]>([]);
  const [isLoadingPreview, setIsLoadingPreview] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewSortColumnIndex, setPreviewSortColumnIndex] = useState<number | null>(null);
  const [previewSortDirection, setPreviewSortDirection] = useState<"asc" | "desc">("asc");
  const [buildingContext, setBuildingContext] = useState(false);
  const [contextStatus, setContextStatus] = useState<{ ok: boolean; message: string } | null>(null);

  // For the Schema card we want to show ALL columns, even when
  // multiple fact columns map to the same dimension table.
  // Each column represents a distinct business concept and should be visible.
  const schemaColumns: DatasetColumn[] = React.useMemo(() => {
    if (!dataset?.columns) return [];

    // Deduplicate by display label so that two dimension tables sharing the
    // same fact key (e.g. IDEASProduct + IDEASThirdPartyApps both on ProductKey)
    // only appear once under "Product". Columns with distinct labels
    // (e.g. IsMSFTTenant vs IsStrategicCustomer) are kept separately.
    const seen = new Set<string>();
    return dataset.columns.filter((col) => {
      const label =
        col.semantic_type && col.semantic_type.toLowerCase() !== "time"
          ? col.semantic_type.toLowerCase()
          : col.column_name.toLowerCase();
      if (seen.has(label)) return false;
      seen.add(label);
      return true;
    });
  }, [dataset]);

  const datasetId = params?.id as string | undefined;

  const startEditingName = () => {
    setEditNameValue(dataset?.name ?? "");
    setIsEditingName(true);
    setTimeout(() => nameInputRef.current?.select(), 0);
  };

  const commitRename = async () => {
    setIsEditingName(false);
    const trimmed = editNameValue.trim();
    if (!trimmed || trimmed === dataset?.name || !datasetId) return;
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/datasets/${datasetId}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: trimmed }),
      });
      if (res.ok) setDataset(prev => prev ? { ...prev, name: trimmed } : prev);
    } catch { /* silent — name reverts on next load */ }
  };

  // Token no longer needed — NextAuth proxy handles auth via session cookie.

  useEffect(() => {
    if (!isAuthenticated || !datasetId || !accessToken) return;

    const load = async () => {
      setIsLoading(true);
      setError(null);
      try {
        const userEmail = account?.email || account?.username || null;
        const headers: Record<string, string> = {};
        if (userEmail) {
          headers['x-user-email'] = userEmail;
        }

        const res = await msalFetch(`${API_BASE}/api/v1/datasets/${datasetId}`, {
          headers,
        });
        if (!res.ok) {
          throw new Error(`Failed to load dataset: ${res.status}`);
        }
        const data: any = await res.json();

        // Log to debug favorite field
        console.log('[Dataset Detail] Loaded dataset:', {
          id: data.id,
          name: data.name,
          favorite: data.favorite,
          is_favorite: data.is_favorite,
          isFavorite: data.isFavorite
        });

        // Map is_favorite to favorite if needed
        const mappedData: DatasetDetail = {
          ...data,
          favorite: data.favorite ?? data.is_favorite ?? false
        };

        setDataset(mappedData);
        addRecent({ id: String(mappedData.id), label: mappedData.name, href: `/datasets/${datasetId}`, type: "dataset" });
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Unknown error";
        setError(message);
      } finally {
        setIsLoading(false);
      }
    };

    void load();
  }, [isAuthenticated, datasetId, accessToken, account]);

  useEffect(() => {
    if (!isAuthenticated) return;
    if (!dataset) return;

    const runPreview = async () => {
      try {
        setIsLoadingPreview(true);
        setPreviewError(null);

        const userEmail = account?.email || account?.username || null;

        if (dataset.database_name) {
          const resSwitch = await msalFetch(`${API_BASE}/api/v1/lab/switch-database`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ database_name: dataset.database_name }),
          });
          const dataSwitch = await resSwitch.json();
          if (!resSwitch.ok || !dataSwitch.success) {
            throw new Error(dataSwitch.error || "Failed to switch database for preview");
          }
        }

        const qualifiedTable = dataset.schema_name && dataset.schema_name !== "dbo" && dataset.schema_name !== "public"
          ? `${dataset.schema_name}.${dataset.table_name}`
          : dataset.table_name;
        const isPg = dataset.database_name === "kaveon" || dataset.schema_name === "climate_energy" || dataset.schema_name === "public";
        // For PostgreSQL: simple SELECT * LIMIT — bypass the complex dimension JOIN builder
        // which generates SQL Server syntax and breaks on PG
        const sql = dataset.sql_text && !dataset.table_name
          ? `SELECT * FROM (${dataset.sql_text.replace(/;\s*$/, "").trim()}) AS _preview LIMIT 100`
          : isPg
            ? `SELECT * FROM ${qualifiedTable} LIMIT 100`
            : buildDatasetPreviewSql(dataset, 100);

        // Build list of tables used in this query for query history
        const tablesUsed = dataset.sql_text && !dataset.table_name
          ? []
          : [
          `${dataset.schema_name || 'dbo'}.${dataset.table_name}`, // Fact table
          ...(dataset.dimensions || []).map(d => d.dimension_table) // Dimension tables
        ];

        const res = await msalFetch(`${API_BASE}/api/v1/lab/query`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            query: sql,
            datasetId: dataset.id,
            database: dataset.database_name,
            rowLimit: 100,
            runContext: "dataset-detail",
            executedBy: userEmail,
            tablesUsed: tablesUsed,
          }),
        });
        const data = await res.json();
        if (!res.ok || !data.success) {
          throw new Error(data.detail || data.error || `Preview query failed (HTTP ${res.status})`);
        }

        const apiColumns: string[] = Array.isArray(data.columns) ? data.columns : [];
        const apiRows: any[][] = Array.isArray(data.rows) ? data.rows : [];

        let finalColumns = apiColumns;
        let finalRows = apiRows;

        if (dataset.columns && dataset.columns.length > 0 && apiColumns.length > 0) {
          const preferredNames = dataset.columns.map((c) =>
            c.semantic_type && c.semantic_type.toLowerCase() !== "time"
              ? c.semantic_type
              : c.column_name,
          );

          // Ensure we only keep unique preferred names so that when
          // multiple dataset columns share the same semantic label
          // (e.g. two Product columns from different dimension tables
          // that are coalesced in the SQL), we only reference the
          // resulting column once in the preview ordering.
          const preferred = Array.from(
            new Set(preferredNames.filter((name) => apiColumns.includes(name))),
          );

          const remaining = apiColumns.filter((name) => !preferred.includes(name));
          finalColumns = [...preferred, ...remaining];

          const indexMap = finalColumns.map((name) => apiColumns.indexOf(name));
          finalRows = apiRows.map((row) =>
            indexMap.map((idx) => (idx >= 0 && idx < row.length ? row[idx] : null)),
          );
        }

        // Ensure the date/time column appears as the first column in the preview
        if (dataset.date_column && finalColumns.length > 0) {
          const timeDisplayName =
            dataset.columns?.find(
              (c) => (c.semantic_type || "").toLowerCase() === "time",
            )?.column_name || dataset.date_column;

          if (timeDisplayName && finalColumns[0] !== timeDisplayName) {
            const idx = finalColumns.indexOf(timeDisplayName);
            if (idx > -1) {
              const reorderedColumns = [
                timeDisplayName,
                ...finalColumns.filter((_, i) => i !== idx),
              ];
              const reorderedRows = finalRows.map((row) => [
                row[idx],
                ...row.filter((_, i) => i !== idx),
              ]);
              finalColumns = reorderedColumns;
              finalRows = reorderedRows;
            }
          }
        }
        // Reset sort state whenever we load a fresh preview
        setPreviewSortColumnIndex(null);
        setPreviewSortDirection("asc");
        setPreviewColumns(finalColumns);
        setPreviewRows(finalRows);
      } catch (e: unknown) {
        const message = e instanceof Error ? e.message : "Failed to load preview";
        setPreviewError(message);
        setPreviewColumns([]);
        setPreviewRows([]);
      } finally {
        setIsLoadingPreview(false);
      }
    };

    void runPreview();
  }, [isAuthenticated, dataset]);

  const sortedPreviewRows = React.useMemo(() => {
    if (!previewRows || previewRows.length === 0) return previewRows;
    if (previewSortColumnIndex === null) return previewRows;

    const rowsCopy = [...previewRows];
    const colIndex = previewSortColumnIndex;
    const directionMultiplier = previewSortDirection === "asc" ? 1 : -1;

    rowsCopy.sort((a, b) => {
      const aVal = a[colIndex];
      const bVal = b[colIndex];

      const aStr = aVal == null ? "" : String(aVal);
      const bStr = bVal == null ? "" : String(bVal);

      const aNum = parseFloat(aStr);
      const bNum = parseFloat(bStr);

      const aIsNum = !Number.isNaN(aNum) && aStr.trim() !== "";
      const bIsNum = !Number.isNaN(bNum) && bStr.trim() !== "";

      if (aIsNum && bIsNum) {
        if (aNum === bNum) return 0;
        return aNum > bNum ? directionMultiplier : -directionMultiplier;
      }

      if (aStr === bStr) return 0;
      return aStr > bStr ? directionMultiplier : -directionMultiplier;
    });

    return rowsCopy;
  }, [previewRows, previewSortColumnIndex, previewSortDirection]);

  const handlePreviewSort = (columnIndex: number) => {
    // 3-state sort cycle per column: unsorted -> asc -> desc -> unsorted
    if (previewSortColumnIndex === columnIndex) {
      if (previewSortDirection === "asc") {
        setPreviewSortDirection("desc");
      } else {
        // was desc, clear sorting for this column
        setPreviewSortColumnIndex(null);
        setPreviewSortDirection("asc");
      }
    } else {
      setPreviewSortColumnIndex(columnIndex);
      setPreviewSortDirection("asc");
    }
  };

  const handleToggleFavorite = async () => {
    if (!dataset || !datasetId || isTogglingFavorite) return;

    const newFavoriteState = !dataset.favorite;
    setIsTogglingFavorite(true);

    // Optimistically update UI
    setDataset(prev => prev ? { ...prev, favorite: newFavoriteState } : prev);

    try {
      const userEmail = account?.email || account?.username || null;
      const headers: Record<string, string> = {};
      if (userEmail) {
        headers['x-user-email'] = userEmail;
      }

      const res = await msalFetch(
        `${API_BASE}/api/v1/datasets/${datasetId}/favorite?is_favorite=${newFavoriteState}`,
        {
          method: "PUT",
          headers,
        }
      );

      if (!res.ok) {
        // Revert on failure
        setDataset(prev => prev ? { ...prev, favorite: !newFavoriteState } : prev);
        throw new Error("Failed to update favorite status");
      }
    } catch (e) {
      console.error("Failed to toggle favorite:", e);
      // Already reverted above
    } finally {
      setIsTogglingFavorite(false);
    }
  };

  const handleBuildContext = async () => {
    if (!dataset || buildingContext) return;
    setBuildingContext(true);
    setContextStatus(null);
    try {
      // Gather all tables: fact table + dimension tables
      const tables = [dataset.table_name];
      if (dataset.dimensions) {
        for (const dim of dataset.dimensions) {
          const tbl = (dim as any).dimension_table || (dim as any).table_name;
          if (tbl && !tables.includes(tbl)) tables.push(tbl);
        }
      }
      const res = await msalFetch(`${API_BASE}/api/v1/context/build`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          database: dataset.database_name || "",
          schema_name: dataset.schema_name || "public",
          tables,
        }),
      });
      const data = await res.json();
      if (res.ok && data.supported !== false) {
        setContextStatus({ ok: true, message: `Context built: ${data.elements} elements across ${data.tables_profiled} tables` });
      } else {
        setContextStatus({ ok: false, message: data.reason || data.detail || "Failed to build context" });
      }
    } catch (e) {
      setContextStatus({ ok: false, message: e instanceof Error ? e.message : "Failed to build context" });
    } finally {
      setBuildingContext(false);
      setTimeout(() => setContextStatus(null), 8000);
    }
  };

  if (isLoading) {
    return <LoadingOverlay />;
  }

  if (!isAuthenticated) {
    return (
      <div className="page-shell">
        <header className="page-header">
          <h1 className="page-header-title">Dataset</h1>
          <p className="page-header-subtitle">Sign in to view this dataset.</p>
        </header>
      </div>
    );
  }

  return (
    <div className="page-shell" style={{ display: "flex", flexDirection: "column", height: "calc(100vh - 8px)", overflow: "hidden" }}>
      {/* ── Header card — matches dashboard view style ── */}
      <header style={{
        background: "var(--bg-surface)", border: "1px solid var(--border)", borderRadius: 12,
        padding: "12px 18px", display: "flex", alignItems: "center",
        justifyContent: "space-between", gap: 16, flexWrap: "wrap", marginBottom: 16,
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: 14, flex: 1, minWidth: 0 }}>
          <button
            type="button"
            onClick={() => router.push("/workspace?tab=datasets")}
            title="Back to datasets"
            style={{
              flexShrink: 0, width: 36, height: 36, borderRadius: 9,
              display: "flex", alignItems: "center", justifyContent: "center",
              border: "1px solid var(--border)", background: "var(--bg-surface)",
              color: "var(--text-secondary)", cursor: "pointer",
              transition: "background 0.15s, color 0.15s",
            }}
            onMouseEnter={e => { e.currentTarget.style.background = "var(--bg-hover)"; e.currentTarget.style.color = "var(--text-primary)"; }}
            onMouseLeave={e => { e.currentTarget.style.background = "var(--bg-surface)"; e.currentTarget.style.color = "var(--text-secondary)"; }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="19" y1="12" x2="5" y2="12" /><polyline points="12 19 5 12 12 5" />
            </svg>
          </button>
          <div style={{ display: "flex", flexDirection: "column", gap: 2, minWidth: 0 }}>
            {!isEditingName ? (
              <h1
                style={{ margin: 0, fontSize: "1.25rem", fontWeight: 800, color: "var(--text-primary)", lineHeight: 1.2, letterSpacing: "-0.3px", cursor: "pointer", borderRadius: 6, padding: "2px 6px", border: "2px solid transparent", transition: "background 0.15s" }}
                onClick={startEditingName}
                onMouseOver={e => { e.currentTarget.style.background = "var(--bg-hover)"; }}
                onMouseOut={e => { e.currentTarget.style.background = "transparent"; }}
                title="Click to rename"
              >
                {dataset?.name ?? "Dataset"}
              </h1>
            ) : (
              <input
                ref={nameInputRef}
                type="text"
                value={editNameValue}
                onChange={e => setEditNameValue(e.target.value)}
                onBlur={commitRename}
                onKeyDown={e => { if (e.key === "Enter") commitRename(); if (e.key === "Escape") setIsEditingName(false); }}
                style={{
                  fontSize: "1.25rem", fontWeight: 800, color: "var(--text-primary)",
                  padding: "2px 6px", margin: 0,
                  border: "2px solid var(--accent)", borderRadius: 6,
                  outline: "none", background: "var(--bg-surface)",
                  fontFamily: "inherit", minWidth: 260,
                }}
              />
            )}
            {dataset?.description && (
              <p style={{ fontSize: 12.5, color: "var(--text-muted)", margin: "2px 0 0 6px", lineHeight: 1.4 }}>
                {dataset.description}
              </p>
            )}
          </div>
        </div>
          {dataset && (
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <button
                type="button"
                className="overview-primary-btn"
                onClick={() => {
                  window.location.href = `/charts?datasetId=${dataset.id}`;
                }}
                style={{ fontSize: 14, padding: "9px 16px", fontWeight: 500 }}
              >
                <i className="fas fa-chart-bar" style={{ marginRight: 8 }} />
                Create chart
              </button>
              <button
                type="button"
                className="icon-button"
                onClick={() => {
                  window.location.href = `/datasets/new?datasetId=${dataset.id}`;
                }}
                title="Edit dataset"
                style={{ width: 36, height: 36, display: "flex", alignItems: "center", justifyContent: "center" }}
              >
                <i className="fas fa-edit" aria-hidden="true" style={{ fontSize: 14 }} />
                <span className="sr-only">Edit dataset</span>
              </button>
              <button
                type="button"
                className="icon-button"
                onClick={() => window.location.reload()}
                title="Refresh"
                style={{ width: 36, height: 36, display: "flex", alignItems: "center", justifyContent: "center" }}
              >
                <i className="fas fa-arrows-rotate" aria-hidden="true" style={{ fontSize: 14 }} />
              </button>
              <button
                type="button"
                className="icon-button"
                onClick={handleBuildContext}
                disabled={buildingContext}
                title="Build context for Adaptive Context Routing"
                style={{
                  width: 36, height: 36, display: "flex", alignItems: "center", justifyContent: "center",
                  opacity: buildingContext ? 0.5 : 1, cursor: buildingContext ? "not-allowed" : "pointer",
                }}
              >
                <i className={buildingContext ? "fas fa-spinner fa-spin" : "fas fa-brain"} aria-hidden="true" style={{ fontSize: 14 }} />
                <span className="sr-only">Build Context</span>
              </button>
              <button
                type="button"
                className="icon-button"
                onClick={handleToggleFavorite}
                disabled={isTogglingFavorite}
                title={dataset.favorite ? "Remove from favorites" : "Add to favorites"}
                style={{
                  width: 36,
                  height: 36,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  opacity: isTogglingFavorite ? 0.5 : 1,
                  cursor: isTogglingFavorite ? "not-allowed" : "pointer",
                  transition: "all 0.2s ease"
                }}
              >
                <i
                  className={dataset.favorite ? "fas fa-star" : "far fa-star"}
                  aria-hidden="true"
                  style={{
                    fontSize: 16,
                    color: dataset.favorite ? "#fbbf24" : "currentColor",
                    transition: "color 0.2s ease"
                  }}
                />
                <span className="sr-only">
                  {dataset.favorite ? "Remove from favorites" : "Add to favorites"}
                </span>
              </button>
            </div>
          )}
      </header>

      {contextStatus && (
        <div style={{
          margin: "0 24px 12px", padding: "10px 16px", borderRadius: 10,
          display: "flex", alignItems: "center", gap: 10, fontSize: 13,
          background: contextStatus.ok ? "color-mix(in srgb, var(--success) 10%, var(--bg-surface))" : "color-mix(in srgb, var(--error) 10%, var(--bg-surface))",
          border: `1px solid ${contextStatus.ok ? "color-mix(in srgb, var(--success) 30%, transparent)" : "color-mix(in srgb, var(--error) 30%, transparent)"}`,
          color: contextStatus.ok ? "var(--success)" : "var(--error)",
        }}>
          <i className={`fas ${contextStatus.ok ? "fa-check-circle" : "fa-exclamation-circle"}`} />
          {contextStatus.message}
        </div>
      )}

      {isLoading && <LoadingOverlay />}
      {error && !isLoading && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading dataset</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}

      {!isLoading && !error && dataset && (
        <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "auto", gap: 12, minHeight: 0 }}>
        <DatasetContextPanel datasetId={datasetId} />
        <div className="card" style={{ flexShrink: 0, border: "1px solid var(--border)", borderRadius: 12, padding: "20px" }}>
          {/* Key Info and Metrics */}
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12, flexShrink: 0, marginBottom: 12 }}>
            {/* Dataset Info */}
            <div style={{ padding: 0 }}>
              <h3 style={{ fontSize: 13, fontWeight: 700, color: "var(--text-muted)", margin: "0 0 14px 0", textTransform: "uppercase", letterSpacing: "0.05em", display: "flex", alignItems: "center", gap: 8 }}>
                <i className="fas fa-database" style={{ fontSize: 12, color: "var(--accent)", opacity: 0.7 }} />
                Connection
              </h3>
              <table style={{ width: "100%", fontSize: 13, borderCollapse: "collapse" }}>
                <thead>
                  <tr style={{ borderBottom: "1px solid var(--border)" }}>
                    <th align="left" style={{ padding: "0 12px 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Property</th>
                    <th align="left" style={{ padding: "0 0 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Value</th>
                  </tr>
                </thead>
                <tbody>
                  {[
                    { label: "Database", value: dataset.database_name || "(default)" },
                    { label: "Schema", value: dataset.schema_name || "(default)" },
                    { label: "Table", value: dataset.table_name || (dataset.sql_text ? "Virtual (SQL)" : "(none)") },
                    { label: "Date column", value: dataset.date_column || "(none)" },
                  ].map(({ label, value }, idx) => (
                    <tr key={label} style={{ borderBottom: idx < 3 ? "1px solid var(--border)" : "none" }}>
                      <td style={{ padding: "10px 12px 10px 0", fontWeight: 500, fontSize: 13, color: "var(--text-primary)" }}>{label}</td>
                      <td style={{ padding: "10px 0", fontSize: 13, color: "var(--text-secondary)", fontFamily: "var(--font-mono, monospace)" }}>{value}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {/* Metrics */}
            <div style={{ padding: 0 }}>
              <h3 style={{ fontSize: 13, fontWeight: 700, color: "var(--text-muted)", margin: "0 0 14px 0", textTransform: "uppercase", letterSpacing: "0.05em", display: "flex", alignItems: "center", gap: 8 }}>
                <i className="fas fa-calculator" style={{ fontSize: 12, color: "var(--accent)", opacity: 0.7 }} />
                Metrics {dataset.metrics && dataset.metrics.length > 0 && <span style={{ fontWeight: 500, fontSize: 12 }}>({dataset.metrics.length})</span>}
              </h3>
              {dataset.metrics && dataset.metrics.length > 0 ? (
                <div style={{ overflowY: "auto", maxHeight: 320 }}>
                  <table style={{ width: "100%", fontSize: 13, borderCollapse: "collapse" }}>
                    <thead>
                      <tr style={{ borderBottom: "1px solid var(--border)" }}>
                        <th align="left" style={{ padding: "0 12px 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Name</th>
                        <th align="left" style={{ padding: "0 12px 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Type</th>
                        <th align="left" style={{ padding: "0 0 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Expression</th>
                      </tr>
                    </thead>
                    <tbody>
                      {dataset.metrics.map((m, idx) => (
                        <tr key={`${m.name}-${idx}`} style={{ borderBottom: idx < dataset.metrics!.length - 1 ? "1px solid var(--border)" : "none" }}>
                          <td style={{ padding: "10px 12px 10px 0", fontWeight: 500, fontSize: 13, color: "var(--text-primary)", verticalAlign: "top" }}>{m.name}</td>
                          <td style={{ padding: "10px 12px 10px 0", fontSize: 11, color: "var(--success)", fontWeight: 600, textTransform: "uppercase", verticalAlign: "top" }}>{m.metric_type}</td>
                          <td style={{ padding: "10px 0", fontFamily: "monospace", fontSize: 12, color: "var(--text-secondary)", verticalAlign: "top" }}>{m.expression}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ) : (
                <p style={{ margin: 0, fontSize: 13, color: "var(--text-muted)" }}>No metrics defined</p>
              )}
            </div>
          </div>

          {/* Schema and Dimensions - Collapsible */}
          <style>{`
            details.schema-card summary::-webkit-details-marker { display: none; }
            details.schema-card[open] .schema-chevron { transform: rotate(180deg); }
            details.schema-card summary:hover { background: var(--bg-hover); }
            details.schema-card summary:hover .schema-chevron-box { border-color: var(--accent); color: var(--accent); }
          `}</style>
          <details className="schema-card" style={{ flexShrink: 0, borderTop: "1px solid var(--border)", paddingTop: 12, marginTop: 4 }}>
            <summary style={{
              padding: "14px 16px",
              cursor: "pointer",
              listStyle: "none",
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              fontWeight: 700,
              fontSize: 15,
              color: "var(--text-primary)",
              userSelect: "none",
              letterSpacing: "-0.01em",
              borderRadius: 10,
              transition: "background 0.15s",
            }}>
              <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <i className="fas fa-table" style={{ fontSize: 13, color: "var(--accent)", opacity: 0.7 }} />
                Schema & Dimensions
                <span style={{ color: "var(--text-muted)", fontWeight: 500, fontSize: 13 }}>({schemaColumns?.length || 0} columns, {dataset.dimensions?.length || 0} joins)</span>
              </span>
              <div className="schema-chevron-box" style={{
                width: 26, height: 26, borderRadius: 6,
                display: "flex", alignItems: "center", justifyContent: "center",
                background: "var(--bg-primary)", border: "1px solid var(--border)",
                transition: "border-color 0.15s, color 0.15s",
              }}>
                <i className="fas fa-chevron-down schema-chevron" style={{ fontSize: 10, color: "inherit", transition: "transform 0.2s" }} />
              </div>
            </summary>

            <div style={{ borderTop: "1px solid var(--border)", padding: 16 }}>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>
                {/* Schema */}
                <div>
                  <h4 style={{ fontSize: 13, fontWeight: 600, color: "var(--text-muted)", margin: "0 0 10px 0", textTransform: "uppercase", letterSpacing: "0.05em" }}>Schema</h4>
                  {schemaColumns && schemaColumns.length > 0 ? (
                    <div style={{ overflowY: "auto", maxHeight: 320 }}>
                      <table style={{ width: "100%", fontSize: 13, borderCollapse: "collapse" }}>
                        <thead>
                          <tr style={{ borderBottom: "1px solid var(--border)" }}>
                            <th align="left" style={{ padding: "0 12px 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Column</th>
                            <th align="left" style={{ padding: "0 12px 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Type</th>
                            <th align="left" style={{ padding: "0 0 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Role</th>
                          </tr>
                        </thead>
                        <tbody>
                          {schemaColumns.map((col, idx) => {
                            const label = col.column_name;
                            let role = "";
                            let roleColor = "var(--text-muted)";
                            if (col.is_dimension) {
                              role = "Dimension";
                              roleColor = "var(--accent)";
                            } else if (col.is_metric) {
                              role = "Metric";
                              roleColor = "var(--success)";
                            } else if ((col.semantic_type || "").toLowerCase() === "time") {
                              role = "Time";
                              roleColor = "#8b5cf6";
                            }
                            return (
                              <tr key={`${col.column_name}-${idx}`} style={{ borderBottom: idx < schemaColumns.length - 1 ? "1px solid var(--border)" : "none" }}>
                                <td style={{ padding: "10px 12px 10px 0", fontWeight: 500, fontSize: 13, color: "var(--text-primary)", verticalAlign: "top" }}>{label}</td>
                                <td style={{ padding: "10px 12px 10px 0", fontSize: 12, color: "var(--text-secondary)", verticalAlign: "top" }}>{col.data_type}</td>
                                <td style={{ padding: "10px 0", fontSize: 11, fontWeight: 600, color: roleColor, textTransform: "uppercase", verticalAlign: "top" }}>{role}</td>
                              </tr>
                            );
                          })}
                        </tbody>
                      </table>
                    </div>
                  ) : (
                    <p style={{ margin: 0, fontSize: 13, color: "var(--text-muted)" }}>No columns</p>
                  )}
                </div>

                {/* Dimensions */}
                <div>
                  <h4 style={{ fontSize: 13, fontWeight: 600, color: "var(--text-muted)", margin: "0 0 10px 0", textTransform: "uppercase", letterSpacing: "0.05em" }}>Dimensions</h4>
                  {dataset.dimensions && dataset.dimensions.length > 0 ? (
                    <div style={{ overflowY: "auto", maxHeight: 320 }}>
                      <table style={{ width: "100%", fontSize: 13, borderCollapse: "collapse" }}>
                        <thead>
                          <tr style={{ borderBottom: "1px solid var(--border)" }}>
                            <th align="left" style={{ padding: "0 12px 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Table</th>
                            <th align="left" style={{ padding: "0 0 8px 0", color: "var(--text-muted)", fontWeight: 500, fontSize: 11, textTransform: "uppercase", letterSpacing: "0.05em" }}>Join</th>
                          </tr>
                        </thead>
                        <tbody>
                          {dataset.dimensions.map((dim, idx) => {
                            const parsed = parseJoinCondition(dim.join_condition || "");
                            const tableDisplay = parsed.dimTable ? (parsed.dimSchema ? `${parsed.dimSchema}.${parsed.dimTable}` : parsed.dimTable) : dim.dimension_table;
                            let joinDisplay = dim.join_condition || "";
                            if (parsed.factTable && parsed.factKey && parsed.dimTable && parsed.dimKey) {
                              joinDisplay = `${parsed.factTable}.${parsed.factKey} = ${parsed.dimTable}.${parsed.dimKey}`;
                            }
                            return (
                              <tr key={`${dim.dimension_table}-${idx}`} style={{ borderBottom: idx < (dataset.dimensions?.length ?? 0) - 1 ? "1px solid var(--border)" : "none" }}>
                                <td style={{ padding: "10px 12px 10px 0", fontWeight: 500, fontSize: 13, verticalAlign: "top" }}>{tableDisplay}</td>
                                <td style={{ padding: "10px 0", fontSize: 12, fontFamily: "monospace", color: "var(--text-muted)", verticalAlign: "top" }} title={dim.join_condition}>{joinDisplay}</td>
                              </tr>
                            );
                          })}
                        </tbody>
                      </table>
                    </div>
                  ) : (
                    <p style={{ margin: 0, fontSize: 13, color: "var(--text-muted)" }}>No dimensions</p>
                  )}
                </div>
              </div>
            </div>
          </details>
        </div>

        {/* Data Preview — separate card */}
          <div className="card" style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden", border: "1px solid var(--border)", borderRadius: 12, padding: "16px 20px", minHeight: 0 }}>
            <div style={{ fontSize: 12, fontWeight: 600, color: "var(--text-muted)", marginBottom: 8, flexShrink: 0 }}>
              Data Preview <span style={{ fontWeight: 400 }}>(top 100 rows)</span>
            </div>
            {isLoadingPreview && <p className="muted">Loading preview…</p>}
            {previewError && !isLoadingPreview && (
              <p className="page-empty-body">{previewError}</p>
            )}
            {!isLoadingPreview && !previewError && previewColumns.length === 0 && (
              <p className="page-empty-body">No rows returned.</p>
            )}
            {!isLoadingPreview && !previewError && previewColumns.length > 0 && (
              <div style={{ flex: 1, overflow: "auto", minHeight: 0, border: "1px solid var(--border)", borderRadius: 8 }}>
                <table className="results-table" style={{ width: "100%", fontSize: 12 }}>
                  <thead style={{ position: "sticky", top: 0, zIndex: 2 }}>
                    <tr style={{ background: "var(--bg-surface)" }}>
                      {previewColumns.map((col, colIndex) => {
                        const isSorted = previewSortColumnIndex === colIndex;
                        const sortIconClass = !isSorted
                          ? "fas fa-sort column-sort-icon"
                          : previewSortDirection === "asc"
                          ? "fas fa-sort-up column-sort-icon"
                          : "fas fa-sort-down column-sort-icon";

                        return (
                          <th
                            key={col}
                            align="left"
                            onClick={() => handlePreviewSort(colIndex)}
                            className={isSorted ? "sorted" : undefined}
                            style={{ cursor: "pointer" }}
                          >
                            <span className="column-header-label">{col}</span>
                            <i className={sortIconClass} />
                          </th>
                        );
                      })}
                    </tr>
                  </thead>
                  <tbody>
                    {sortedPreviewRows.map((row, rowIdx) => (
                      <tr key={rowIdx}>
                        {row.map((cell, colIdx) => (
                          <td key={colIdx}>{cell === null || cell === undefined ? "" : String(cell)}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
