# Login & About Page Improvements

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Make Google/Microsoft login buttons show a friendly toast, make `/about` the default landing page with caching, replace fake charts with a real dashboard screenshot, and tighten the layout density.

**Architecture:** Minimal changes — toast component in AuthScreen, middleware redirect update, about page layout overhaul with `next/image` screenshot, caching headers.

**Tech Stack:** Next.js 15, NextAuth, next/image

---

### Task 1: Toast for unimplemented Google/Microsoft login

**Files:**
- Modify: `apps/kaveon-web/components/AuthScreen.tsx`

- [ ] Add toast state + auto-dismiss timer
- [ ] Google/Microsoft buttons show toast instead of calling `signIn`
- [ ] Toast: cyan left-border, "We're flattered you trust us with your {provider} account, but..." + CTA button for GitHub
- [ ] Buttons get `opacity: 0.5` + `cursor: not-allowed` styling
- [ ] Commit

### Task 2: Make `/about` default landing for unauthenticated users

**Files:**
- Modify: `apps/kaveon-web/auth.ts` (authorized callback)
- Modify: `apps/kaveon-web/middleware.ts` (matcher)

- [ ] In `authorized` callback: redirect unauthenticated from `/` to `/about` instead of `/login`
- [ ] Add `/about` to middleware matcher exclusion
- [ ] `/login` still accessible directly
- [ ] Commit

### Task 3: Replace fake ProductShot with real dashboard screenshot

**Files:**
- Create: `apps/kaveon-web/public/product-shot.webp` (screenshot capture)
- Modify: `apps/kaveon-web/app/about/page.tsx` (replace ProductShot)

- [ ] Capture a screenshot of the COVID Impact dashboard
- [ ] Convert to WebP, optimize
- [ ] Replace `ProductShot` component with `next/image` using the screenshot
- [ ] Commit

### Task 4: Tighten about page layout — Superset density

**Files:**
- Modify: `apps/kaveon-web/app/about/page.tsx`

- [ ] Hero: cut vertical padding ~40%, remove animated aperture rotation, keep static mark, everything above fold
- [ ] Features grid: tighter gap (16px), smaller card padding, shorter descriptions
- [ ] Compare table: tighter row height and cell padding
- [ ] CTA: collapse to compact row
- [ ] Section padding from ~88px → ~48px throughout
- [ ] Commit

### Task 5: Caching for about page

**Files:**
- Modify: `apps/kaveon-web/app/about/page.tsx` (convert to static)

- [ ] Remove `"use client"` — make it a server component (no interactive state needed in about)
- [ ] Or if client interactivity needed (anchor links), add cache headers via route segment config
- [ ] Commit
# Archived implementation plan

> Historical design record. Paths, line counts, and implementation-state statements may no longer match the current repository. Use `README.md`, `STATUS.md`, and `ARCHITECTURE.md` as the authoritative sources.
