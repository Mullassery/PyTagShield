# PyTagShield Python wrapper packaging — design

Date: 2026-11-22

## Intent

Make PyTagShield a normal Python package installable with `pip` and `uv`, while
keeping Rust as the implementation core. The public Python API stays
`import pytagshield` with the same objects and behavior.

## Goals

- `pip install .`, `pip install -e .`, `uv pip install .` and `uv sync` from a
  checkout produce a working `pytagshield` package.
- The built distribution contains a Python wrapper package plus the compiled
  Rust extension at `pytagshield._core`.
- Public API unchanged: `Config`, `Pipeline`, `ValidationError`, `ConfigError`
  and `__version__` are re-exported by the wrapper.
- Version bumped to 0.3.0 (0.2.0 is already on PyPI); README and CONTRIBUTING
  updated for pip + uv; new wheel and sdist uploaded to PyPI.

## Non-goals

- No pure-Python reimplementation of the pipeline.
- No changes to Rust core behavior.
- No multi-platform wheel CI in this release; the uploaded wheel is macOS arm64
  and other platforms install from the sdist with a Rust toolchain.

## Constraints

- Python >=3.10, PyO3 0.22, abi3, maturin >=1.5.
- Publishing uses the maintainer's existing `~/.pypirc`; no credentials in the
  repo or CI for this release.
- PyPI releases are irreversible; 0.2.0 must not be reused.

## Approaches considered

1. **GitHub Actions release pipeline** building Linux/macOS/Windows wheels and
   publishing on a tag. Best end-user coverage, but needs repository token
   configuration and a manual release trigger. Deferred to a follow-up.
2. **Source-install-only documentation.** Smallest change, but Linux and Windows
   users still get no wheel from PyPI.
3. **Pure-Python rewrite.** Rejected: it discards the Rust core that defines the
   project.
4. **Thin Python wrapper over the PyO3 `_core`. Chosen.** Keeps the Rust core,
   makes the distribution a real Python package, and works with both pip and uv.

## Decisions

- `crates/pytagshield-python/src/lib.rs`: rename the `#[pymodule]` entry point
  from `pytagshield` to `_core`. Keep exception and class module names as
  `pytagshield` so public error paths and reprs stay stable.
- New `python/pytagshield/__init__.py`: re-export the five public names from
  `pytagshield._core` and define `__all__`. No fallback implementation.
- `pyproject.toml`: add `python-source = "python"`, set
  `module-name = "pytagshield._core"`, add `features = ["pyo3/extension-module"]`
  and `include = ["LICENSE"]`, bump version to 0.3.0, keep runtime dependencies
  empty.
- Move the binding test to repo-root `tests/test_binding.py`, add assertions that
  the wrapper exports the public API and that `__version__` is present.
- CI: build and install in clean venvs with both `pip install .` and
  `uv pip install .`, then run pytest. Keep the existing Rust job unchanged.
- Docs: README, CONTRIBUTING and the crate README get pip + uv install commands,
  a note that source builds need Rust, and a note that `_core` is internal;
  version references updated to 0.3.0.
- Release: build wheel + sdist with maturin into `dist/`, verify import in a
  fresh venv with both installers, then upload with the local `~/.pypirc`
  credentials.

## Left to builder

- The other queued repos (PyPerfGuard, PyCustomerHub, OmniPilot) get their own
  design docs and version decisions before any code is changed.
- OmniPilot's wrapper shape: a facade over its existing Python modules, since it
  has no PyO3 bindings today. Confirm at its design review.
- If the deferred all-platform wheel pipeline is wanted, it is a separate change
  after this release.
