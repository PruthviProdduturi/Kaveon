"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { DatasetContextPanel } from "../../../components/DatasetContextPanel";
import { DatasetContextEditor } from "../../../components/DatasetContextEditor";
import { useAuth } from "../../../auth/useAuth";
import { useRouter, useParams } from "next/navigation";
import { useRecents } from "../../../hooks/useRecents";
import { useDlmArtifact } from "../../../hooks/useDlmArtifact";
import s from "./dataset.module.css";

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
  keyColumn?: string | null;
  valueKey?: string | string[] | null;
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
  /** Which SQL this dataset takes, resolved by the server from the registered
   *  catalog sources. `engine` means double-quoted identifiers and LIMIT. */
  sql_dialect?: "engine" | "tsql";
}

function compactNum(n: number): string {
  const a = Math.abs(n);
  if (a >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (a >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (a >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
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
  // An Engine catalog takes `schema.table` with LIMIT N; Fabric SQL and Azure
  // SQL take [schema].[table] with TOP N. The server resolves which, because
  // only it knows whether `database_name` names an Engine catalog or a
  // registered external database. This was inferred here from the schema name,
  // with `public` and `climate_energy` hardcoded as the two that meant Engine,
  // so a dataset in any other schema — every ai_benchmarks dataset — was given
  // `SELECT TOP 100 … FROM [ai_benchmarks].[leaderboard]`, which the Engine
  // refuses with "schema not found" because the brackets become part of the
  // quoted name. Default to the Engine: a deployment with no registered
  // external database has nothing else to be.
  const isEngine = (dataset.sql_dialect ?? "engine") === "engine";

  const filters = dataset.filters || [];
  const whereClauses: string[] = [];

  // Filter on the fact table's own key where the dataset records one, so the
  // predicate does not need the dimension join.
  for (const f of filters) {
    const column = (f.column || "").trim();
    if (!column) continue;
    const op = (f.op || "=").trim().toUpperCase();
    const val = f.value;

    const keyColumn = f.keyColumn;
    const valueKey = f.valueKey;

    if (keyColumn && (valueKey !== undefined && valueKey !== null && valueKey !== '')) {
      const keyColName = keyColumn.split('.').pop() || keyColumn;

      if (Array.isArray(valueKey)) {
        if (valueKey.length === 0) continue;
        const safeKeys = valueKey.map((k) => `'${String(k).replace(/'/g, "''")}'`);
        whereClauses.push(`${quoteIdentifier(keyColName)} IN (${safeKeys.join(", ")})`);
      } else {
        const keyStr = `'${String(valueKey).replace(/'/g, "''")}'`;
        whereClauses.push(`${quoteIdentifier(keyColName)} ${op} ${keyStr}`);
      }
    } else {
      // Standard filter on the display column (may need the dimension join).
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

  // The preview shows every column the model declares — dimensions, metrics
  // and the time column — so a reader sees the shape of the whole dataset.
  const modelColumns = (dataset.columns || []).slice();

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

  // Join only the dimensions a displayed column actually comes from.
  const dimsToJoin = new Set<string>();
  modelColumns.forEach((c) => {
    if (c.is_dimension && c.table_name) {
      dimsToJoin.add(c.table_name);
    }
  });

  const joinClauses: string[] = [];
  const dims = dataset.dimensions || [];
  const usedAliases = new Set<string>();
  const dimAliases: Record<string, string> = {};

  // Dimensions grouped by the fact key they join on, so two dimension tables
  // that share one key can be coalesced into a single displayed column.
  const dimensionsByFactKey: Record<string, Array<{ alias: string; tableName: string; semantic?: string; columnName?: string }>> = {};

  // A semantic type resolves to the alias of the dimension that supplies it.
  const semanticToAlias: Record<string, string> = {};

  for (const dim of dims) {
    if (!dimsToJoin.has(dim.dimension_table)) continue;
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

      const dimColumn = (dataset.columns || []).find(c =>
        c.table_name === dim.dimension_table &&
        c.is_dimension === true &&
        c.semantic_type?.toLowerCase() === aliasBase.toLowerCase()
      );

      if (dimColumn?.semantic_type) {
        semanticToAlias[dimColumn.semantic_type.toLowerCase()] = alias;
      }
    }

    if (parsed.factSchema && parsed.factTable && parsed.factKey && parsed.dimKey && alias) {
      const factRef = `${quoteIdentifier(parsed.factSchema)}.${quoteIdentifier(parsed.factTable)}`;
      const aliasIdent = quoteIdentifier(alias);
      const onExpr = `${factRef}.${quoteIdentifier(parsed.factKey)} = ${aliasIdent}.${quoteIdentifier(parsed.dimKey)}`;
      joinClauses.push(`LEFT JOIN ${dimRef} AS ${aliasIdent} ON ${onExpr}`);

      const factKey = parsed.factKey.toLowerCase();
      if (!dimensionsByFactKey[factKey]) {
        dimensionsByFactKey[factKey] = [];
      }

      const dimColumn = (dataset.columns || []).find(c =>
        c.table_name === dim.dimension_table &&
        c.is_dimension === true &&
        c.semantic_type?.toLowerCase() === aliasBase?.toLowerCase()
      );

      dimensionsByFactKey[factKey].push({
        alias,
        tableName: dim.dimension_table,
        semantic: dimColumn?.semantic_type || undefined,
        columnName: dimColumn?.column_name || undefined
      });
    } else {
      joinClauses.push(`LEFT JOIN ${dimRef} ON ${dim.join_condition}`);
    }
  }

  // Where one semantic type is supplied by more than one dimension on the same
  // fact key, the displayed column is a COALESCE across those sources.
  const semanticToSources: Record<string, Array<{ alias: string; columnName: string }>> = {};
  Object.values(dimensionsByFactKey).forEach((dimGroup) => {
    if (dimGroup.length <= 1) return;
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
      }
    });
  });

  const factRef = isEngine ? `${schema}.${table}` : `${quoteIdentifier(schema)}.${quoteIdentifier(table)}`;

  let selectClause: string;
  if (modelColumns.length > 0) {
    const selectExprs: string[] = [];

    const getSourceForColumn = (col: DatasetColumn): string => {
      // A dimension column resolves by its semantic type first.
      if (col.semantic_type) {
        const alias = semanticToAlias[col.semantic_type.toLowerCase()];
        if (alias) return quoteIdentifier(alias);
      }

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

    const usedDisplayNames = new Set<string>();
    const processedSemantics = new Set<string>();

    for (const col of modelColumns) {
      let displayName =
        col.semantic_type && col.semantic_type.toLowerCase() !== "time"
          ? col.semantic_type
          : col.column_name;

      const semantic = (col.semantic_type || '').toLowerCase();

      if (semantic && semantic !== 'time' && semanticToSources[semantic]?.length > 1) {
        if (processedSemantics.has(semantic)) continue;
        processedSemantics.add(semantic);

        const sources = semanticToSources[semantic];
        const coalesceParts = sources.map(src => `${quoteIdentifier(src.alias)}.${quoteIdentifier(src.columnName)}`);
        selectExprs.push(`COALESCE(${coalesceParts.join(', ')}) AS ${quoteIdentifier(displayName)}`);
        usedDisplayNames.add(displayName.toLowerCase());
      } else {
        // Fall back to the physical column name when the label is taken, so
        // every projected column has a distinct name.
        if (usedDisplayNames.has(displayName.toLowerCase())) {
          displayName = col.column_name;
        }
        usedDisplayNames.add(displayName.toLowerCase());

        const source = getSourceForColumn(col);
        selectExprs.push(`${source}.${quoteIdentifier(col.column_name)} AS ${quoteIdentifier(displayName)}`);
      }
    }

    selectClause = selectExprs.join(", ");
  } else {
    selectClause = "*";
  }

  let base = isEngine
    ? `SELECT ${selectClause} FROM ${factRef}`
    : `SELECT TOP ${top} ${selectClause} FROM ${factRef}`;
  if (joinClauses.length > 0) {
    base = `${base} ${joinClauses.join(" ")}`;
  }
  if (whereClauses.length > 0) {
    base = `${base} WHERE ${whereClauses.join(" AND ")}`;
  }
  if (isEngine) {
    base = `${base} LIMIT ${top}`;
  }
  return base;
}

/** The grid's own frame and row rhythm, with a bar per column. */
function PreviewSkeleton({ columns, rows }: { columns: number; rows: number }) {
  const widths = [72, 54, 86, 44, 64, 50, 78];
  return (
    <div className={s.skelGrid} style={{ ["--cols" as string]: columns }} aria-hidden="true">
      <div className={`${s.skelRow} ${s.skelHead}`}>
        {Array.from({ length: columns }, (_, i) => (
          <span key={i} className={s.skel} style={{ width: 56 + (i % 3) * 14 }} />
        ))}
      </div>
      {Array.from({ length: rows }, (_, r) => (
        <div key={r} className={s.skelRow}>
          {Array.from({ length: columns }, (_, c) => (
            <span key={c} className={s.skel} style={{ width: widths[(r + c) % widths.length] }} />
          ))}
        </div>
      ))}
    </div>
  );
}

export default function DatasetDetailPage() {
  const router = useRouter();
  const params = useParams();
  const { isAuthenticated, account } = useAuth();

  const [dataset, setDataset] = useState<DatasetDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isTogglingFavorite, setIsTogglingFavorite] = useState(false);
  const [isEditingName, setIsEditingName] = useState(false);
  const [editNameValue, setEditNameValue] = useState("");
  const nameInputRef = useRef<HTMLInputElement>(null);
  const { addRecent } = useRecents();
  const [previewColumns, setPreviewColumns] = useState<string[]>([]);
  const [previewRows, setPreviewRows] = useState<unknown[][]>([]);
  const [previewOpen, setPreviewOpen] = useState(true);
  const [isLoadingPreview, setIsLoadingPreview] = useState(true);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewSortColumnIndex, setPreviewSortColumnIndex] = useState<number | null>(null);
  const [previewSortDirection, setPreviewSortDirection] = useState<"asc" | "desc">("asc");
  const [previewSql, setPreviewSql] = useState<string>("");
  const [sqlCopied, setSqlCopied] = useState(false);

  const datasetId = params?.id as string | undefined;
  // The account object is rebuilt on every auth-provider render, so the
  // effects below depend on the identity string rather than the object:
  // a re-render must not re-run the preview statement.
  const userEmail = account?.email || account?.username || null;

  // One read of the DLM artifact for the whole page: the row-count fact here,
  // the Context card, and the context editor's defaults all read this.
  const dlm = useDlmArtifact(datasetId, isAuthenticated);
  const rowCount = dlm.rowCount;

  // The Schema card shows every column, even when several fact columns map to
  // the same dimension table: each one is a distinct business concept. Two
  // dimension tables sharing one fact key do collapse to a single label.
  const schemaColumns: DatasetColumn[] = useMemo(() => {
    if (!dataset?.columns) return [];
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
    } catch { /* silent — the name reverts on the next load */ }
  };

  useEffect(() => {
    if (!isAuthenticated || !datasetId) return;

    const load = async () => {
      setIsLoading(true);
      setError(null);
      try {
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
        const data = await res.json() as DatasetDetail & { is_favorite?: boolean };

        const mappedData: DatasetDetail = {
          ...data,
          favorite: data.favorite ?? data.is_favorite ?? false,
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
  }, [isAuthenticated, datasetId, userEmail, addRecent]);

  useEffect(() => {
    if (!isAuthenticated) return;
    if (!dataset) return;

    const runPreview = async () => {
      try {
        setIsLoadingPreview(true);
        setPreviewError(null);

        const qualifiedTable = dataset.schema_name && dataset.schema_name !== "dbo" && dataset.schema_name !== "public"
          ? `${dataset.schema_name}.${dataset.table_name}`
          : dataset.table_name;
        // The join builder exists to reach a dataset's dimension tables, and it
        // writes SQL Server: TOP, bracket quoting, and columns qualified by
        // schema *and* table. A dataset with no dimensions needs none of that,
        // and the Engine refuses a three-part name that does not match the
        // selected catalog — which is what the builder produced here, because
        // it qualifies a date column the table does not even have.
        //
        // It used to be chosen by a hardcoded list of schema names from the
        // warehouse era, so a dataset simply named outside that list took the
        // SQL Server path regardless of where it actually lives.
        const joinsDimensions = (dataset.dimensions || []).length > 0;
        const sql = dataset.sql_text && !dataset.table_name
          ? `SELECT * FROM (${dataset.sql_text.replace(/;\s*$/, "").trim()}) AS _preview LIMIT 100`
          : joinsDimensions
            ? buildDatasetPreviewSql(dataset, 100)
            : `SELECT * FROM ${qualifiedTable} LIMIT 100`;
        setPreviewSql(sql);

        const tablesUsed = dataset.sql_text && !dataset.table_name
          ? []
          : [
          `${dataset.schema_name || 'dbo'}.${dataset.table_name}`, // Fact table
          ...(dataset.dimensions || []).map(d => d.dimension_table) // Dimension tables
        ];

        // No `lab/switch-database` call precedes this. That route only echoes
        // the name it is given — it selects nothing server-side — while the
        // statement below already carries its catalog, so the request was a
        // round trip the preview waited on for nothing.
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
        const apiRows: unknown[][] = Array.isArray(data.rows) ? data.rows : [];

        let finalColumns = apiColumns;
        let finalRows = apiRows;

        if (dataset.columns && dataset.columns.length > 0 && apiColumns.length > 0) {
          const preferredNames = dataset.columns.map((c) =>
            c.semantic_type && c.semantic_type.toLowerCase() !== "time"
              ? c.semantic_type
              : c.column_name,
          );

          // Keep the preferred names unique: when several dataset columns share
          // one semantic label (two Product columns coalesced in the SQL) the
          // resulting column is referenced once in the preview ordering.
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

        // The time column reads first.
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
  }, [isAuthenticated, dataset, userEmail]);

  const sortedPreviewRows = useMemo(() => {
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
    // Per column: unsorted -> ascending -> descending -> unsorted.
    if (previewSortColumnIndex === columnIndex) {
      if (previewSortDirection === "asc") {
        setPreviewSortDirection("desc");
      } else {
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
    setDataset(prev => prev ? { ...prev, favorite: newFavoriteState } : prev);

    try {
      const headers: Record<string, string> = {};
      if (userEmail) {
        headers['x-user-email'] = userEmail;
      }

      const res = await msalFetch(
        `${API_BASE}/api/v1/datasets/${datasetId}/favorite?is_favorite=${newFavoriteState}`,
        { method: "PUT", headers },
      );

      if (!res.ok) {
        setDataset(prev => prev ? { ...prev, favorite: !newFavoriteState } : prev);
      }
    } catch {
      setDataset(prev => prev ? { ...prev, favorite: !newFavoriteState } : prev);
    } finally {
      setIsTogglingFavorite(false);
    }
  };

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

  // The number of columns the skeleton draws: the dataset's own column count
  // once it is known, so the grid does not change shape when the rows arrive.
  const skeletonColumns = Math.max(3, Math.min(schemaColumns.length || 5, 7));

  return (
    <div className={`page-shell ${s.page}`}>
      <nav className={s.crumbs}>
        <button type="button" className={s.crumb} onClick={() => router.push("/workspace?tab=datasets")}>
          <i className="fas fa-chevron-left" aria-hidden="true" />
          Datasets
        </button>
      </nav>

      <header className={`${s.card} ${s.header}`}>
        <div className={s.identity}>
          {!isEditingName ? (
            <h1
              className={s.title}
              tabIndex={dataset ? 0 : undefined}
              onClick={() => { if (dataset) startEditingName(); }}
              onKeyDown={(e) => {
                if (!dataset) return;
                if (e.key === "Enter" || e.key === " ") { e.preventDefault(); startEditingName(); }
              }}
              title={dataset ? "Click to rename" : undefined}
            >
              {dataset?.name ?? "Dataset"}
            </h1>
          ) : (
            <input
              ref={nameInputRef}
              type="text"
              className={s.titleInput}
              value={editNameValue}
              onChange={e => setEditNameValue(e.target.value)}
              onBlur={commitRename}
              onKeyDown={e => { if (e.key === "Enter") commitRename(); if (e.key === "Escape") setIsEditingName(false); }}
            />
          )}

          <div className={s.facts}>
            {rowCount != null && (
              <span className={s.fact}>
                <i className="fas fa-table-list" aria-hidden="true" />
                {compactNum(rowCount)} rows
              </span>
            )}
            {schemaColumns.length > 0 && (
              <span className={s.fact}>
                <i className="fas fa-table-columns" aria-hidden="true" />
                {schemaColumns.length} columns
              </span>
            )}
            {(dataset?.dimensions?.length ?? 0) > 0 && (
              <span className={s.fact}>
                <i className="fas fa-link" aria-hidden="true" />
                {dataset!.dimensions!.length} joins
              </span>
            )}
            {(dataset?.metrics?.length ?? 0) > 0 && (
              <span className={s.fact}>
                <i className="fas fa-chart-line" aria-hidden="true" />
                {dataset!.metrics!.length} metrics
              </span>
            )}
            {isLoading && <span className={s.skel} style={{ width: 160 }} aria-hidden="true" />}
          </div>

          {dataset?.description && <p className={s.about}>{dataset.description}</p>}
        </div>

        {dataset && (
          <div className={s.actions}>
            <button
              type="button"
              className={s.primary}
              onClick={() => router.push(`/charts?datasetId=${dataset.id}`)}
            >
              <i className="fas fa-chart-bar" aria-hidden="true" />
              Create chart
            </button>
            <button
              type="button"
              className={s.iconBtn}
              onClick={() => router.push(`/datasets/new?datasetId=${dataset.id}`)}
              title="Edit dataset"
            >
              <i className="fas fa-pen" aria-hidden="true" />
              <span className="sr-only">Edit dataset</span>
            </button>
            <button
              type="button"
              className={s.iconBtn}
              onClick={() => window.location.reload()}
              title="Reload"
            >
              <i className="fas fa-arrows-rotate" aria-hidden="true" />
              <span className="sr-only">Reload</span>
            </button>
            <button
              type="button"
              className={`${s.iconBtn} ${dataset.favorite ? s.starOn : ""}`}
              onClick={handleToggleFavorite}
              disabled={isTogglingFavorite}
              title={dataset.favorite ? "Remove from favorites" : "Add to favorites"}
            >
              <i className={dataset.favorite ? "fas fa-star" : "far fa-star"} aria-hidden="true" />
              <span className="sr-only">
                {dataset.favorite ? "Remove from favorites" : "Add to favorites"}
              </span>
            </button>
          </div>
        )}
      </header>

      {error && (
        <div className={s.card}>
          <div className={s.bar}>
            <i className={`fas fa-circle-exclamation ${s.barIcon}`} aria-hidden="true" />
            <span className={s.barName}>Problem loading dataset</span>
          </div>
          <p className={`${s.note} ${s.noteError}`}>{error}</p>
        </div>
      )}

      {!error && (
        <>
          {/* The Context card and its editor read the shared artifact, so they
              render as soon as that one read lands rather than waiting for the
              dataset document and then fetching it again themselves. */}
          <DatasetContextPanel datasetId={datasetId} dlm={dlm} className={s.card} />
          <DatasetContextEditor datasetId={datasetId} dlm={dlm} className={s.card} />

          <section className={`${s.card} ${s.preview}`}>
            <div className={s.bar}>
              <button
                type="button"
                className={s.barToggle}
                aria-expanded={previewOpen}
                onClick={() => setPreviewOpen((open) => !open)}
              >
                <i className={`fas fa-table ${s.barIcon}`} aria-hidden="true" />
                <span className={s.barName}>Data preview</span>
                <span className={s.barNote}>top 100 rows</span>
                <i className={`fas fa-chevron-down ${s.chev} ${previewOpen ? s.chevOpen : ""}`} aria-hidden="true" />
              </button>
              {previewSql && (
                <div className={s.barTools}>
                  <button
                    type="button"
                    className={`${s.ghost} ${sqlCopied ? s.ghostDone : ""}`}
                    onClick={() => {
                      void navigator.clipboard.writeText(previewSql);
                      setSqlCopied(true);
                      setTimeout(() => setSqlCopied(false), 2000);
                    }}
                  >
                    <i className={sqlCopied ? "fas fa-check" : "fas fa-copy"} aria-hidden="true" />
                    {sqlCopied ? "Copied" : "Copy SQL"}
                  </button>
                  <button
                    type="button"
                    className={s.ghost}
                    onClick={() => {
                      // `database_name` is the Engine catalog, and the Lab
                      // picks its source from `catalog`. Sent as `db` it was
                      // read as a schema, so the Lab stayed on its default
                      // source while the statement named another catalog:
                      // "Engine query references a catalog outside the
                      // selected source".
                      const parts = [`query=${encodeURIComponent(previewSql)}`];
                      if (dataset?.database_name) parts.push(`catalog=${encodeURIComponent(dataset.database_name)}`);
                      if (dataset?.schema_name) parts.push(`schema=${encodeURIComponent(dataset.schema_name)}`);
                      router.push(`/lab?${parts.join("&")}`);
                    }}
                  >
                    <i className="fas fa-terminal" aria-hidden="true" />
                    Execute in Lab
                  </button>
                </div>
              )}
            </div>

            {previewOpen && (isLoading || isLoadingPreview) && (
              <PreviewSkeleton columns={skeletonColumns} rows={6} />
            )}
            {previewOpen && previewError && !isLoadingPreview && (
              <p className={`${s.note} ${s.noteError}`}>{previewError}</p>
            )}
            {previewOpen && !isLoading && !isLoadingPreview && !previewError && previewColumns.length === 0 && (
              <p className={s.note}>No rows returned.</p>
            )}
            {previewOpen && !isLoadingPreview && !previewError && previewColumns.length > 0 && (
              <div className={s.grid}>
                <table className="results-table">
                  <thead>
                    <tr>
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
                          >
                            <span className="column-header-label">{col}</span>
                            <i className={sortIconClass} aria-hidden="true" />
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
          </section>

          {dataset && (
            <>
              <details className={s.fold}>
                <summary className={s.foldSummary}>
                  <i className={`fas fa-database ${s.barIcon}`} aria-hidden="true" />
                  <span className={s.barName}>Connection &amp; metrics</span>
                  <span className={s.barNote}>
                    {dataset.database_name || "default"} · {dataset.schema_name || "default"} · {dataset.table_name}
                    {dataset.metrics?.length ? ` · ${dataset.metrics.length} metrics` : ""}
                  </span>
                  <i className={`fas fa-chevron-down ${s.chev} ${s.push}`} aria-hidden="true" />
                </summary>
                <div className={s.foldBody}>
                  <div className={s.col}>
                    <h2 className={s.colHead}>Connection</h2>
                    <table className={s.defs}>
                      <tbody>
                        {[
                          { label: "Database", value: dataset.database_name || "(default)" },
                          { label: "Schema", value: dataset.schema_name || "(default)" },
                          { label: "Table", value: dataset.table_name || (dataset.sql_text ? "Virtual (SQL)" : "(none)") },
                          { label: "Date column", value: dataset.date_column || "(none)" },
                        ].map(({ label, value }) => (
                          <tr key={label}>
                            <td>{label}</td>
                            <td><span className={s.mono}>{value}</span></td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                  <div className={s.col}>
                    <h2 className={s.colHead}>
                      Metrics{dataset.metrics && dataset.metrics.length > 0 ? ` (${dataset.metrics.length})` : ""}
                    </h2>
                    {dataset.metrics && dataset.metrics.length > 0 ? (
                      <div className={s.scroll}>
                        <table className={s.defs}>
                          <tbody>
                            {dataset.metrics.map((m, idx) => (
                              <tr key={`${m.name}-${idx}`}>
                                <td>{m.name}</td>
                                <td>
                                  <div className={s.cell}>
                                    <span className={`${s.tag} ${s.tagMetric}`}>{m.metric_type}</span>
                                    <span className={s.mono}>{m.expression}</span>
                                  </div>
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </div>
                    ) : (
                      <p className={s.empty}>No metrics defined</p>
                    )}
                  </div>
                </div>
              </details>

              <details className={s.fold}>
                <summary className={s.foldSummary}>
                  <i className={`fas fa-table-columns ${s.barIcon}`} aria-hidden="true" />
                  <span className={s.barName}>Schema &amp; dimensions</span>
                  <span className={s.barNote}>
                    {schemaColumns.length} columns · {dataset.dimensions?.length || 0} joins
                  </span>
                  <i className={`fas fa-chevron-down ${s.chev} ${s.push}`} aria-hidden="true" />
                </summary>
                <div className={s.foldBody}>
                  <div className={s.col}>
                    <h2 className={s.colHead}>Columns</h2>
                    {schemaColumns.length > 0 ? (
                      <div className={s.scroll}>
                        <table className={s.defs}>
                          <tbody>
                            {schemaColumns.map((col, idx) => {
                              let role = "";
                              let roleClass = "";
                              if (col.is_dimension) { role = "Dimension"; roleClass = s.tagDim; }
                              else if (col.is_metric) { role = "Metric"; roleClass = s.tagMetric; }
                              else if ((col.semantic_type || "").toLowerCase() === "time") { role = "Time"; roleClass = s.tagTime; }
                              return (
                                <tr key={`${col.column_name}-${idx}`}>
                                  <td>{col.column_name}</td>
                                  <td>
                                    <div className={s.cell}>
                                      <span className={s.mono}>{col.data_type}</span>
                                      {role && <span className={`${s.tag} ${roleClass}`}>{role}</span>}
                                    </div>
                                  </td>
                                </tr>
                              );
                            })}
                          </tbody>
                        </table>
                      </div>
                    ) : (
                      <p className={s.empty}>No columns</p>
                    )}
                  </div>
                  <div className={s.col}>
                    <h2 className={s.colHead}>Joins</h2>
                    {dataset.dimensions && dataset.dimensions.length > 0 ? (
                      <div className={s.scroll}>
                        <table className={s.defs}>
                          <tbody>
                            {dataset.dimensions.map((dim, idx) => {
                              const parsed = parseJoinCondition(dim.join_condition || "");
                              const tableDisplay = parsed.dimTable
                                ? (parsed.dimSchema ? `${parsed.dimSchema}.${parsed.dimTable}` : parsed.dimTable)
                                : dim.dimension_table;
                              let joinDisplay = dim.join_condition || "";
                              if (parsed.factTable && parsed.factKey && parsed.dimTable && parsed.dimKey) {
                                joinDisplay = `${parsed.factTable}.${parsed.factKey} = ${parsed.dimTable}.${parsed.dimKey}`;
                              }
                              return (
                                <tr key={`${dim.dimension_table}-${idx}`}>
                                  <td>{tableDisplay}</td>
                                  <td title={dim.join_condition}><span className={s.mono}>{joinDisplay}</span></td>
                                </tr>
                              );
                            })}
                          </tbody>
                        </table>
                      </div>
                    ) : (
                      <p className={s.empty}>No dimension joins</p>
                    )}
                  </div>
                </div>
              </details>
            </>
          )}
        </>
      )}
    </div>
  );
}
