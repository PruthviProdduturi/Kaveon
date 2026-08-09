/**
 * Dashboard Text Component — rich markdown text block
 *
 * View mode  : renders markdown (bold, italic, links, headings, lists, code, blockquote, hr)
 * Edit mode  : hover shows Edit / Delete toolbar
 *              clicking Edit opens an inline editor with:
 *                - Markdown / Preview tabs
 *                - Formatting toolbar (B, I, code, link, quote, ul, ol, hr)
 *                - Alignment, font-size, color controls
 */

import React, { useState, useEffect, useRef } from 'react';
import { HexColorPicker } from 'react-colorful';
import type { DashboardComponentProps } from '../../../types/dashboard';
import { ConfirmModal } from '../../ConfirmModal';

// ─── Lightweight markdown renderer ────────────────────────────────────────────

function parseInline(text: string, key?: string | number): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  const re = /(\*\*(.+?)\*\*|\*(.+?)\*|`(.+?)`|\[(.+?)\]\((.+?)\))/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) nodes.push(text.slice(last, m.index));
    const k = `${key}-${m.index}`;
    if (m[1].startsWith('**'))
      nodes.push(<strong key={k}>{parseInline(m[2], k + 'b')}</strong>);
    else if (m[1].startsWith('*'))
      nodes.push(<em key={k}>{parseInline(m[3], k + 'i')}</em>);
    else if (m[1].startsWith('`'))
      nodes.push(<code key={k} style={{ background: '#f1f5f9', padding: '1px 5px', borderRadius: 3, fontFamily: 'ui-monospace,monospace', fontSize: '0.88em', color: '#c7254e' }}>{m[4]}</code>);
    else if (m[1].startsWith('['))
      nodes.push(<a key={k} href={m[6]} target={m[6].startsWith('/') ? '_self' : '_blank'} rel="noopener noreferrer" style={{ color: '#2563eb', textDecoration: 'underline', fontWeight: 600 }}>{parseInline(m[5], k + 'l')}</a>);
    last = m.index + m[0].length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function renderMarkdown(text: string): React.ReactNode {
  const lines = text.split('\n');
  const out: React.ReactNode[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      out.push(<div key={i} style={{ height: 8 }} />);
      i++;
      continue;
    }
    if (line.startsWith('### ')) { out.push(<h3 key={i} style={{ fontSize: 14, fontWeight: 700, margin: '10px 0 3px', color: 'inherit' }}>{parseInline(line.slice(4), i)}</h3>); i++; continue; }
    if (line.startsWith('## '))  { out.push(<h2 key={i} style={{ fontSize: 17, fontWeight: 700, margin: '12px 0 4px', color: 'inherit' }}>{parseInline(line.slice(3), i)}</h2>); i++; continue; }
    if (line.startsWith('# '))   { out.push(<h1 key={i} style={{ fontSize: 21, fontWeight: 800, margin: '14px 0 5px', color: 'inherit' }}>{parseInline(line.slice(2), i)}</h1>); i++; continue; }
    if (/^-{3,}$/.test(line.trim())) { out.push(<hr key={i} style={{ border: 'none', borderTop: '1px solid #e2e8f0', margin: '8px 0' }} />); i++; continue; }
    if (line.startsWith('> ')) {
      const qLines: string[] = [];
      while (i < lines.length && lines[i].startsWith('> ')) { qLines.push(lines[i].slice(2)); i++; }
      out.push(
        <blockquote key={`bq-${i}`} style={{ borderLeft: '3px solid #94a3b8', paddingLeft: 12, margin: '6px 0', color: '#64748b', fontStyle: 'italic' }}>
          {qLines.map((ql, qi) => <p key={qi} style={{ margin: 0 }}>{parseInline(ql, qi)}</p>)}
        </blockquote>
      );
      continue;
    }
    if (line.startsWith('- ') || line.startsWith('* ')) {
      const items: React.ReactNode[] = [];
      while (i < lines.length && (lines[i].startsWith('- ') || lines[i].startsWith('* '))) {
        items.push(<li key={i}>{parseInline(lines[i].slice(2), i)}</li>);
        i++;
      }
      out.push(<ul key={`ul-${i}`} style={{ paddingLeft: 20, margin: '4px 0', lineHeight: 1.7 }}>{items}</ul>);
      continue;
    }
    if (/^\d+\. /.test(line)) {
      const items: React.ReactNode[] = [];
      while (i < lines.length && /^\d+\. /.test(lines[i])) {
        items.push(<li key={i}>{parseInline(lines[i].replace(/^\d+\. /, ''), i)}</li>);
        i++;
      }
      out.push(<ol key={`ol-${i}`} style={{ paddingLeft: 20, margin: '4px 0', lineHeight: 1.7 }}>{items}</ol>);
      continue;
    }
    out.push(<p key={i} style={{ margin: '2px 0', lineHeight: 1.7 }}>{parseInline(line, i)}</p>);
    i++;
  }
  return <>{out}</>;
}

// ─── Formatting toolbar actions ────────────────────────────────────────────────

type ToolFn = (text: string, ss: number, se: number) => [string, number, number];

// Toggle markers around the selection: adds them if absent, removes them if
// already present — so clicking Bold twice returns the text to normal instead
// of stacking **** markers.
function toggleWrap(mk: string): ToolFn {
  return (t, ss, se) => {
    const sel = t.slice(ss, se);
    // A) selection itself already includes the markers → strip them
    if (sel.length >= 2 * mk.length && sel.startsWith(mk) && sel.endsWith(mk)) {
      const inner = sel.slice(mk.length, sel.length - mk.length);
      return [t.slice(0, ss) + inner + t.slice(se), ss, ss + inner.length];
    }
    // B) markers sit immediately outside the selection → strip them
    const before = t.slice(ss - mk.length, ss);
    const after = t.slice(se, se + mk.length);
    // For single-char markers (italic *, code `) don't mistake the inner star of
    // a bold ** run for an italic wrapper.
    const ambiguous = mk.length === 1 && (t[ss - 2] === mk || t[se + 1] === mk);
    if (before === mk && after === mk && !ambiguous) {
      return [t.slice(0, ss - mk.length) + sel + t.slice(se + mk.length), ss - mk.length, se - mk.length];
    }
    // C) not wrapped yet → add the markers
    return [t.slice(0, ss) + mk + sel + mk + t.slice(se), ss + mk.length, ss + mk.length + sel.length];
  };
}

// Toggle a line prefix (quote/bullet/numbered): removes it if the line already
// starts with it, otherwise adds it — so a second click clears the formatting.
function togglePrefix(prefix: string): ToolFn {
  return (t, ss) => {
    const ls = t.lastIndexOf('\n', ss - 1) + 1;
    const line = t.slice(ls);
    // Numbered list is a pattern (1. / 2. / …), not a literal prefix.
    const match = prefix === '1. ' ? line.match(/^\d+\.\s/) : (line.startsWith(prefix) ? [prefix] : null);
    if (match) {
      const len = match[0].length;
      const r = t.slice(0, ls) + line.slice(len);
      const c = Math.max(ls, ss - len);
      return [r, c, c];
    }
    const r = t.slice(0, ls) + prefix + t.slice(ls);
    return [r, ss + prefix.length, ss + prefix.length];
  };
}

const TOOLBAR: { icon: string; title: string; fn: ToolFn }[] = [
  { icon: 'fas fa-bold',        title: 'Bold',          fn: toggleWrap('**') },
  { icon: 'fas fa-italic',      title: 'Italic',        fn: toggleWrap('*') },
  { icon: 'fas fa-code',        title: 'Inline code',   fn: toggleWrap('`') },
  { icon: 'fas fa-link',        title: 'Link',          fn: (t, ss, se) => { const sel = t.slice(ss, se) || 'text'; const r = t.slice(0, ss) + `[${sel}](url)` + t.slice(se); return [r, ss + 1, ss + 1 + sel.length]; } },
  { icon: 'fas fa-quote-right', title: 'Blockquote',    fn: togglePrefix('> ') },
  { icon: 'fas fa-list-ul',     title: 'Bullet list',   fn: togglePrefix('- ') },
  { icon: 'fas fa-list-ol',     title: 'Numbered list', fn: togglePrefix('1. ') },
  { icon: 'fas fa-minus',       title: 'Horizontal rule', fn: (t, ss) => { const ls = t.lastIndexOf('\n', ss - 1) + 1; const r = t.slice(0, ls) + '\n---\n' + t.slice(ls); return [r, ls + 5, ls + 5]; } },
];

// ─── Main component ────────────────────────────────────────────────────────────

const FONT_SIZES = [12, 13, 14, 15, 16, 18, 20];
const ALIGNMENTS: { val: 'left' | 'center' | 'right'; icon: string }[] = [
  { val: 'left',   icon: 'fas fa-align-left' },
  { val: 'center', icon: 'fas fa-align-center' },
  { val: 'right',  icon: 'fas fa-align-right' },
];

const DashboardTextComponent: React.FC<DashboardComponentProps> = ({ item, isEditMode, onConfigChange, onRemove }) => {
  const [content,     setContent]     = useState(item.textConfig?.content    || '');
  const [alignment,   setAlignment]   = useState<'left'|'center'|'right'>(item.textConfig?.alignment || 'left');
  const [fontSize,    setFontSize]    = useState(item.textConfig?.fontSize   || 14);
  const [color,       setColor]       = useState(item.textConfig?.color      || '#334155');
  const [isEditing,   setIsEditing]   = useState(false);
  const [tab,         setTab]         = useState<'markdown'|'preview'>('markdown');
  const [hovered,     setHovered]     = useState(false);
  const [showColor,   setShowColor]   = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const colorRef    = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setContent(item.textConfig?.content   || '');
    setAlignment(item.textConfig?.alignment || 'left');
    setFontSize(item.textConfig?.fontSize   || 14);
    setColor(item.textConfig?.color         || '#334155');
  }, [item.textConfig?.content, item.textConfig?.alignment, item.textConfig?.fontSize, item.textConfig?.color]);

  useEffect(() => {
    if (isEditing && tab === 'markdown') {
      requestAnimationFrame(() => textareaRef.current?.focus());
    }
  }, [isEditing, tab]);

  // Close colour picker on outside click
  useEffect(() => {
    if (!showColor) return;
    const h = (e: MouseEvent) => {
      if (colorRef.current && !colorRef.current.contains(e.target as Node)) setShowColor(false);
    };
    document.addEventListener('mousedown', h);
    return () => document.removeEventListener('mousedown', h);
  }, [showColor]);

  const commit = () => {
    setIsEditing(false);
    setShowColor(false);
    onConfigChange?.(item.i, { textConfig: { content, alignment, fontSize, color } });
  };

  const applyFmt = (fn: ToolFn) => {
    const ta = textareaRef.current;
    if (!ta) return;
    const [next, ns, ne] = fn(content, ta.selectionStart, ta.selectionEnd);
    setContent(next);
    requestAnimationFrame(() => { ta.focus(); ta.setSelectionRange(ns, ne); });
  };

  return (
    <>
      <div
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{ position: 'relative', width: '100%', height: '100%', boxSizing: 'border-box' }}
      >
        {/* ── View-mode hover toolbar ── */}
        {isEditMode && hovered && !isEditing && (
          <div style={{
            position: 'absolute', top: 4, right: 8, zIndex: 10,
            display: 'flex', alignItems: 'center', gap: 3,
            background: '#fff', border: '1px solid #e2e8f0',
            borderRadius: 6, padding: '2px 5px',
            boxShadow: '0 2px 8px rgba(0,0,0,0.1)',
          }}>
            <button onClick={() => { setIsEditing(true); setTab('markdown'); }} style={hoverBtn(false)}>
              <i className="fas fa-pencil-alt" style={{ fontSize: 10 }} /> Edit
            </button>
            <div style={{ width: 1, height: 14, background: '#e2e8f0' }} />
            <button onClick={() => setConfirmOpen(true)} style={hoverBtn(true)}>
              <i className="fas fa-trash" />
            </button>
          </div>
        )}

        {isEditing ? (
          /* ── Edit mode ── */
          <div style={{ display: 'flex', flexDirection: 'column', height: '100%', padding: '6px 8px', boxSizing: 'border-box', gap: 6 }}>

            {/* Row 1: tabs + formatting toolbar + style controls + Done */}
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
              {/* Markdown / Preview tabs */}
              <div style={{ display: 'flex', background: '#f1f5f9', borderRadius: 5, padding: 2, gap: 2 }}>
                {(['markdown', 'preview'] as const).map(t => (
                  <button key={t} onClick={() => setTab(t)} style={{
                    padding: '2px 9px', fontSize: 11, fontWeight: 600,
                    background: tab === t ? '#fff' : 'transparent',
                    border: tab === t ? '1px solid #e2e8f0' : '1px solid transparent',
                    borderRadius: 4, cursor: 'pointer',
                    color: tab === t ? '#1e293b' : '#64748b',
                    boxShadow: tab === t ? '0 1px 2px rgba(0,0,0,0.06)' : 'none',
                  }}>{t === 'markdown' ? 'Markdown' : 'Preview'}</button>
                ))}
              </div>

              {/* Formatting buttons */}
              {tab === 'markdown' && (
                <div style={{ display: 'flex', gap: 1, background: '#f8fafc', border: '1px solid #e2e8f0', borderRadius: 5, padding: '1px 3px' }}>
                  {TOOLBAR.map(tb => (
                    <button
                      key={tb.title}
                      title={tb.title}
                      onMouseDown={e => { e.preventDefault(); applyFmt(tb.fn); }}
                      style={{ width: 22, height: 22, display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'transparent', border: 'none', borderRadius: 3, cursor: 'pointer', fontSize: 10, color: '#475569' }}
                      onMouseOver={e => { e.currentTarget.style.background = '#e2e8f0'; }}
                      onMouseOut={e  => { e.currentTarget.style.background = 'transparent'; }}
                    >
                      <i className={tb.icon} />
                    </button>
                  ))}
                </div>
              )}

              {/* Separator */}
              <div style={{ width: 1, height: 20, background: '#e2e8f0' }} />

              {/* Alignment */}
              <div style={{ display: 'flex', gap: 1 }}>
                {ALIGNMENTS.map(a => (
                  <button
                    key={a.val}
                    title={a.val}
                    onClick={() => setAlignment(a.val)}
                    style={{ width: 22, height: 22, display: 'flex', alignItems: 'center', justifyContent: 'center', cursor: 'pointer', fontSize: 10, borderRadius: 3, background: alignment === a.val ? '#dbeafe' : 'transparent', border: alignment === a.val ? '1px solid #93c5fd' : '1px solid transparent', color: alignment === a.val ? '#2563eb' : '#64748b' }}
                  >
                    <i className={a.icon} />
                  </button>
                ))}
              </div>

              {/* Font size */}
              <select
                value={fontSize}
                onChange={e => setFontSize(Number(e.target.value))}
                style={{ height: 22, fontSize: 11, border: '1px solid #e2e8f0', borderRadius: 4, padding: '0 2px', background: '#fff', color: '#475569', cursor: 'pointer' }}
              >
                {FONT_SIZES.map(s => <option key={s} value={s}>{s}px</option>)}
              </select>

              {/* Colour picker */}
              <div ref={colorRef} style={{ position: 'relative' }}>
                <button
                  title="Text colour"
                  onClick={() => setShowColor(v => !v)}
                  style={{ width: 22, height: 22, display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'transparent', border: '1px solid #e2e8f0', borderRadius: 4, cursor: 'pointer', padding: 0 }}
                >
                  <span style={{ width: 14, height: 14, borderRadius: 3, background: color, border: '1px solid rgba(0,0,0,0.15)', display: 'block' }} />
                </button>
                {showColor && (
                  <div style={{ position: 'absolute', top: 26, left: 0, zIndex: 50, boxShadow: '0 4px 16px rgba(0,0,0,0.15)', borderRadius: 8, overflow: 'hidden', border: '1px solid #e2e8f0' }}>
                    <HexColorPicker color={color} onChange={setColor} />
                  </div>
                )}
              </div>

              {/* Done button */}
              <button
                onMouseDown={e => { e.preventDefault(); commit(); }}
                style={{ marginLeft: 'auto', padding: '2px 12px', background: '#2563eb', color: '#fff', border: 'none', borderRadius: 4, cursor: 'pointer', fontSize: 11, fontWeight: 600 }}
              >
                Done
              </button>
            </div>

            {/* Row 2: editor or preview */}
            {tab === 'markdown' ? (
              <textarea
                ref={textareaRef}
                value={content}
                onChange={e => setContent(e.target.value)}
                placeholder={"Write markdown here…\n\n**bold**, *italic*, `code`, [link](url)\n# Heading, - list, > quote, ---"}
                style={{
                  flex: 1, resize: 'none', width: '100%',
                  fontSize: 12, fontFamily: 'ui-monospace, monospace',
                  lineHeight: 1.7, color: '#334155',
                  background: '#fafafa', border: '1px solid #e2e8f0',
                  borderRadius: 6, outline: 'none', padding: '8px 10px',
                  boxSizing: 'border-box',
                }}
              />
            ) : (
              <div style={{
                flex: 1, overflow: 'auto', padding: '8px 10px',
                fontSize, color, textAlign: alignment,
                lineHeight: 1.7, background: '#fff',
                border: '1px solid #e2e8f0', borderRadius: 6,
              }}>
                {content ? renderMarkdown(content) : <span style={{ color: '#94a3b8' }}>Nothing to preview.</span>}
              </div>
            )}
          </div>
        ) : (
          /* ── View mode ── */
          <div
            onClick={() => { if (isEditMode) { setIsEditing(true); setTab('markdown'); } }}
            style={{
              padding: '8px 10px', fontSize, color,
              textAlign: alignment, lineHeight: 1.7,
              cursor: isEditMode ? 'text' : 'default',
              height: '100%', overflow: 'auto', boxSizing: 'border-box',
            }}
          >
            {content
              ? renderMarkdown(content)
              : isEditMode
                ? <span style={{ color: '#94a3b8', fontSize: 13 }}>Click to add text — supports <strong>markdown</strong></span>
                : null
            }
          </div>
        )}
      </div>

      <ConfirmModal
        isOpen={confirmOpen}
        title="Remove text block"
        message="This text block will be removed from the dashboard."
        confirmLabel="Remove"
        onConfirm={() => { setConfirmOpen(false); onRemove?.(item.i); }}
        onCancel={() => setConfirmOpen(false)}
      />
    </>
  );
};

function hoverBtn(danger: boolean): React.CSSProperties {
  return {
    display: 'flex', alignItems: 'center', gap: 4,
    padding: danger ? '2px 6px' : '2px 8px',
    background: 'transparent', border: '1px solid transparent',
    borderRadius: 4, cursor: 'pointer', fontSize: 11,
    color: danger ? '#ef4444' : '#475569', fontWeight: 500,
  };
}

export default DashboardTextComponent;
