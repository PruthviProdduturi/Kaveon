import { Code, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Quickstart" };

export default function Quickstart() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Getting Started" title="Quickstart" lead="Start Kaveon on your own machine and run a SQL query. You can verify the workspace before connecting any data." />
    <p>Already have access to a running Studio? Sign in and go straight to <a href="#first-query">Run your first query</a>. For a shared installation, see <a href="/docs/installation">Deploy Kaveon</a>.</p>

    <h2>Start the local stack</h2>
    <p>Use Git, Bash and a running Docker daemon with Compose v2. On Windows, run the commands in WSL with Docker integration enabled. Keep ports 3000, 8081 and 8082 available on localhost. The first build compiles the Rust Engine and can take several minutes.</p>
    <Code lang="bash">{`git clone https://github.com/PruthviProdduturi/Kaveon.git
cd Kaveon
bash scripts/kaveon-up.sh
docker compose ps`}</Code>
    <p>The launcher generates local credentials and starts Studio, the API, one coordinator and two workers. With the default Compose settings and no configured sign-in provider, Studio uses a local development identity. Configure sign-in before sharing the installation.</p>

    <h2>Verify the services</h2>
    <Code lang="bash">{`curl -fsS http://localhost:8082/api/health
curl -fsS http://localhost:8081/health`}</Code>
    <p>Open <a href="http://localhost:3000">Studio on localhost</a>. If a check fails, run <code>docker compose logs --tail=80 api engine-coordinator studio</code> from the checkout.</p>

    <h2 id="first-query">Run your first query</h2>
    <p>Open <strong>SQL Lab</strong>, enter the statement below and select <strong>Run</strong>. Expect one row with a column named <code>ready</code> and the value <code>1</code>.</p>
    <Code lang="sql">SELECT 1 AS ready;</Code>

    <h2>Read your own data</h2>
    <p>To use local Parquet files or Delta tables, mount the directory when starting or restarting the stack. Replace the example path with an existing directory on your machine.</p>
    <Code lang="bash">{`bash scripts/kaveon-up.sh --data /absolute/path/to/warehouse`}</Code>
    <p>The coordinator and workers see that directory at <code>/data</code>. Register the catalog source and a table at its mounted location. In SQL Lab, select a registered catalog, expand its schema and use a table’s <strong>Sample</strong> query to check that the workers can read it. On an existing shared deployment, use a table you already have permission to query.</p>
    <p>To ask questions in chat, create a <a href="/docs/datasets">semantic dataset</a> over the table, name its metrics and dimensions, and generate its <a href="/docs/dlm">DLM context</a>. For example, a dataset with a Revenue metric and a Region dimension can answer “Revenue by region.”</p>

    <h2>Stop and resume</h2>
    <Code lang="bash">{`docker compose down
# Resume from this checkout with the same configuration and volumes.
docker compose up -d`}</Code>
    <p>Keep the checkout’s <code>.env</code> and its volumes. Adding <code>--volumes</code> to the stop command deletes named volumes, including the coordinator catalog.</p>
    <Pager prev={{ href: "/docs", title: "Introduction" }} next={{ href: "/docs/installation", title: "Deploy Kaveon" }} />
  </div>;
}
