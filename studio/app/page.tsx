import ProductPage from "./ProductPage";
import { loadClickBenchFigure } from "../utils/clickbench";

// The public product page at the site root. The benchmark figure is read from
// public/benchmarks at build time and passed down; when the file is absent the
// section is simply not rendered.
export default async function Page() {
  const benchmark = await loadClickBenchFigure();
  return <ProductPage benchmark={benchmark} />;
}
