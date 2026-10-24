## What changed and why

<!-- Link the issue or design note this relates to, if any. -->

## Testing

<!-- Paste the commands you ran and their real result, not "tests pass". -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace --all-features`
- [ ] `python crates/pytagshield-python/tests/test_binding.py` (Python changes only)

## Checklist

- [ ] No secrets, credentials, or customer data committed
- [ ] README/docs updated if behavior or known limitations changed
- [ ] New behavior has a test, or the gap is explained
