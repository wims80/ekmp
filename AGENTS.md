 ## Working rules

- Keep changes focused on the requested task.
- Preserve existing public behavior unless the task requires changing it.
- Do not add production dependencies without asking first.
- Do not modify generated files directly.
- Do not commit changes unless explicitly asked.

## Rust conventions

- Follow the existing architecture and naming conventions.
- Prefer clear, idiomatic Rust over clever abstractions.
- Avoid unnecessary cloning and broad `allow` attributes.
- Add or update tests when behavior changes.

## Verification

After changing Rust code, run:

- `cargo fmt --check`
- `cargo check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features`

If a command cannot run, explain why in the final response.

## Definition of done

- The requested behavior is implemented.
- Relevant tests pass.
- Formatting and lint checks pass.
- The final response summarizes changed files and any remaining risks.

