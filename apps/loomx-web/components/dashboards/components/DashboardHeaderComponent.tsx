/**
 * Dashboard Header Component
 *
 * Renders a section header (H1, H2, or H3) with inline editing support
 * in edit mode. Used to organize and label dashboard sections.
 */

import React, { useState, useEffect } from 'react';
import type { DashboardComponentProps, HeaderSize } from '../../../types/dashboard';

/**
 * DashboardHeaderComponent
 *
 * Configurable header component for dashboard section titles.
 */
const DashboardHeaderComponent: React.FC<DashboardComponentProps> = ({
  item,
  isEditMode,
  onConfigChange,
}) => {
  const [content, setContent] = useState(item.headerConfig?.content || '');
  const [isEditing, setIsEditing] = useState(false);

  // Sync content when item changes
  useEffect(() => {
    setContent(item.headerConfig?.content || '');
  }, [item.headerConfig?.content]);

  /**
   * Handle content changes
   */
  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setContent(e.target.value);
  };

  /**
   * Handle blur - save changes
   */
  const handleBlur = () => {
    setIsEditing(false);
    if (onConfigChange && content !== item.headerConfig?.content) {
      onConfigChange(item.i, {
        headerConfig: {
          ...item.headerConfig,
          content,
          size: item.headerConfig?.size || 'large',
        },
      });
    }
  };

  /**
   * Handle focus - enter edit mode
   */
  const handleFocus = () => {
    if (isEditMode) {
      setIsEditing(true);
    }
  };

  const size = item.headerConfig?.size || 'large';
  const alignment = item.headerConfig?.alignment || 'left';
  const color = item.headerConfig?.color || undefined;

  /**
   * Get the appropriate header class based on size
   */
  const getHeaderClass = (size: HeaderSize) => {
    switch (size) {
      case 'large':
        return 'dashboard-header-h1';
      case 'medium':
        return 'dashboard-header-h2';
      case 'small':
        return 'dashboard-header-h3';
      default:
        return 'dashboard-header-h2';
    }
  };

  const headerClass = getHeaderClass(size);

  return (
    <div className="dashboard-header-component">
      {isEditMode ? (
        <div
          className="dashboard-header-editable"
          style={{ textAlign: alignment }}
          onFocus={handleFocus}
          onBlur={handleBlur}
        >
          <input
            type="text"
            className={`dashboard-header-input ${headerClass}`}
            value={content}
            onChange={handleChange}
            placeholder="Enter header text..."
            style={{ color }}
          />
        </div>
      ) : (
        <div style={{ textAlign: alignment }}>
          {size === 'large' && (
            <h1 className={headerClass} style={{ color }}>
              {content || 'Header'}
            </h1>
          )}
          {size === 'medium' && (
            <h2 className={headerClass} style={{ color }}>
              {content || 'Header'}
            </h2>
          )}
          {size === 'small' && (
            <h3 className={headerClass} style={{ color }}>
              {content || 'Header'}
            </h3>
          )}
        </div>
      )}
    </div>
  );
};

export default DashboardHeaderComponent;
