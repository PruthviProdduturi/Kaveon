/**
 * Extracts table names referenced in a SQL query.
 *
 * Parses every FROM and JOIN clause to collect schema.table references.
 * Used as a server-side fallback when the frontend does not supply
 * tables_used — guarantees the field is never null in query_history.
 *
 * Returns a sorted, deduplicated array of normalised "schema.table" strings
 * (brackets and excess whitespace removed, lowercased).
 */
export function extractTablesFromSql(sql: string): string[] {
  const tables = new Set<string>();

  // Strip single-line and multi-line SQL comments then collapse whitespace.
  const stripped = sql
    .replace(/--[^\n]*/g, ' ')
    .replace(/\/\*[\s\S]*?\*\//g, ' ')
    .replace(/\s+/g, ' ')
    .trim();

  // Match FROM or any JOIN variant followed by a table reference.
  // Deliberately excludes subqueries that start with "(" so that
  // "FROM (SELECT ...)" does not produce a spurious match.
  //
  // Captures:
  //   [schema].[table]   →  schema.table
  //   dbo.MyTable        →  dbo.mytable
  //   MyTable            →  mytable
  //   [MyTable]          →  mytable
  const pattern =
    /\b(?:FROM|(?:INNER\s+|LEFT\s+|RIGHT\s+|FULL\s+|CROSS\s+|OUTER\s+)?JOIN)\s+(?!\()(\[?[\w$]+\]?(?:\s*\.\s*\[?[\w$]+\]?)?)/gi;

  let match: RegExpExecArray | null;
  while ((match = pattern.exec(stripped)) !== null) {
    const raw = match[1]
      .replace(/\[|\]/g, '') // strip square brackets
      .replace(/\s+/g, '')   // remove any whitespace around the dot
      .trim()
      .toLowerCase();

    // Skip SQL keywords that can appear after FROM/JOIN in edge cases,
    // temp tables (#tableName), variables (@var), and empty strings.
    if (
      raw &&
      raw.length >= 1 &&
      !raw.startsWith('#') &&
      !raw.startsWith('@') &&
      !/^(select|where|on|and|or|set|values|openquery|opendatasource|openrowset)$/.test(raw)
    ) {
      tables.add(raw);
    }
  }

  return Array.from(tables).sort();
}
