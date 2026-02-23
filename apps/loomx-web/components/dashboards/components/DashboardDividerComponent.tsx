/**
 * Dashboard Divider Component
 *
 * Renders a visual divider (horizontal or vertical line) to separate
 * dashboard sections. Supports configurable style, color, and thickness.
 */

import React from 'react';
import type { DashboardComponentProps } from '../../../types/dashboard';

/**
 * DashboardDividerComponent
 *
 * Simple divider component for visual separation.
 */
const DashboardDividerComponent: React.FC<DashboardComponentProps> = ({ item }) => {
  const orientation = item.dividerConfig?.orientation || 'horizontal';
  const thickness = item.dividerConfig?.thickness || 1;
  const color = item.dividerConfig?.color || '#cbd5e1';
  const style = item.dividerConfig?.style || 'solid';

  /**
   * Get CSS class for divider style
   */
  const getStyleClass = () => {
    switch (style) {
      case 'dashed':
        return 'dashboard-divider-dashed';
      case 'dotted':
        return 'dashboard-divider-dotted';
      default:
        return 'dashboard-divider-solid';
    }
  };

  const baseClass =
    orientation === 'horizontal'
      ? 'dashboard-divider-horizontal'
      : 'dashboard-divider-vertical';

  const styleClass = getStyleClass();

  const dividerStyle =
    orientation === 'horizontal'
      ? {
          borderTopWidth: `${thickness}px`,
          borderTopColor: color,
        }
      : {
          borderLeftWidth: `${thickness}px`,
          borderLeftColor: color,
        };

  return (
    <hr
      className={`${baseClass} ${styleClass}`}
      style={dividerStyle}
      aria-hidden="true"
    />
  );
};

export default DashboardDividerComponent;
