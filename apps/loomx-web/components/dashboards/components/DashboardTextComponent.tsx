/**
 * Dashboard Text Component
 *
 * Renders a text block with support for inline editing in edit mode.
 * Supports basic markdown-style formatting and configurable alignment.
 */

import React, { useState, useEffect } from 'react';
import type { DashboardComponentProps } from '../../../types/dashboard';

/**
 * DashboardTextComponent
 *
 * Editable text block component for dashboards.
 */
const DashboardTextComponent: React.FC<DashboardComponentProps> = ({
  item,
  isEditMode,
  onConfigChange,
}) => {
  const [content, setContent] = useState(item.textConfig?.content || '');
  const [isEditing, setIsEditing] = useState(false);

  // Sync content when item changes
  useEffect(() => {
    setContent(item.textConfig?.content || '');
  }, [item.textConfig?.content]);

  /**
   * Handle content changes
   */
  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setContent(e.target.value);
  };

  /**
   * Handle blur - save changes
   */
  const handleBlur = () => {
    setIsEditing(false);
    if (onConfigChange && content !== item.textConfig?.content) {
      onConfigChange(item.i, {
        textConfig: {
          ...item.textConfig,
          content,
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

  const alignment = item.textConfig?.alignment || 'left';
  const fontSize = item.textConfig?.fontSize || 14;
  const color = item.textConfig?.color || '#334155';

  return (
    <div className="dashboard-text-component">
      {isEditMode ? (
        <div
          className="dashboard-text-editable"
          onFocus={handleFocus}
          onBlur={handleBlur}
        >
          <textarea
            className="dashboard-text-textarea"
            value={content}
            onChange={handleChange}
            placeholder="Enter text content..."
            style={{
              textAlign: alignment,
              fontSize: `${fontSize}px`,
              color,
            }}
          />
        </div>
      ) : (
        <div
          style={{
            textAlign: alignment,
            fontSize: `${fontSize}px`,
            color,
            lineHeight: 1.6,
            whiteSpace: 'pre-wrap',
          }}
        >
          {content || <span style={{ color: '#94a3b8' }}>No content</span>}
        </div>
      )}
    </div>
  );
};

export default DashboardTextComponent;
