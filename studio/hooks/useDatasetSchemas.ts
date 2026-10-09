"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { msalFetch } from "../utils/msalFetch";
import type { DatasetSchema } from "../utils/nlToSql";

/**
 * One read of every visible dataset's askable shape.
 *
 * The chat workbench needs the columns and metrics of *every* dataset the user
 * can see, so a question can be routed to the dataset that can answer it. It
 * used to collect that one dataset at a time — a `GET /datasets/{id}` per
 * dataset, fanned out after the dataset list had already resolved — and each of
 * those documents costs a document read plus three component reads plus a
 * shadow-read observation on the server, and a proxy round trip in the browser.
 * With nine datasets that is nine requests nobody reads individually, and the
 * composer was disabled until the slowest of them landed.
 *
 * `GET /datasets/schemas` answers the same question once, carrying only the
 * names and types the router scores on. It is a projection of the dataset
 * documents computed per request, so it is as fresh as the documents are; there
 * is no cache here and nothing derived from a dataset is held across a reload.
 */

export interface DatasetSchemaEntry {
  id: number;
  name: string;
  description: string | null;
  databaseName: string | null;
  /** Resolved and qualified once by the server. */
  schema: DatasetSchema;
}

/** `loading` until the read settles; `empty` when the user can see no dataset. */
export type DatasetSchemasState = "loading" | "empty" | "ready" | "error";

export interface DatasetSchemasHandle {
  schemas: DatasetSchemaEntry[];
  state: DatasetSchemasState;
  /** Latest value without re-rendering on it — for event handlers. */
  ref: React.RefObject<DatasetSchemaEntry[]>;
  /** Resolves once the in-flight read has settled, so a handler can await it. */
  settled: () => Promise<DatasetSchemaEntry[]>;
  reload: () => Promise<void>;
}

interface RawColumn {
  name?: string;
  data_type?: string;
}

interface RawEntry {
  id?: string | number;
  name?: string;
  description?: string | null;
  database_name?: string | null;
  table?: string;
  columns?: RawColumn[];
  metrics?: { name?: string; expression?: string }[];
}

/**
 * The three families nlToSql reasons over, from a source-reported SQL type.
 *
 * It knows "number", "date" and "string" and nothing else, so everything that
 * is neither numeric nor temporal is a string here — a boolean included.
 * `double precision`, `real` and `money` are named explicitly: they are
 * numbers, and a metric column declared as one of them used to fall through to
 * "string" and stay invisible to the parser's numeric matching.
 */
function columnKind(dataType: string | undefined): string {
  const type = (dataType || "").toLowerCase();
  if (/int|float|double|decimal|numeric|real|money/.test(type)) return "number";
  if (/date|time/.test(type)) return "date";
  return "string";
}

function adapt(raw: RawEntry): DatasetSchemaEntry | null {
  const id = Number(raw.id);
  if (!Number.isFinite(id)) return null;
  return {
    id,
    name: raw.name || `Dataset ${id}`,
    description: raw.description ?? null,
    databaseName: raw.database_name ?? null,
    schema: {
      tableName: raw.table || "data",
      columns: (raw.columns || [])
        .filter((column) => !!column.name)
        .map((column) => ({ name: column.name as string, type: columnKind(column.data_type) })),
      metrics: (raw.metrics || [])
        .filter((metric) => !!metric.name)
        .map((metric) => ({
          name: metric.name as string,
          expression: metric.expression || `SUM(${metric.name})`,
        })),
    },
  };
}

export function useDatasetSchemas(enabled = true): DatasetSchemasHandle {
  const [schemas, setSchemas] = useState<DatasetSchemaEntry[]>([]);
  const [state, setState] = useState<DatasetSchemasState>("loading");
  const ref = useRef<DatasetSchemaEntry[]>([]);
  const inFlight = useRef<Promise<DatasetSchemaEntry[]> | null>(null);

  const reload = useCallback(async () => {
    if (!enabled) return;
    const read = (async () => {
      try {
        const res = await msalFetch("/api/v1/datasets/schemas");
        if (!res.ok) {
          setState("error");
          return ref.current;
        }
        const body = (await res.json()) as { schemas?: RawEntry[] };
        const next = (body.schemas || [])
          .map(adapt)
          .filter((entry): entry is DatasetSchemaEntry => entry !== null);
        ref.current = next;
        setSchemas(next);
        setState(next.length > 0 ? "ready" : "empty");
        return next;
      } catch {
        setState("error");
        return ref.current;
      }
    })();
    inFlight.current = read;
    await read;
  }, [enabled]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const settled = useCallback(async () => {
    if (inFlight.current) return inFlight.current;
    return ref.current;
  }, []);

  return useMemo(
    () => ({ schemas, state, ref, settled, reload }),
    [schemas, state, settled, reload],
  );
}
