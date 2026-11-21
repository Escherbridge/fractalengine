//! `fe` binary: collects argv/env and hands off to the library.

use std::collections::BTreeMap;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let invocation = fe_cli::Invocation {
        argv,
        env,
        stdout: Box::new(std::io::stdout()),
        stderr: Box::new(std::io::stderr()),
    };
    std::process::exit(fe_cli::run(invocation));
}
