"use client";

import { useParams } from "next/navigation";
import { Inventory } from "../Inventory";
import { SystemCatalog } from "../SystemCatalog";
import { isSystemCatalog } from "../../utils/systemCatalog";

export default function CatalogPage() {
  const params = useParams<{ catalog: string }>();
  const catalog = params.catalog ? decodeURIComponent(params.catalog) : "";
  return isSystemCatalog(catalog) ? <SystemCatalog /> : <Inventory catalogName={catalog} />;
}
