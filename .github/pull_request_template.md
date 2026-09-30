## What and why

<!-- One or two sentences. Link the issue: Closes #123 -->

## Checklist

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test` (any new test is offline, or `#[ignore]`d with a run command in its doc comment)
- [ ] Commit messages use `type : subject` (see CONTRIBUTING.md)
- [ ] README/key docs updated if behaviour changed
- [ ] Tried it in a real terminal, if this touches the TUI or playback
