"use client";

import { LabWorkbench } from "./LabWorkbench";

// SQL Lab is a destination of its own. The Catalog's Query action opens it
// with ?catalog=&schema=&name=&query= so the table arrives preselected.
export default function LabPage() {
  return <LabWorkbench />;
}
