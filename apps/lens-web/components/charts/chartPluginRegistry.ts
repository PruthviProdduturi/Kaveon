/**
 * Chart Plugin Registry
 *
 * Allows new chart types to be registered without modifying core files.
 * Call `registerChartPlugin` at module load time from any file, then
 * import that file in the app entry point.
 *
 * Example:
 *   import { registerChartPlugin } from "./chartPluginRegistry";
 *   registerChartPlugin({
 *     id: "waterfall",
 *     name: "Waterfall",
 *     description: "Shows running total with incremental changes.",
 *     category: "Bar",
 *     previewKind: "waterfall",
 *     buildOptions: ({ rows, columns }) => { ... return echartsOption; },
 *   });
 */

import React from "react";

export interface ChartPluginDef {
  /** Unique ID — must not clash with built-in TEMPLATES ids */
  id: string;
  /** Display name shown in the chart type picker */
  name: string;
  /** Short description shown in the picker */
  description: string;
  /** Category ID — must match an entry in CHART_CATEGORIES or be a new one */
  category: string;
  /** ECharts series type hint used for thumbnail previews */
  previewKind: string;
  /** Optional thumbnail image path */
  thumbnail?: string;
  /**
   * Build an ECharts option object from query result data.
   * Return null to show the empty placeholder.
   */
  buildOptions?: (data: {
    rows: (string | number | null)[][];
    columns: string[];
    advancedOptions: any;
    config: any;
  }) => any | null;
  /**
   * If provided, this React component replaces ReactECharts as the chart renderer.
   * Use for non-ECharts visuals (e.g. plain React, D3, custom HTML).
   */
  Renderer?: React.ComponentType<{
    options: any;
    rows: (string | number | null)[][];
    columns: string[];
  }>;
  /**
   * Optional panel rendered inside the "Customize" tab after built-in options.
   * It receives advancedOptions and setAdvancedOptions from ChartBuilderContext.
   */
  CustomizePanel?: React.ComponentType<{
    advancedOptions: any;
    setAdvancedOptions: (updater: (prev: any) => any) => void;
  }>;
}

const _registry = new Map<string, ChartPluginDef>();

/**
 * Register a new chart type plugin.
 * Safe to call multiple times with the same id — last registration wins.
 */
export function registerChartPlugin(plugin: ChartPluginDef): void {
  _registry.set(plugin.id, plugin);
}

/** Returns all registered external plugins (built-in TEMPLATES not included). */
export function getRegisteredPlugins(): ChartPluginDef[] {
  return Array.from(_registry.values());
}

/** Looks up a plugin by id. Returns undefined if not found. */
export function getPlugin(id: string): ChartPluginDef | undefined {
  return _registry.get(id);
}
