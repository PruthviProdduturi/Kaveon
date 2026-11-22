# Documentation validation — 2026-11-22

The repository documentation validator passed:

```text
Documentation validation passed: 761 Markdown files, 40 routes, 9 SVGs.
```

Command:

```powershell
node scripts/validate-docs.mjs
```

This validates links, route references, and SVG/document structure. It is a
documentation integrity gate, not evidence that cloud-only Engine operations
are qualified.
