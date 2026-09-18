import ProductPage from "./ProductPage";
import { loadBenchmarkFigures } from "../utils/benchmarks";

// The public product page at the site root. The benchmark figures are read
// from public/benchmarks at build time and passed down; a figure whose file
// is absent is not offered, and with none present the section is not
// rendered.
export default async function Page() {
  const benchmarks = await loadBenchmarkFigures();
  return <ProductPage benchmarks={benchmarks} />;
}
