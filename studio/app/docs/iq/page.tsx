import { PageHeader, Callout, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Kaveon Intelligence" };

export default function KaveonIqDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Intelligence"
        title="Kaveon Intelligence"
        lead="The governed intelligence layer that connects Kaveon’s deterministic Data Language Model, distributed Engine, and Studio experience."
      />

      <Callout type="note">
        Kaveon Intelligence is a capability for individuals, teams, and organizations—not a fourth runtime. The DLM remains the
        deterministic semantic path, the Engine remains the execution path, and Studio remains the
        user-facing workspace.
      </Callout>

      <h2>Three connected pillars</h2>
      <table>
        <thead><tr><th>Pillar</th><th>IQ responsibility</th><th>Primary evidence</th></tr></thead>
        <tbody>
          <tr><td><strong><a href="/docs/dlm">Kaveon DLM</a></strong></td><td>Compile governed dataset context and resolve supported questions deterministically.</td><td>Context version, matched entities, generated SQL, and answer route.</td></tr>
          <tr><td><strong><a href="/docs/engine">Kaveon Engine</a></strong></td><td>Execute analytical SQL and transactional operations over governed storage.</td><td>Plan, stages, workers, resource usage, and query result.</td></tr>
          <tr><td><strong><a href="/docs/studio">Kaveon Studio</a></strong></td><td>Authenticate users and make governed intelligence usable through Chat, SQL Lab, charts, and dashboards.</td><td>Identity, permissions, saved artifact, visualization, and audit record.</td></tr>
        </tbody>
      </table>

      <h2>What Kaveon Intelligence governs</h2>
      <ul>
        <li><strong>Context:</strong> metrics, dimensions, relationships, aliases, source locations, and freshness.</li>
        <li><strong>Resolution:</strong> deterministic matching, supported question classes, and refusal when evidence is insufficient.</li>
        <li><strong>Execution:</strong> the selected source, Engine plan, query limits, cancellation, and result delivery.</li>
        <li><strong>Evidence:</strong> context version, source snapshot, generated SQL, timing, row counts, and audit identity.</li>
        <li><strong>Access:</strong> tenant, catalog, dataset, and column permissions applied before context or SQL is returned.</li>
      </ul>

      <h2>Agent boundary</h2>
      <p>
        External agents may ask Kaveon Intelligence for a grounded answer, but they do not receive unrestricted
        database access. The request is evaluated against identity and permissions, resolved through the
        DLM when supported, and otherwise routed to a bounded Engine query. Every answer carries its
        evidence and refusal reason when Kaveon cannot prove the result.
      </p>

      <h2>Current versus target</h2>
      <p>
        The deterministic DLM path, Engine execution, Studio surfaces, freshness routing, and audit
        metadata are current product capabilities. Cross-source semantic graphs, broader agent tool
        orchestration, and enterprise-wide knowledge federation are target work and are not implied by
        the Kaveon Intelligence name.
      </p>

      <Pager prev={{ href: "/docs/dlm", title: "Data Language Model" }} next={{ href: "/docs/nl-to-sql", title: "DLM · NL→SQL" }} />
    </div>
  );
}
