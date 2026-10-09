"use client";

import React, { useCallback, useEffect, useId, useRef, useState } from "react";

export interface CatalogOption {
  id: string;
  /** The name to show. For KaveonDB this is the product name, not the identifier. */
  catalog: string;
  /** Schemas the catalog holds, when the Lab has already counted them. */
  schemas?: number | null;
  /** True for the platform's own catalog, which holds no queryable tables. */
  system?: boolean;
}

export interface CatalogPickerProps {
  options: CatalogOption[];
  value: string | null;
  disabled?: boolean;
  onSelect: (id: string) => void;
  /** A quiet fact about what is selected, shown against the label. */
  meta?: React.ReactNode;
  /** An action belonging to the selection, shown beside the control. */
  action?: React.ReactNode;
}

/**
 * Which catalog the Lab is reading.
 *
 * A native select cannot be styled past its own chrome — the list is drawn by
 * the operating system, in its own typeface, on its own background — so in a
 * dark workbench it reads as a hole. This draws the control and its list, and
 * keeps the keyboard behaviour a select gives for free: the roving focus, the
 * jump to first and last, Escape to leave it as it was, and the fact that the
 * open list is what takes the arrows.
 */
export function CatalogPicker({ options, value, disabled = false, onSelect, meta, action }: CatalogPickerProps) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const listId = useId();

  const selectedIndex = Math.max(0, options.findIndex((option) => option.id === value));
  const selected = options[selectedIndex] ?? null;

  const close = useCallback((restoreFocus = true) => {
    setOpen(false);
    if (restoreFocus) rootRef.current?.querySelector("button")?.focus();
  }, []);

  // A click anywhere else closes the list, as a select's does.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [open]);

  const commit = (index: number) => {
    const option = options[index];
    if (option) onSelect(option.id);
    close();
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (disabled) return;
    if (!open) {
      if (["Enter", " ", "ArrowDown", "ArrowUp"].includes(event.key)) {
        event.preventDefault();
        setActive(selectedIndex);
        setOpen(true);
      }
      return;
    }
    switch (event.key) {
      case "Escape":
        event.preventDefault();
        close();
        break;
      case "ArrowDown":
        event.preventDefault();
        setActive((index) => Math.min(options.length - 1, index + 1));
        break;
      case "ArrowUp":
        event.preventDefault();
        setActive((index) => Math.max(0, index - 1));
        break;
      case "Home":
        event.preventDefault();
        setActive(0);
        break;
      case "End":
        event.preventDefault();
        setActive(options.length - 1);
        break;
      case "Enter":
      case " ":
        event.preventDefault();
        commit(active);
        break;
      case "Tab":
        close(false);
        break;
      default:
        break;
    }
  };

  return (
    <div className="catalog-picker" ref={rootRef} onKeyDown={onKeyDown}>
      <div className="catalog-picker__labelrow">
        <span className="catalog-picker__label" id={`${listId}-label`}>Catalog</span>
        {meta && <span className="catalog-picker__meta">{meta}</span>}
      </div>
      <div className="catalog-picker__row">
      <button
        type="button"
        className="catalog-picker__control"
        disabled={disabled || options.length === 0}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-labelledby={`${listId}-label`}
        onClick={() => {
          if (disabled || options.length === 0) return;
          setActive(selectedIndex);
          setOpen((isOpen) => !isOpen);
        }}
      >
        <i className="fas fa-layer-group catalog-picker__glyph" aria-hidden="true" />
        <span className="catalog-picker__name">
          {selected ? selected.catalog : options.length === 0 ? "No catalog" : "Select a catalog"}
        </span>
        <i
          className={`fas fa-chevron-${open ? "up" : "down"} catalog-picker__caret`}
          aria-hidden="true"
        />
      </button>
      {action}
      </div>

      {open && (
        <ul className="catalog-picker__list" role="listbox" aria-labelledby={`${listId}-label`} tabIndex={-1}>
          {options.map((option, index) => {
            const isSelected = option.id === value;
            return (
              <li
                key={option.id}
                role="option"
                aria-selected={isSelected}
                className={
                  "catalog-picker__option"
                  + (index === active ? " catalog-picker__option--active" : "")
                  + (isSelected ? " catalog-picker__option--selected" : "")
                }
                onMouseEnter={() => setActive(index)}
                onClick={() => commit(index)}
              >
                <span className="catalog-picker__option-name">{option.catalog}</span>
                {/* The platform's own catalog says what it is instead of how
                    many schemas it has: its schema count is not the fact a
                    reader needs when choosing what to query. */}
                {option.system ? (
                  <span className="catalog-picker__option-note">system</span>
                ) : typeof option.schemas === "number" && (
                  <span className="catalog-picker__option-note">
                    {option.schemas} {option.schemas === 1 ? "schema" : "schemas"}
                  </span>
                )}
                {isSelected && <i className="fas fa-check catalog-picker__tick" aria-hidden="true" />}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

export default CatalogPicker;
