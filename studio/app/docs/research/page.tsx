import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Papers & Patents" };

export default function ResearchDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Research" title="Papers & patents" lead="The technical foundations behind Kaveon's deterministic natural-language analytics, compiled context, curation, and adaptive routing." />
    <Callout type="note">Research documents explain methods, experiments, and product direction. Consult <code>STATUS.md</code> and feature guides before treating a described capability as shipped.</Callout>

    <h2>Technical papers</h2>
    <table><thead><tr><th>Paper</th><th>Focus</th><th>Repository source</th></tr></thead><tbody>
      <tr><td>Data Language Model</td><td>Self-compiling semantic context and deterministic question resolution</td><td><a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/whitepaper-dlm.md">Read paper</a></td></tr>
      <tr><td>DLM Curation at Scale</td><td>Precomputed answer tiers, HLL cuboids, incremental refresh, and serving</td><td><a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/whitepaper-dlm-curation.md">Read paper</a></td></tr>
      <tr><td>Adaptive Context Routing</td><td>Validity scoring and selection between context and live data</td><td><a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/whitepaper-adaptive-context-routing.md">Read paper</a></td></tr>
      <tr><td>Deterministic NL→SQL</td><td>Template patterns, fuzzy matching, dataset selection, and chart choice</td><td><a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/whitepaper-nl-to-sql.md">Read paper</a></td></tr>
    </tbody></table>

    <h2>Patent disclosure</h2>
    <p><a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/patent-adaptive-context-routing.md">The adaptive-context-routing disclosure</a> records the invention, claims, embodiments, and prior-art delta. It is a legal/technical disclosure, not an API contract or deployment guide.</p>

    <h2>Reading paths</h2>
    <ul>
      <li><strong>Product practitioner:</strong> DLM guide → Freshness Algorithm → DLM Curation at Scale.</li>
      <li><strong>Data engineer:</strong> Deterministic NL→SQL → Adaptive Context Routing → Architecture.</li>
      <li><strong>Researcher:</strong> DLM paper → Curation paper → routing paper → patent disclosure.</li>
    </ul>
    <p>The Markdown sources remain the canonical long-form editions and render directly on the repository host. Studio documentation provides the operational interpretation and route into each topic.</p>
    <Pager prev={{ href: "/docs/operations", title: "Operations" }} />
  </div>;
}
