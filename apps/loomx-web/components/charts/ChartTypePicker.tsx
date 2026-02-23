import React, { useEffect, useMemo, useState } from "react";
import { useChartBuilder } from "./ChartBuilderContext";

const ChartTypePicker: React.FC = () => {
  const { categories, templates, chartType, setChartType } = useChartBuilder();

  const [selectedCategoryId, setSelectedCategoryId] = useState<string>("all");

  const categoryOptions = useMemo(
    () => [
      { id: "all", label: "All charts" },
      ...categories.map((c) => ({
        id: c.id,
        label: c.label,
      })),
    ],
    [categories],
  );

  const categoryEmojiById: Record<string, string> = {
    all: "📊",
    Line: "📈",
    Bar: "📊",
    Pie: "🟠",
    Heatmap: "🟪",
    Treemap: "🧩",
    Sunburst: "☀️",
    Funnel: "⏬",
    Custom: "⚙️",
    Dataset: "📋",
  };

  const formatCategoryLabel = (id: string, label: string) => {
    const emoji = categoryEmojiById[id];
    return emoji ? `${emoji}  ${label}` : label;
  };

  const filteredTemplates = useMemo(
    () =>
      templates.filter((t) =>
        selectedCategoryId === "all" ? true : t.category === selectedCategoryId,
      ),
    [templates, selectedCategoryId],
  );

  const templateLabel = (id: string) => {
    const tmpl = templates.find((t) => t.id === id);
    if (!tmpl) return "";
    const emoji = categoryEmojiById[tmpl.category as keyof typeof categoryEmojiById];
    return emoji ? `${emoji}  ${tmpl.name}` : tmpl.name;
  };

  // When a chart type is already selected (e.g. editing an existing chart),
  // auto-select its category so both dropdowns reflect the current config.
  useEffect(() => {
    if (!chartType) return;
    const tmpl = templates.find((t) => t.id === chartType);
    if (!tmpl) return;
    setSelectedCategoryId((prev) => (prev === "all" ? tmpl.category : prev));
  }, [chartType, templates]);

  return (
    <div className="chart-builder-field-group">
      <label className="chart-builder-label" htmlFor="chart-category-select">
        Chart type
      </label>
      <div style={{ display: "flex", gap: 8 }}>
        <select
          id="chart-category-select"
          className="chart-builder-select"
          style={{ maxWidth: "40%" }}
          value={selectedCategoryId}
          onChange={(e) => setSelectedCategoryId(e.target.value)}
        >
          {categoryOptions.map((opt) => (
            <option key={opt.id} value={opt.id}>
              {formatCategoryLabel(opt.id, opt.label)}
            </option>
          ))}
        </select>
        <select
          id="chart-template-select"
          className="chart-builder-select"
          value={chartType ?? ""}
          onChange={(e) => {
            const value = e.target.value as typeof chartType;
            setChartType(value || null);
          }}
        >
          <option value="">Select a chart template...</option>
          {filteredTemplates.map((tmpl) => (
            <option key={tmpl.id} value={tmpl.id}>
              {templateLabel(tmpl.id)}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
};

export default ChartTypePicker;
