import React, { useEffect, useMemo, useState } from "react";

import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";
import { mmh3Hash64 } from "../../utils/mmh3";
import { useChartBuilder, ChartFilterOption } from "./ChartBuilderContext";

const FilterBuilder: React.FC = () => {
  const {
    selectedDatasetId,
    datasetColumns,
    datasetDetail,
    filters,
    setFilters,
    filterLogic,
    setFilterLogic,
  } = useChartBuilder();

  const [expandedFilterId, setExpandedFilterId] = useState<string | null>(null);
  const [valueCache, setValueCache] = useState<Record<string, { options: ChartFilterOption[]; keyColumn: string | null }>>({});

  const addFilter = () => {
    const id = crypto.randomUUID();
    setFilters((prev) => [
      ...prev,
      {
        id,
        column: "",
        operator: "=",
        value: "",
        options: [],
        isLoading: false,
        isPending: true,
        groupKey: "primary",
      },
    ]);
    setExpandedFilterId(id);
  };

  const addGroupFilter = () => {
    const id = crypto.randomUUID();
    setFilters((prev) => [
      ...prev,
      {
        id,
        column: "",
        operator: "=",
        value: "",
        options: [],
        isLoading: false,
        isPending: true,
        groupKey: "secondary",
      },
    ]);
    setExpandedFilterId(id);
  };

  const updateFilter = (
    id: string,
    patch: Partial<{
      column: string;
      columnLabel: string;
      keyColumn: string;
      operator: string;
      value: string;
      valueKey: string;
      options: ChartFilterOption[];
      isLoading: boolean;
      isPending: boolean;
      groupKey: "primary" | "secondary";
    }>,
  ) => {
    setFilters((prev) => prev.map((f) => (f.id === id ? { ...f, ...patch } : f)));
  };

  const removeFilter = (id: string) => {
    setFilters((prev) => prev.filter((f) => f.id !== id));
    if (expandedFilterId === id) {
      setExpandedFilterId(null);
    }
  };

  const filterColumns = useMemo(() => {
    // id           – unique React key AND the value stored in f.column
    // physicalCol  – actual table.column sent to the backend for distinct-values query
    // factKey      – fact-side key passed to backend so it returns the right keyColumn
    // label        – display name shown in the dropdown
    const rows: { id: string; physicalCol: string; factKey: string; label: string }[] = [];

    // Tracks semantic labels already added.
    // When multiple dimension tables share the same semantic (e.g. IDEASProduct +
    // IDEASThirdPartyApps both → "Product"), only the first is shown — the backend
    // already COALESCEs them in the generated SQL.
    const seenSemantics = new Set<string>();

    // Tracks full ids to avoid exact duplicates regardless of semantic.
    // When same table is used with different factKeys (e.g. IDEASBoolFlag for
    // IsMSFTTenant and IsStrategicCustomer), the factKey is appended to make each unique.
    const seenIds = new Set<string>();

    datasetColumns.forEach((c) => {
      const semantic = (c.semantic_type || "").toLowerCase();
      const isSemanticDim = c.is_dimension && semantic && semantic !== "time";
      const physicalCol = `${c.table_name}.${c.column_name}`;

      if (isSemanticDim) {
        // Deduplicate by semantic label — multiple tables with the same semantic
        // concept (same factKey) appear only once in the dropdown.
        if (seenSemantics.has(semantic)) return;
        seenSemantics.add(semantic);
      }

      // For semantic dims that share the same physical column but have different
      // factKeys (e.g. IDEASBoolFlag for IsMSFTTenant vs IsStrategicCustomer),
      // append factKey so each concept gets a distinct id.
      const id = isSemanticDim && c.fact_key
        ? `${physicalCol}|${c.fact_key}`
        : physicalCol;

      if (seenIds.has(id)) return;
      seenIds.add(id);

      const label = isSemanticDim && c.semantic_type ? c.semantic_type : c.column_name;
      rows.push({ id, physicalCol, factKey: c.fact_key || "", label });
    });

    return rows;
  }, [datasetColumns]);

  // Reset cached values when switching datasets so we don't
  // reuse distinct lists from a different dataset.
  useEffect(() => {
    setValueCache({});
  }, [selectedDatasetId]);

  const columnDisplayById = useMemo(() => {
    const map = new Map<string, string>();
    filterColumns.forEach((c) => map.set(c.id, c.label));
    return map;
  }, [filterColumns]);

  // Resolve physical column and factKey from a filter column id
  const columnMetaById = useMemo(() => {
    const map = new Map<string, { physicalCol: string; factKey: string }>();
    filterColumns.forEach((c) => map.set(c.id, { physicalCol: c.physicalCol, factKey: c.factKey }));
    return map;
  }, [filterColumns]);

  const buildFilterLabel = (columnId: string, operator: string, value: string) => {
    const colLabel = columnDisplayById.get(columnId) ?? "Column";
    if (!columnId && !value) return "Click to configure filter";
    if (!columnId) return `${operator} ${value || "…"}`;
    return `${colLabel} ${operator} ${value || "…"}`;
  };

  const editingFilter = useMemo(
    () => filters.find((f) => f.id === expandedFilterId) || null,
    [filters, expandedFilterId],
  );

  // Create a stable key representing which filters need loading
  // This prevents infinite loops by only changing when the actual filter structure changes
  const filtersNeedingLoad = useMemo(() => {
    return filters
      .filter(
        (f) =>
          f.column &&
          !f.isLoading &&
          ((f.options.length === 0 && !valueCache[f.column]) ||
            (f.options.length === 1 && !valueCache[f.column]))
      )
      .map((f) => `${f.id}:${f.column}`)
      .join("|");
  }, [filters, valueCache]);

  // Load distinct values for filters whose column is selected but
  // whose options have not yet been populated. We switch the
  // Fabric database once, then fire all distinct-value queries
  // in parallel.
  useEffect(() => {
    if (!selectedDatasetId || !datasetDetail) return;

    // First, hydrate any filters that can use cached values so
    // they don't need to hit the backend again.
    const canUseCache = filters.some(
      (f) => f.column && !f.isLoading && f.options.length === 0 && valueCache[f.column],
    );

    if (canUseCache) {
      setFilters((prev) =>
        prev.map((row) => {
          const cached = row.column ? valueCache[row.column] : undefined;
          if (row.column && !row.isLoading && row.options.length === 0 && cached) {
            return {
              ...row,
              options: cached.options,
              keyColumn: cached.keyColumn ?? row.keyColumn,
            };
          }
          return row;
        }),
      );
    }

    const pending = filters.filter(
      (f) =>
        f.column &&
        !f.isLoading &&
        f.options &&
        (f.options.length === 0 || f.options.length === 1) && // Also reload if only has saved value
        !valueCache[f.column],
    );
    if (pending.length === 0) return;

    const pendingIds = new Set(pending.map((f) => f.id));

    // Mark all pending filters as loading in a single state update
    setFilters((prev) =>
      prev.map((row) =>
        pendingIds.has(row.id)
          ? {
              ...row,
              isLoading: true,
            }
          : row,
      ),
    );

    const loadAll = async () => {
      // Switch database once for this batch
      if (datasetDetail.database_name) {
        try {
          const resSwitch = await msalFetch(`${API_BASE}/api/v1/lab/switch-database`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ database_name: datasetDetail.database_name }),
          });
          const dataSwitch = await resSwitch.json();
          if (!resSwitch.ok || !dataSwitch.success) {
            throw new Error(dataSwitch.error || "Failed to switch database");
          }
        } catch {
          // If switching fails, clear loading state for all pending filters
          setFilters((prev) =>
            prev.map((row) =>
              pendingIds.has(row.id)
                ? {
                    ...row,
                    options: [],
                    isLoading: false,
                  }
                : row,
            ),
          );
          return;
        }
      }

      await Promise.all(
        pending.map(async (f) => {
          try {
            const meta = columnMetaById.get(f.column);
            const physicalCol = meta?.physicalCol ?? f.column;
            const factKey = meta?.factKey ?? "";
            const params = new URLSearchParams({
              dataset_id: String(selectedDatasetId),
              column: physicalCol,
              ...(factKey ? { fact_key: factKey } : {}),
              limit: "100",
            });
            const res = await msalFetch(
              `${API_BASE}/api/v1/sql/distinct-filter-values?${params.toString()}`,
            );
            const data = await res.json();
            if (!res.ok || !data.success) {
              throw new Error(data.error || "Failed to load values");
            }

            const values = Array.isArray(data.values)
              ? data.values
                  .map((v: any) => {
                    if (v == null) return null;
                    // Handle key-value pair objects
                    if (typeof v === "object" && "key" in v && "value" in v) {
                      return {
                        key: String(v.key || ""),
                        value: String(v.value || "")
                      };
                    }
                    // Backwards compatibility: handle plain strings
                    if (
                      typeof v === "string" ||
                      typeof v === "number" ||
                      typeof v === "boolean"
                    ) {
                      const strVal = String(v);
                      return { key: strVal, value: strVal };
                    }
                    return null;
                  })
                  .filter((item: { key: string; value: string } | null): item is { key: string; value: string } => item !== null && item.value !== "")
              : [];

            const keyColumn = data.keyColumn || null;

            console.log('[FilterBuilder] Loaded filter values:', {
              column: f.column,
              keyColumn,
              sampleValues: values.slice(0, 3),
              totalValues: values.length
            });

            // Cache values + keyColumn per column so future filters on the same
            // column reuse this list (including keyColumn for tier 1/2 optimization).
            if (f.column) {
              setValueCache((prev) => ({ ...prev, [f.column!]: { options: values, keyColumn } }));
            }
            setFilters((prev) =>
              prev.map((row) =>
                row.id === f.id
                  ? {
                      ...row,
                      options: values,
                      keyColumn: keyColumn,
                      isLoading: false,
                    }
                  : row,
              ),
            );
          } catch {
            if (f.column) {
              setValueCache((prev) => ({ ...prev, [f.column!]: { options: [], keyColumn: null } }));
            }
            setFilters((prev) =>
              prev.map((row) =>
                row.id === f.id
                  ? {
                      ...row,
                      options: [],
                      isLoading: false,
                    }
                  : row,
              ),
            );
          }
        }),
      );
    };

    void loadAll();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filtersNeedingLoad, selectedDatasetId, datasetDetail]);

  return (
    <div>
      {!selectedDatasetId ? (
        <div className="muted" style={{ fontSize: 13 }}>
          Select a dataset to define filters on its columns.
        </div>
      ) : (
        <div className="chart-filter-card">
          <div className="chart-filter-header">
            <button
              type="button"
              className="chart-filter-add-btn chart-filter-add-btn-primary-left"
              onClick={addFilter}
            >
              + Add filters
            </button>
            <button
              type="button"
              className="chart-filter-add-btn chart-filter-add-btn-secondary chart-filter-add-btn-right"
              onClick={addGroupFilter}
            >
              + Add groups
            </button>
          </div>

          {filters.filter((f) => !f.isPending && (f.groupKey ?? "primary") === "primary").length > 0 && (
            <div className="chart-filter-body">
              <div className="chart-filter-logic-row">
                <button
                  type="button"
                  className="chart-filter-logic-pill"
                  onClick={() =>
                    setFilterLogic(filterLogic === "AND" ? "OR" : "AND")
                  }
                >
                  {filterLogic}
                </button>
              </div>
              <div className="chart-filter-list">
                {filters
                  .filter((filter) => !filter.isPending && (filter.groupKey ?? "primary") === "primary")
                  .map((filter) => {
                  const isExpanded = expandedFilterId === filter.id;
                  const label = buildFilterLabel(
                    filter.column,
                    filter.operator,
                    filter.value,
                  );
                  return (
                    <div key={filter.id} className="chart-filter-list-item">
                      <div className="chart-filter-list-main">
                        <button
                          type="button"
                          className="chart-filter-chip-remove"
                          onClick={() => removeFilter(filter.id)}
                        >
                          ×
                        </button>
                        <button
                          type="button"
                          className="chart-filter-chip"
                          onClick={() =>
                            setExpandedFilterId(isExpanded ? null : filter.id)
                          }
                        >
                          <span className="chart-filter-chip-label">{label}</span>
                          <span className="chart-filter-chip-arrow">›</span>
                        </button>
                      </div>
                      {isExpanded && <div className="chart-filter-chip-active-indicator" />}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
          {filters.filter((f) => !f.isPending && f.groupKey === "secondary").length > 0 && (
            <div className="chart-filter-body" style={{ marginTop: 8 }}>
              <div className="chart-filter-logic-row">
                <span className="chart-filter-logic-pill" style={{ cursor: "default" }}>
                  {filterLogic === "AND" ? "OR group" : "AND group"}
                </span>
              </div>
              <div className="chart-filter-list">
                {filters
                  .filter((filter) => !filter.isPending && filter.groupKey === "secondary")
                  .map((filter) => {
                    const isExpanded = expandedFilterId === filter.id;
                    const label = buildFilterLabel(
                      filter.column,
                      filter.operator,
                      filter.value,
                    );
                    return (
                      <div key={filter.id} className="chart-filter-list-item">
                        <div className="chart-filter-list-main">
                          <button
                            type="button"
                            className="chart-filter-chip-remove"
                            onClick={() => removeFilter(filter.id)}
                          >
                            ×
                          </button>
                          <button
                            type="button"
                            className="chart-filter-chip"
                            onClick={() =>
                              setExpandedFilterId(isExpanded ? null : filter.id)
                            }
                          >
                            <span className="chart-filter-chip-label">{label}</span>
                            <span className="chart-filter-chip-arrow">›</span>
                          </button>
                        </div>
                        {isExpanded && <div className="chart-filter-chip-active-indicator" />}
                      </div>
                    );
                  })}
              </div>
            </div>
          )}
          {editingFilter && (
            <div className="chart-filter-popover">
              <div className="chart-filter-popover-header">Edit filter</div>
              <div className="chart-filter-detail-row">
                <select
                  className="chart-builder-select"
                  value={editingFilter.column}
                  onChange={(e) => {
                    const columnId = e.target.value;
                    const selectedColumn = filterColumns.find((c) => c.id === columnId);

                    updateFilter(editingFilter.id, {
                      column: columnId,
                      columnLabel: selectedColumn?.label || columnId,
                      value: "",
                      valueKey: "",
                      options: [],
                      isLoading: false,
                    });
                  }}
                >
                  <option value="">Column...</option>
                  {filterColumns.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.label}
                    </option>
                  ))}
                </select>
                <select
                  className="chart-builder-select"
                  value={editingFilter.operator}
                  onChange={(e) =>
                    updateFilter(editingFilter.id, { operator: e.target.value })
                  }
                >
                  <option value="=">=</option>
                  <option value="!=">!=</option>
                  <option value=">">&gt;</option>
                  <option value="<">&lt;</option>
                  <option value=">=">&gt;=</option>
                  <option value="<=">&lt;=</option>
                  <option value="IN">IN</option>
                </select>
                {editingFilter.isLoading ? (
                  <div className="chart-filter-value-field">
                    <select className="chart-builder-select" disabled>
                      <option>
                        {editingFilter.value
                          ? `${editingFilter.value} (loading…)`
                          : "Loading values…"}
                      </option>
                    </select>
                    <div className="chart-filter-spinner" aria-label="Loading values" />
                  </div>
                ) : editingFilter.options && editingFilter.options.length > 0 ? (
                  <div className="chart-filter-value-field">
                    <select
                      className="chart-builder-select"
                      value={editingFilter.value || ""}
                      onChange={(e) => {
                        const selectedValue = e.target.value;
                        // Check keyColumn (e.g., "Dims.IDEASProduct.ProductPrimaryKey")
                        const keyColumn = editingFilter.keyColumn || '';
                        // Extract just the column name (last part after the dot)
                        const keyColumnName = keyColumn.split('.').pop() || '';

                        let valueKey: string;

                        // Special case: IDEASBoolFlag dimension has predefined key mappings
                        if (keyColumn.includes('IDEASBoolFlag')) {
                          const boolMapping: Record<string, string> = {
                            'False': '-715022825',
                            'True': '888000370',
                            'AllUp': '-9999',
                            'Unknown': '-1'
                          };
                          valueKey = boolMapping[selectedValue] || selectedValue;
                          console.log(`[FilterBuilder] BoolFlag mapping for "${selectedValue}": ${valueKey}`);
                        }
                        // For keyColumns ending with "Key": compute MMH3 hash from display value
                        else if (keyColumnName.endsWith('Key')) {
                          valueKey = mmh3Hash64(selectedValue);
                          console.log(`[FilterBuilder] Hashing for ${keyColumnName}: "${selectedValue}" -> ${valueKey}`);
                        }
                        // For keyColumns ending with "ID": use the selected value directly (actual ID from dimension)
                        else {
                          valueKey = selectedValue;
                          console.log(`[FilterBuilder] Direct value for ${keyColumnName}: "${selectedValue}"`);
                        }

                        updateFilter(editingFilter.id, {
                          value: selectedValue,
                          valueKey: valueKey
                        });
                      }}
                      disabled={!editingFilter.column}
                    >
                      <option value="">
                        {editingFilter.column ? "Select value" : "Select column first"}
                      </option>
                      {editingFilter.options.map((opt, idx) => (
                        <option key={opt.key || `opt-${idx}`} value={opt.value}>
                          {opt.value}
                        </option>
                      ))}
                    </select>
                  </div>
                ) : (
                  <input
                    className="chart-builder-input"
                    placeholder={
                      editingFilter.operator.toUpperCase() === "IN"
                        ? "Comma-separated values"
                        : "Value"
                    }
                    value={editingFilter.value}
                    onChange={(e) => {
                      const inputValue = e.target.value;
                      // Check keyColumn (e.g., "Dims.IDEASProduct.ProductPrimaryKey")
                      const keyColumn = editingFilter.keyColumn || '';
                      // Extract just the column name (last part after the dot)
                      const keyColumnName = keyColumn.split('.').pop() || '';

                      let valueKey: string;

                      if (!inputValue) {
                        valueKey = '';
                      }
                      // Special case: IDEASBoolFlag dimension has predefined key mappings
                      else if (keyColumn.includes('IDEASBoolFlag')) {
                        const boolMapping: Record<string, string> = {
                          'False': '-715022825',
                          'True': '888000370',
                          'AllUp': '-9999',
                          'Unknown': '-1'
                        };
                        valueKey = boolMapping[inputValue] || inputValue;
                      }
                      // For keyColumns ending with "Key": compute MMH3 hash from display value
                      else if (keyColumnName.endsWith('Key')) {
                        valueKey = mmh3Hash64(inputValue);
                      }
                      // For keyColumns ending with "ID": use the input value directly (actual ID from dimension)
                      else {
                        valueKey = inputValue;
                      }

                      updateFilter(editingFilter.id, {
                        value: inputValue,
                        valueKey: valueKey
                      });
                    }}
                  />
                )}
              </div>
              <div className="chart-filter-popover-actions">
                <button
                  type="button"
                  className="chart-save-modal-btn chart-save-modal-btn-secondary"
                  onClick={() => {
                    if (editingFilter) {
                      const shouldKeep = Boolean(
                        editingFilter.column && editingFilter.value !== "",
                      );
                      if (!shouldKeep) {
                        // If no value was provided, discard the filter entirely.
                        removeFilter(editingFilter.id);
                      } else if (editingFilter.isPending) {
                        // Promote a newly added filter to an active chip.
                        updateFilter(editingFilter.id, { isPending: false });
                      }
                    }
                    setExpandedFilterId(null);
                  }}
                >
                  Apply
                </button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export default FilterBuilder;
