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
import type { DragEvent as ReactDragEvent, RefObject } from 'react';
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

/** Marks a container's direct child wrapper as one insertion slot. */
export const CHILD_SLOT_ATTR = 'data-child-slot';

/**
 * Where a drop would land inside a container.
 *
 * `index` is the insertion index among the container's children — 0 before the
 * first, `children.length` after the last — and `offset` is where to draw the
 * indicator, in pixels from the content element's top-left along the
 * container's axis.
 */
export interface ContainerDropSlot {
  index: number;
  offset: number;
}

/**
 * How a container lays its children out, and where to find them.
 *
 * `contentRef` is the element whose DIRECT `[data-child-slot]` children are the
 * slots. Direct matters: a row holds columns that carry slots of their own, and
 * an unscoped `querySelectorAll` returns the grandchildren too — measured in
 * the harness, a row of three children reported four slots unscoped and three
 * with `:scope >`.
 */
export interface ContainerDropOptions {
  axis: 'x' | 'y';
  contentRef: RefObject<HTMLElement | null>;
}

/**
 * The insertion index the pointer is nearest, and where to draw it.
 *
 * Midpoints, not edges: the boundary belongs to whichever side of a child's
 * centre the pointer is on, which is the only rule that stays reachable when
 * children are narrow. Verified against real rects in the harness — three
 * 394px-wide slots at 9/419/829 resolved to index 0 up to x=206, index 1 from
 * x=220 to x=616, index 2 to x=1026 and index 3 beyond it, with the offset
 * landing on the leading edge of the slot it would push aside (and on the
 * trailing edge of the last child for an append).
 */
function resolveDropSlot(
  host: HTMLElement,
  axis: 'x' | 'y',
  e: ReactDragEvent
): ContainerDropSlot {
  const slots = Array.from(
    host.querySelectorAll(`:scope > [${CHILD_SLOT_ATTR}]`)
  ) as HTMLElement[];
  const hostRect = host.getBoundingClientRect();
  if (slots.length === 0) return { index: 0, offset: 0 };

  const pos = axis === 'x' ? e.clientX : e.clientY;
  let index = slots.length;
  for (let k = 0; k < slots.length; k += 1) {
    const r = slots[k].getBoundingClientRect();
    const mid = axis === 'x' ? r.left + r.width / 2 : r.top + r.height / 2;
    if (pos < mid) {
      index = k;
      break;
    }
  }

  const r = (index < slots.length ? slots[index] : slots[slots.length - 1]).getBoundingClientRect();
  const edge = index < slots.length
    ? (axis === 'x' ? r.left : r.top)
    : (axis === 'x' ? r.right : r.bottom);
  const raw = axis === 'x' ? edge - hostRect.left : edge - hostRect.top;
  // A row's children are sized `flex: 0 0 (w/12)%` and the resize bars between
  // them are 16px on top of that, so the last child's trailing edge can sit a
  // few pixels outside the content box. Clamp so the indicator is always drawn
  // somewhere the user can see it.
  const span = axis === 'x' ? host.clientWidth : host.clientHeight;
  return { index, offset: Math.max(0, Math.min(span, Math.round(raw))) };
}

/**
 * Makes a row or column accept a drop, at a position.
 *
 * One hook for both containers, because a drop target that behaves differently
 * depending on which container you are over is the defect this is fixing. It
 * decides with the same predicate the model uses — `canContain` for a palette
 * entry, `canMoveItemToContainer` for an existing item — so a container that
 * lights up will take the drop, and one that will not take it never lights up.
 *
 * It also decides WHERE. `moveItemToContainer` has taken an insertion index
 * since D-33 and every caller appended, so nothing could be dropped between two
 * existing children (D-37). The index comes from the pointer against the
 * children's own measured rects, and `dropSlot` is what the container draws its
 * insertion indicator from, so what is shown and what is written are the same
 * number.
 *
 * `stopPropagation` on both dragover and drop matters: the canvas grid is an
 * ancestor and has its own `dropConfig`, and without it a drop meant for the
 * container would also be read as a drop on the canvas.
 */
export function useContainerDropTarget(
  containerId: string,
  containerType: ComponentType,
  enabled: boolean,
  options?: ContainerDropOptions
) {
  const builder = useBuilderContext();
  const {
    canMoveItemToContainer,
    moveItemToContainer,
    addItemToContainer,
    findItem,
  } = useDashboard();
  const [isDropTarget, setIsDropTarget] = useState(false);
  const [dropSlot, setDropSlot] = useState<ContainerDropSlot | null>(null);

  const accepts = (payload: DashboardDragPayload | null): boolean => {
    if (!enabled || !payload) return false;
    return payload.kind === 'new'
      ? canContain(containerType, payload.type)
      : canMoveItemToContainer(payload.itemId, containerId);
  };

  const slotFor = (e: ReactDragEvent): ContainerDropSlot | null => {
    const host = options?.contentRef.current;
    if (!host || !options) return null;
    return resolveDropSlot(host, options.axis, e);
  };

  const clear = () => {
    setIsDropTarget(false);
    setDropSlot(null);
  };

  return {
    /** True while a drag this container would accept is over it. */
    isDropTarget,
    /** The insertion point a release would use, or null when nothing is over it. */
    dropSlot,
    dropProps: {
      onDragOver: (e: ReactDragEvent) => {
        if (!accepts(builder?.pendingDrag.current ?? null)) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = builder?.pendingDrag.current?.kind === 'new' ? 'copy' : 'move';
        if (!isDropTarget) setIsDropTarget(true);
        const next = slotFor(e);
        // Only on a change of boundary. dragover fires continuously, and the
        // resolved slot only moves when the pointer crosses a child's centre,
        // so this re-renders the container a handful of times per drag rather
        // than on every pixel.
        if (next && (next.index !== dropSlot?.index || next.offset !== dropSlot?.offset)) {
          setDropSlot(next);
        }
      },
      onDragLeave: (e: ReactDragEvent) => {
        // Only the container's own boundary counts. Moving the pointer between
        // a container's children fires dragleave for each of them, and reacting
        // to those flickers the highlight off and on.
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        clear();
      },
      onDrop: (e: ReactDragEvent) => {
        const payload = builder?.pendingDrag.current ?? null;
        // Resolved from the release itself rather than from the last dragover,
        // so the drop cannot land one slot behind the indicator.
        const slot = accepts(payload) ? slotFor(e) : null;
        clear();
        if (!accepts(payload) || !payload) return;
        e.preventDefault();
        e.stopPropagation();
        if (builder) builder.pendingDrag.current = null;
        if (payload.kind === 'new') {
          addItemToContainer(containerId, payload.type, payload.config, slot?.index);
          return;
        }
        let index = slot?.index;
        if (index !== undefined && payload.fromParentId === containerId) {
          // Reordering inside one container. `moveItemToContainer` lifts the
          // item out before it inserts, so any boundary AFTER the item's
          // current place is one index lower once it has gone; without this,
          // dragging a child one slot along left it exactly where it was.
          // Checked over every (from, boundary) pair: the child lands on the
          // side of the boundary the indicator showed, and the two boundaries
          // either side of the item itself are no-ops.
          const from = (findItem(containerId)?.children || [])
            .findIndex((c) => c.i === payload.itemId);
          if (from > -1 && index > from) index -= 1;
          if (from > -1 && index === from) return;
        }
        moveItemToContainer(payload.itemId, containerId, index);
      },
    },
  };
}
