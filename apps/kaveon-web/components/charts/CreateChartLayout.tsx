import React, { useState } from "react";

import AdvancedChartOptions from "./AdvancedChartOptions";
import ChartConfigPanel from "./ChartConfigPanel";
import ChartPreview from "./ChartPreview";
import ColumnBrowser from "./ColumnBrowser";
import DatasetSelector from "./DatasetSelector";
import RunQueryButton from "./RunQueryButton";
import SQLPreviewTabs from "./SQLPreviewTabs";

const CreateChartLayout: React.FC = () => {
  const [isSourceCollapsed, setIsSourceCollapsed] = useState(true);
  const [activeCenterTab, setActiveCenterTab] = useState<"data" | "customize">(
    "data",
  );

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
            <div className="chart-builder-tabs-header">
              <button
                type="button"
                className={
                  activeCenterTab === "data"
                    ? "chart-builder-tab chart-builder-tab-active"
                    : "chart-builder-tab"
                }
                onClick={() => setActiveCenterTab("data")}
              >
                Data
              </button>
              <button
                type="button"
                className={
                  activeCenterTab === "customize"
                    ? "chart-builder-tab chart-builder-tab-active"
                    : "chart-builder-tab"
                }
                onClick={() => setActiveCenterTab("customize")}
              >
                Customize
              </button>
            </div>

            <div className="chart-builder-tabs-body">
              <section className="chart-config-panel-card">
                {activeCenterTab === "data" ? (
                  <ChartConfigPanel />
                ) : (
                  <AdvancedChartOptions />
                )}
              </section>
            </div>
          </div>
        </div>

        <div className="chart-builder-center-actions">
          <RunQueryButton />
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
