/**
 * BuilderContext
 *
 * Provides a bridge between the DashboardBuilder (which owns the chart picker
 * modal, the available-charts list and the Items palette) and the canvas and
 * row/column components that need to reach back into it from deep in the tree.
 *
 * It also carries the one drag in flight. Everything draggable in the builder
 * speaks the same payload vocabulary — a palette entry that will become a new
 * item, and an existing tile being moved — so every drop target (the canvas
 * grid, a row, a column) reads one shape and decides one way. The alternative
 * was a second mechanism per movement, and three mechanisms is how you end up
 * with a container that accepts one kind of drop and silently ignores another.
 *
 * This context is only provided inside DashboardBuilder – in view-only mode
 * useBuilderContext() returns null, which the components check before rendering
 * edit controls.
 */

import { createContext, useContext, useState } from 'react';
import type { DragEvent as ReactDragEvent } from 'react';
import type { ComponentType, DashboardLayoutItem } from '../../types/dashboard';
import { canContain, useDashboard } from './DashboardContext';

/** MIME type every builder drag is stamped with, so a drop target can recognise it. */
export const DASHBOARD_DRAG_MIME = 'application/x-kaveon-dashboard-item';

/** A palette entry on its way to becoming a new item. */
export interface NewItemDrag {
  kind: 'new';
  type: ComponentType;
  /** Type-specific fields the new item needs — `chartId`, `textConfig`, … */
  config?: Partial<DashboardLayoutItem>;
}

/**
 * An item that already exists, on its way somewhere else: out of a container
 * onto the grid, into a container, or from one container to another.
 */
export interface MoveItemDrag {
  kind: 'move';
  itemId: string;
  type: ComponentType;
  /** The container it is currently in, or undefined when it sits on the grid. */
  fromParentId?: string;
}

export type DashboardDragPayload = NewItemDrag | MoveItemDrag;

export interface BuilderContextValue {
  /** Open the chart picker modal targeting a specific container, or null for root canvas */
  openChartPickerForContainer: (containerId: string | null) => void;
  /**
   * The drag in flight, or null.
   *
   * A ref rather than state on purpose. react-grid-layout asks for the dropped
   * item's dimensions on every `dragover`, and `DataTransfer.getData` is
   * deliberately unreadable until the `drop` event itself (the HTML5 protected
   * mode), so the payload has to live somewhere every drop target can read
   * synchronously — and re-rendering the canvas on each dragover while a drag
   * is in flight would stutter the placeholder.
   */
  pendingDrag: { current: DashboardDragPayload | null };
}

export const BuilderContext = createContext<BuilderContextValue | null>(null);

/**
 * Hook to access builder context.
 * Returns null when used outside of a DashboardBuilder (i.e. in view mode).
 */
export const useBuilderContext = (): BuilderContextValue | null =>
  useContext(BuilderContext);

/**
 * Props that turn an element into an HTML5 drag source for an existing item.
 *
 * Nested children need this because they are not on the canvas grid and have no
 * drag of their own, and because one press cannot start two drags: a grid tile
 * already treats its whole body as a drag surface, so a child inside it gets an
 * explicit grip instead, and `[draggable="true"]` is in the canvas's
 * `DRAG_CANCEL` so using that grip never moves the tile it sits in.
 */
export const moveDragProps = (
  pendingDrag: BuilderContextValue['pendingDrag'],
  item: Pick<DashboardLayoutItem, 'i' | 'type' | 'parentId'>,
  extra?: { onDragStart?: (e: ReactDragEvent) => void }
) => ({
  draggable: true,
  onDragStart: (e: ReactDragEvent) => {
    const payload: MoveItemDrag = {
      kind: 'move',
      itemId: item.i,
      type: item.type,
      fromParentId: item.parentId,
    };
    pendingDrag.current = payload;
    try {
      e.dataTransfer.setData(DASHBOARD_DRAG_MIME, JSON.stringify(payload));
      e.dataTransfer.effectAllowed = 'move';
    } catch {
      // Protected-mode refusals do not matter: `pendingDrag` is what the drop
      // targets read. The dataTransfer copy is for the cursor and for anything
      // outside this tree.
    }
    extra?.onDragStart?.(e);
  },
  // Fires whether the drag was dropped or abandoned, so an aborted drag cannot
  // leave a payload armed for the next unrelated drop.
  onDragEnd: () => { pendingDrag.current = null; },
});

/**
 * Makes a row or column accept a drop.
 *
 * One hook for both containers, because a drop target that behaves differently
 * depending on which container you are over is the defect this is fixing. It
 * decides with the same predicate the model uses — `canContain` for a palette
 * entry, `canMoveItemToContainer` for an existing item — so a container that
 * lights up will take the drop, and one that will not take it never lights up.
 *
 * `stopPropagation` on both dragover and drop matters: the canvas grid is an
 * ancestor and has its own `dropConfig`, and without it a drop meant for the
 * container would also be read as a drop on the canvas.
 */
export function useContainerDropTarget(
  containerId: string,
  containerType: ComponentType,
  enabled: boolean
) {
  const builder = useBuilderContext();
  const { canMoveItemToContainer, moveItemToContainer, addItemToContainer } = useDashboard();
  const [isDropTarget, setIsDropTarget] = useState(false);

  const accepts = (payload: DashboardDragPayload | null): boolean => {
    if (!enabled || !payload) return false;
    return payload.kind === 'new'
      ? canContain(containerType, payload.type)
      : canMoveItemToContainer(payload.itemId, containerId);
  };

  return {
    /** True while a drag this container would accept is over it. */
    isDropTarget,
    dropProps: {
      onDragOver: (e: ReactDragEvent) => {
        if (!accepts(builder?.pendingDrag.current ?? null)) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = builder?.pendingDrag.current?.kind === 'new' ? 'copy' : 'move';
        if (!isDropTarget) setIsDropTarget(true);
      },
      onDragLeave: (e: ReactDragEvent) => {
        // Only the container's own boundary counts. Moving the pointer between
        // a container's children fires dragleave for each of them, and reacting
        // to those flickers the highlight off and on.
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setIsDropTarget(false);
      },
      onDrop: (e: ReactDragEvent) => {
        const payload = builder?.pendingDrag.current ?? null;
        setIsDropTarget(false);
        if (!accepts(payload) || !payload) return;
        e.preventDefault();
        e.stopPropagation();
        if (builder) builder.pendingDrag.current = null;
        if (payload.kind === 'new') addItemToContainer(containerId, payload.type, payload.config);
        else moveItemToContainer(payload.itemId, containerId);
      },
    },
  };
}
