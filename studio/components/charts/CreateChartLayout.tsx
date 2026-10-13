import React, { useState } from "react";

import AdvancedChartOptions from "./AdvancedChartOptions";
import ChartConfigPanel from "./ChartConfigPanel";
import ChartPreview from "./ChartPreview";
import ColumnBrowser from "./ColumnBrowser";
import DatasetSelector from "./DatasetSelector";
import RunQueryButton from "./RunQueryButton";
import SQLPreviewTabs from "./SQLPreviewTabs";
import { useChartBuilder } from "./ChartBuilderContext";

const CreateChartLayout: React.FC = () => {
  const { readOnly } = useChartBuilder();
  const [isSourceCollapsed, setIsSourceCollapsed] = useState(true);
  const [activeCenterTab, setActiveCenterTab] = useState<"data" | "customize">(
    "data",
  );

  // Customize holds nothing but controls that restyle the chart, so a reader
  // is not given a tab that leads to an inert panel. Data stays, because the
  // chart's configuration is worth reading whether or not it can be changed.
  const centerTab = readOnly ? "data" : activeCenterTab;

  return (
    <div
      className={
        isSourceCollapsed
          ? "chart-builder-layout chart-builder-layout-source-collapsed"
          : "chart-builder-layout"
      }
      style={{ marginTop: 12 }}
    >
        <div
          className={
            isSourceCollapsed
              ? "chart-builder-left chart-builder-left-collapsed"
              : "chart-builder-left"
          }
        >
          {isSourceCollapsed ? (
            <div className="chart-builder-left-header chart-builder-left-header-collapsed">
              <button
                type="button"
                className="chart-builder-collapse-btn"
                onClick={() => setIsSourceCollapsed((prev) => !prev)}
                aria-label="Expand chart source"
              >
                <span aria-hidden="true" className="chart-builder-collapse-arrow">
                  ❯
                </span>
              </button>
              <div className="chart-builder-panel-title chart-builder-panel-title-vertical">
                <span className="chart-builder-panel-title-text-collapsed">
                  <span className="chart-builder-panel-title-text-label">Chart source</span>
                </span>
              </div>
            </div>
          ) : (
            <div className="chart-builder-left-header">
              <div className="chart-builder-panel-title">
                <span className="chart-builder-panel-title-text">
                  <span className="chart-builder-panel-title-text-label">Chart source</span>
                </span>
              </div>
              <button
                type="button"
                className="chart-builder-collapse-btn"
                onClick={() => setIsSourceCollapsed((prev) => !prev)}
                aria-label="Collapse chart source"
              >
                <span aria-hidden="true" className="chart-builder-collapse-arrow">
                  ❮
                </span>
              </button>
            </div>
          )}

          {!isSourceCollapsed && (
            <div className="chart-builder-left-scroll">
              <section className="chart-source-panel">
                <div className="chart-builder-panel-title">Chart source</div>
                <DatasetSelector />
                <ColumnBrowser />
              </section>
            </div>
          )}
        </div>

      <div className="chart-builder-center">
        <div className="chart-builder-center-scroll">
          <div className="chart-builder-tabs">
            {/* One unified card: tabs + config body + Update button */}
            <section className="chart-config-panel-card">
              <div className="chart-builder-tabs-header">
                <button
                  type="button"
                  className={
                    centerTab === "data"
                      ? "chart-builder-tab chart-builder-tab-active"
                      : "chart-builder-tab"
                  }
                  onClick={() => setActiveCenterTab("data")}
                >
                  Data
                </button>
                {!readOnly && (
                  <button
                    type="button"
                    className={
                      centerTab === "customize"
                        ? "chart-builder-tab chart-builder-tab-active"
                        : "chart-builder-tab"
                    }
                    onClick={() => setActiveCenterTab("customize")}
                  >
                    Customize
                  </button>
                )}
              </div>

              <div className="chart-builder-tabs-body">
                {centerTab === "data" ? (
                  <ChartConfigPanel />
                ) : (
                  <AdvancedChartOptions />
                )}
              </div>

              <div className="chart-builder-center-actions">
                <RunQueryButton />
              </div>
            </section>
          </div>
        </div>
      </div>

      <div className="chart-builder-right">
        <div className="chart-builder-right-split">
          <div className="chart-builder-right-preview">
            <ChartPreview />
          </div>
          <div className="chart-builder-right-sql">
            <SQLPreviewTabs />
          </div>
        </div>
      </div>
    </div>
  );
};

export default CreateChartLayout;
