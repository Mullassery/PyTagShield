# Contributing to PyTagShield

Thanks for taking the time to contribute. This is a solo-maintained project,
so review times vary and there is no guaranteed merge or response SLA.

## Before you start

- Read [`README.md`](README.md) for what is implemented today.
- Check [`COMPETITIVE_INTELLIGENCE.md`](docs/internal/COMPETITIVE_INTELLIGENCE.md) before
  proposing a large feature; it records what is deliberately out of scope.
- For security issues, follow [`SECURITY.md`](SECURITY.md) instead of opening
  a public issue.

## Development setup

Requires Rust 1.75+ and CPython 3.10+.

```bash
git clone ../PyTagShield
cd PyTagShield
cargo build
cargo test --workspace
```

Python wrapper and bindings:

```bash
uv sync --extra dev
uv run pytest tests/
```

With pip instead:

```bash
python3 -m venv .venv
source .venv/bin/activate
pip install -e . pytest
pytest tests/
```

## Checks before opening a pull request

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-features
```

CI runs the same checks on every push and pull request.

## Pull request guidance

- Keep changes focused; one behavior change per pull request where possible.
- Add a test for new behavior and for bug fixes.
- Update the README or docs when behavior, configuration, or limitations change.
- Do not describe untested behavior as working.
- Never commit secrets, API keys, or real customer event data.

## License

By contributing, you agree that your contributions are licensed under the
Apache License 2.0, the same license as this project.
