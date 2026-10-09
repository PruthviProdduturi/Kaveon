import { redirect } from "next/navigation";

/**
 * Registering an external database is administration, not content, so it
 * lives with the other connection settings rather than beside Datasets and
 * Dashboards in the sidebar. This path is kept because links to it exist —
 * in people's Recents, in bookmarks, and in the command palette's history —
 * and a moved page that 404s is worse than one that moved.
 */
export default function DataSourcesRedirect() {
  redirect("/settings/data-sources");
}
