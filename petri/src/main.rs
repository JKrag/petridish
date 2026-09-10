//! Argument handling lives in `petri::parse_args`, not here (issue #31,
//! `PLAN-focus-panel.md` T7): integration tests can only reach the lib crate,
//! and the argv contract is genuinely ambiguous — `petri`'s first positional is
//! the state-file path, a documented test hook the PTY suite depends on, while
//! `--mini` also wants an operand, and there is no `clap` to arbitrate
//! (petri/SPEC.md §10 does not list it). `parse_args`' doc comment is the
//! contract; `petri/tests/s12_mini_resolve.rs` asserts it case by case.
fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match petri::parse_args(&argv) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("petri: {msg}");
            std::process::exit(2);
        }
    };

    // Help goes to stdout and exits 0: it was asked for, so it is this run's output, not a
    // diagnostic. The `Err` branch above is the opposite case — stderr and exit 2 — and the
    // two must not be collapsed, or `petri --help | less` pipes nothing.
    if args.help {
        println!("{}", petri::HELP);
        std::process::exit(0);
    }

    if args.version {
        println!("petri {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    let state_path = args.state_path.unwrap_or_else(petri::default_state_path);

    let code = match args.mini {
        // `current_dir` is read here and passed down, rather than inside
        // `resolve_mini`, per CLAUDE.md's parameter-over-environment rule — it
        // is what makes the resolution testable without touching process-global
        // state. A shell that has deleted the cwd out from under us degrades to
        // the empty path, which resolves to no project and prints the ordinary
        // not-a-project message.
        Some(target) => {
            let cwd = std::env::current_dir().unwrap_or_default();
            petri::run_mini(&state_path, &target, &cwd)?
        }
        None => petri::run(&state_path)?,
    };
    std::process::exit(code as i32);
}
