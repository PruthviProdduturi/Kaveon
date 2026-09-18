mod args;
mod auth;
#[allow(dead_code)]
mod client;
mod input;
mod local;
mod output;
mod remote;
#[allow(dead_code)]
mod render;
mod shell;
#[allow(dead_code)]
mod theme;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args::parse_with_config(&args) {
        Ok(args::Command::Help) => print_usage(),
        Ok(args::Command::Version) => println!("kaveon {VERSION}"),
        Ok(args::Command::Run(options)) if options.local => run_local(*options),
        Ok(args::Command::Run(mut options)) => {
            if let Err(error) = remote::run(&mut options) {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("Run 'kaveon --help' for usage.");
            std::process::exit(2);
        }
    }
}

fn run_local(options: args::Options) {
    if let Err(error) = local::run(options) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn print_usage() {
    println!("Usage: kaveon [URL[/catalog/schema]] [OPTIONS]");
    println!();
    println!("Remote coordinator mode is the default.");
    println!();
    println!("Options:");
    println!("      --server <URL>          Coordinator URL (default: http://localhost:8080)");
    println!("      --catalog <NAME>        Session catalog (default: kaveon)");
    println!("      --schema <NAME>         Session schema (default: default)");
    println!("      --user <NAME>           Session user");
    println!("      --source <NAME>         Client source (default: kaveon-cli)");
    println!("      --client-tags <TAGS>    Comma-separated client tags");
    println!("  -e, --execute <SQL>         Execute SQL and exit");
    println!(
        "      --output-format <TYPE>  ALIGNED, AUTO, VERTICAL, MARKDOWN, CSV, TSV, JSON, NULL"
    );
    println!("  -f, --file <PATH>           Execute SQL statements from a UTF-8 file");
    println!(
        "      --ignore-errors         Continue batch statements after errors (exit remains nonzero)"
    );
    println!("      --history-file <PATH>   Persistent interactive history file");
    println!("      --no-history            Disable persistent command history");
    println!("      --editing-mode <MODE>   EMACS (default) or VI");
    println!("      --pager <PROGRAM>       Optional pager; empty disables pagination");
    println!("      --output-format-interactive <TYPE>  Override interactive result format");
    println!("      --disable-auto-suggestion  Disable history suggestions");
    println!("      --theme <NAME>          dark (default), light, or mono");
    println!("      --no-header             Skip the session header");
    println!("      --width <COLS>          Terminal width for result tables (default: detected)");
    println!(
        "      --row-limit <N>         Rows an interactive query without LIMIT shows (default: 1000, max: 10000)"
    );
    println!("      --access-token <TOKEN>  Explicit Engine token (prefer KAVEON_ACCESS_TOKEN)");
    println!("      --auth <MODE>           auto (default), azure-cli, microsoft, or none");
    println!("      --ca-cert <PATH>        Trusted PEM CA (or KAVEON_CA_CERT)");
    println!("      KAVEON_ACCESS_TOKEN     Engine bearer token for unattended use");
    println!("      --timeout <SECONDS>     HTTP request timeout (default: 24h)");
    println!("      --local                 Use the embedded local engine");
    println!("  -d, --data-dir <PATH>       Local Parquet/Delta directory (requires --local)");
    println!("  -c, --config <PATH>         Local catalog config (requires --local)");
    println!("  -V, --version               Print version");
    println!("  -h, --help                  Print help");
    println!();
    println!("Connection defaults: KAVEON_CONFIG or ~/.kaveon_config (key=value).");
    println!("Meta-commands:");
    println!("  .catalogs              List catalogs");
    println!("  .schemas               List schemas in current catalog");
    println!("  .tables                List tables in current schema");
    println!("  .describe <table>      Show table schema (local mode)");
    println!("  .use <catalog.schema>  Switch default catalog/schema");
    println!("  .quit                  Exit");
}
