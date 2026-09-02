import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

const repositoryRoot = resolve(import.meta.dirname, "..");
const ignoredDirectories = new Set([".git", ".next", "node_modules", "target"]);
const markdownLinkPattern = /\[[^\]]+\]\(([^)\s]+)\)/g;
const docsRoutePattern = /href:\s*"(\/docs[^"]*)"/g;
const failures = [];

function collectFiles(directory, extension) {
  const files = [];
  for (const entry of readdirSync(directory)) {
    if (ignoredDirectories.has(entry)) continue;
    const fullPath = join(directory, entry);
    if (statSync(fullPath).isDirectory()) files.push(...collectFiles(fullPath, extension));
    else if (fullPath.endsWith(extension)) files.push(fullPath);
  }
  return files;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function markdownSlug(value) {
  return value
    .trim()
    .toLowerCase()
    .replace(/<[^>]+>/g, "")
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .replace(/\s+/g, "-");
}

function markdownAnchors(content) {
  const counts = new Map();
  const anchors = new Set();
  for (const match of content.matchAll(/^#{1,6}\s+(.+)$/gm)) {
    const base = markdownSlug(match[1]);
    const count = counts.get(base) ?? 0;
    counts.set(base, count + 1);
    anchors.add(count === 0 ? base : `${base}-${count}`);
  }
  return anchors;
}

const markdownFiles = collectFiles(repositoryRoot, ".md");
for (const file of markdownFiles) {
  const content = readFileSync(file, "utf8");
  for (const match of content.matchAll(markdownLinkPattern)) {
    const link = match[1];
    if (/^(?:https?:|mailto:)/.test(link)) continue;
    const [pathPart, fragment] = link.split("#", 2);
    const target = pathPart ? resolve(dirname(file), decodeURIComponent(pathPart)) : file;
    if (!existsSync(target)) {
      failures.push(`${relative(repositoryRoot, file)}: missing link target ${link}`);
      continue;
    }
    if (fragment && target.endsWith(".md")) {
      const anchors = markdownAnchors(readFileSync(target, "utf8"));
      if (!anchors.has(decodeURIComponent(fragment).toLowerCase())) {
        failures.push(`${relative(repositoryRoot, file)}: missing anchor ${link}`);
      }
    }
  }
}

const manifestPath = join(repositoryRoot, "studio/components/docs/manifest.ts");
const manifest = readFileSync(manifestPath, "utf8");
const routes = [...manifest.matchAll(docsRoutePattern)].map((match) => match[1]);
for (const route of routes) {
  const suffix = route.slice("/docs".length).replace(/^\//, "");
  const page = join(repositoryRoot, "studio/app/docs", suffix, "page.tsx");
  if (!existsSync(page)) failures.push(`manifest route ${route}: missing ${relative(repositoryRoot, page)}`);
}

const sourceDirectory = join(repositoryRoot, "docs/reference");
const publicDirectory = join(repositoryRoot, "studio/public/docs/architecture");
const sourceSvgs = collectFiles(sourceDirectory, ".svg");
for (const source of sourceSvgs) {
  const publicCopy = join(publicDirectory, relative(sourceDirectory, source));
  const svg = readFileSync(source, "utf8");
  if (!/<title\b/.test(svg) || !/<desc\b/.test(svg)) {
    failures.push(`${relative(repositoryRoot, source)}: SVG requires title and description`);
  }
  if (!existsSync(publicCopy)) failures.push(`${relative(repositoryRoot, source)}: missing public copy`);
  else if (sha256(source) !== sha256(publicCopy)) failures.push(`${relative(repositoryRoot, source)}: public copy differs`);
}

if (failures.length > 0) {
  console.error(`Documentation validation failed (${failures.length}):`);
  failures.forEach((failure) => console.error(`- ${failure}`));
  process.exitCode = 1;
} else {
  console.log(`Documentation validation passed: ${markdownFiles.length} Markdown files, ${routes.length} routes, ${sourceSvgs.length} SVGs.`);
}
