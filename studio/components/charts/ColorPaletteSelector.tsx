import React, { useState } from "react";

const DEFAULT_PALETTES = [
  ["#5470C6", "#91CC75", "#EE6666", "#FAC858", "#73C0DE", "#3BA272", "#FC8452", "#9A60B4", "#EA7CCC"],
  ["#2E91E5", "#E15F99", "#1CA71C", "#FB0D0D", "#DA16FF", "#222A2A", "#B68100", "#750D86", "#EB663B"],
  ["#003F5C", "#58508D", "#BC5090", "#FF6361", "#FFA600"],
];

export interface ColorPaletteSelectorProps {
  value: string[];
  onChange: (palette: string[]) => void;
  /**
   * Show the chart's palette without offering to change it. Set by the
   * builder's read-only mode, where a new palette could not be saved.
   */
  readOnly?: boolean;
}

const ColorPaletteSelector: React.FC<ColorPaletteSelectorProps> = ({ value, onChange, readOnly = false }) => {
  const [custom, setCustom] = useState<string>("");

  // A reader is shown the palette the chart actually uses — not the three it
  // could be switched to, and not a field for a fourth.
  if (readOnly) {
    return (
      <div
        style={{ display: "flex", flexWrap: "wrap", gap: 3 }}
        role="img"
        aria-label={`Chart palette, ${value.length} ${value.length === 1 ? "colour" : "colours"}`}
      >
        {value.map((color, idx) => (
          <span
            key={`${color}-${idx}`}
            title={color}
            style={{
              width: 16,
              height: 16,
              background: color,
              borderRadius: 3,
              display: "inline-block",
              border: "1px solid var(--border)",
            }}
          />
        ))}
      </div>
    );
  }

  return (
    <div>
      <div style={{ display: "flex", gap: 12, marginBottom: 8 }}>
        {DEFAULT_PALETTES.map((palette, idx) => (
          <button
            key={idx}
            style={{
              border: JSON.stringify(value) === JSON.stringify(palette) ? "2px solid #2563eb" : "1px solid #ccc",
              borderRadius: 4,
              padding: 2,
              background: "#fff",
              cursor: "pointer",
              display: "flex",
              gap: 2,
            }}
            onClick={() => onChange(palette)}
            aria-label={`Select palette ${idx + 1}`}
          >
            {palette.map((color) => (
              <span
                key={color}
                style={{
                  width: 16,
                  height: 16,
                  background: color,
                  borderRadius: 2,
                  display: "inline-block",
                  border: "1px solid #eee",
                }}
              />
            ))}
          </button>
        ))}
      </div>
      <div style={{ marginTop: 8 }}>
        <label style={{ fontSize: 13, color: "#555" }}>
          Custom palette (comma-separated hex):
          <input
            type="text"
            value={custom}
            onChange={e => setCustom(e.target.value)}
            placeholder="#123456,#abcdef,..."
            style={{ marginLeft: 8, width: 180 }}
            onBlur={() => {
              const arr = custom.split(",").map(s => s.trim()).filter(Boolean);
              if (arr.every(s => /^#([0-9a-f]{3}){1,2}$/i.test(s))) {
                onChange(arr);
              }
            }}
          />
        </label>
      </div>
    </div>
  );
};

export default ColorPaletteSelector;
