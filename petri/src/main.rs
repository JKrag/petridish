//! Optional first positional arg overrides the state-file path (test hook,
//! mirrors `swab`'s `--state` global flag in spirit though not in exact form —
//! kept a plain positional rather than pulling in `clap` as a dependency for one
//! flag; petri/SPEC.md §10 does not list `clap` among petri's dependencies).
fn main() -> std::io::Result<()> {
    let state_path = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(petri::default_state_path);
    let code = petri::run(&state_path)?;
    std::process::exit(code as i32);
}
