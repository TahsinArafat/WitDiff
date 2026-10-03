//! `witdiff-mcp`: serve WitDiff over the Model Context Protocol on stdio.
//!
//! See ADR-0014. Diagnostics go to stderr; stdout carries protocol frames only.

use std::io::{self, BufReader};

use witdiff_mcp::Server;

fn main() -> std::process::ExitCode {
    let repo = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let server = Server::new(repo);
    let stdin = BufReader::new(io::stdin().lock());
    let stdout = io::stdout().lock();

    match server.serve(stdin, stdout) {
        Ok(_) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("witdiff-mcp: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
