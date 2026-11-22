# PyTagShield Python Wrapper Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship PyTagShield as a normal Python package installable with pip and uv, with a thin Python wrapper over the Rust core, version 0.3.0, updated docs, and a PyPI release.

**Architecture:** The PyO3 extension moves from top-level `pytagshield` to `pytagshield._core`. A new Python package at `python/pytagshield/` re-exports the public API. maturin builds the wrapper plus extension into one wheel; an sdist lets non-macOS users build from source.

**Tech Stack:** Python 3.10+ (abi3), PyO3 0.22, maturin 1.14, uv, pytest, twine, Cargo/Rust 1.75+.

**Spec:** `docs/superpowers/specs/2026-11-22-python-wrapper-packaging-design.md`

## Global Constraints

- Version is exactly `0.3.0` in `pyproject.toml` and the Cargo workspace.
- `requires-python = ">=3.10"`; extension is abi3 (`abi3-py310`).
- Runtime `dependencies = []`; dev extra keeps pytest and maturin.
- Public names are exactly `Config`, `Pipeline`, `ValidationError`, `ConfigError`, `__version__`.
- Exception and class `__module__` stay `pytagshield` (not `pytagshield._core`).
- `pytagshield._core` is internal; no pure-Python fallback.
- Publishing uses the maintainer's `~/.pypirc`; never write credentials to files or CI config.
- Do not commit unless the user explicitly asks.

## Review Focus

- **A user installs from a checkout with pip on Python 3.10.** Verify in a clean venv that `import pytagshield` works and `Pipeline` can process an event — Task 1 wrapper test plus Task 2 binding test.
- **A user installs with uv instead of pip.** Verify `uv pip install .` yields the same public API — Task 3 CI step and Task 4 verification.
- **Exception identity survives the module move.** `except pytagshield.ValidationError` must catch errors raised inside `_core` — Task 2 test.
- **The sdist is installable without the prebuilt wheel.** Verify it contains the wrapper source and all Rust crate sources, then install it in a fresh venv — Task 4 verification.
- **The published artifact is the new version and wrapper, not the old top-level extension.** Verify wheel contents and PyPI metadata before upload — Task 4 verification.

---

### Task 1: Move the PyO3 module to `_core` and add the Python wrapper package

**Files:**
- Modify: `crates/pytagshield-python/src/lib.rs:311-320`
- Modify: `pyproject.toml:47-50`
- Create: `python/pytagshield/__init__.py`
- Test: `tests/test_wrapper.py`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: importable `pytagshield` package exporting `Config`, `Pipeline`, `ValidationError`, `ConfigError`, `__version__`, `__all__`; internal `pytagshield._core` with the same five objects.

- [ ] **Step 1: Write the failing wrapper test**

Create `tests/test_wrapper.py` with exactly these assertions:

```python
import pytagshield
from pytagshield import _core

def test_public_api_is_reexported_from_core():
    assert pytagshield.__version__
    assert pytagshield.Config is _core.Config
    assert pytagshield.Pipeline is _core.Pipeline
    assert pytagshield.ConfigError is _core.ConfigError
    assert pytagshield.ValidationError is _core.ValidationError
    assert pytagshield.__all__ == [
        "Config", "ConfigError", "Pipeline", "ValidationError", "__version__",
    ]

def test_public_types_keep_public_module_path():
    assert pytagshield.Config.__module__ == "pytagshield"
    assert pytagshield.Pipeline.__module__ == "pytagshield"
```

- [ ] **Step 2: Install the current package and verify the test fails**

Run:
```bash
uv venv --python 3.12 .venv-test
uv pip install --python .venv-test/bin/python -e . pytest
.venv-test/bin/python -m pytest tests/test_wrapper.py -v
```
Expected: FAIL because `pytagshield._core` does not exist yet.

- [ ] **Step 3: Rename the PyO3 module entry point**

In `crates/pytagshield-python/src/lib.rs`, change only `#[pymodule] fn pytagshield(...)` to `#[pymodule] fn _core(...)`. Leave `pyo3::create_exception!(pytagshield, ...)`, the `#[pyclass(..., module = "pytagshield")]` attributes, and `m.add("__version__", ...)` unchanged.

- [ ] **Step 4: Point maturin at the wrapper package**

In `pyproject.toml` `[tool.maturin]`, set:
```toml
python-source = "python"
module-name = "pytagshield._core"
features = ["pyo3/extension-module"]
include = ["LICENSE"]
```
Keep `manifest-path` and `bindings` as they are.

- [ ] **Step 5: Write the wrapper package**

Create `python/pytagshield/__init__.py` with exactly:

```python
from pytagshield._core import (
    Config,
    ConfigError,
    Pipeline,
    ValidationError,
    __version__,
)

__all__ = ["Config", "ConfigError", "Pipeline", "ValidationError", "__version__"]
```

- [ ] **Step 6: Reinstall and verify the test passes**

Run:
```bash
uv pip install --python .venv-test/bin/python --reinstall -e .
.venv-test/bin/python -m pytest tests/test_wrapper.py -v
```
Expected: both tests PASS.

- [ ] **Step 7: Check Rust still builds and lints**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no errors.

### Task 2: Bump to 0.3.0 and move the binding test to the package root

**Files:**
- Modify: `pyproject.toml:7`
- Modify: `Cargo.toml:17`
- Modify: `Cargo.lock` (regenerated)
- Move: `crates/pytagshield-python/tests/test_binding.py` → `tests/test_binding.py`
- Test: `tests/test_binding.py`

**Interfaces:**
- Consumes: wrapper package from Task 1.
- Produces: version 0.3.0 reported by `pytagshield.__version__`; one root-level pytest suite covering the full binding behavior.

- [ ] **Step 1: Add version and error-identity assertions to the binding test**

Move the file to `tests/test_binding.py`, then add immediately after `import pytagshield`:

```python
assert pytagshield.__version__ == "0.3.0"
assert pytagshield.Config is pytagshield._core.Config
```

Keep all 12 existing checks unchanged.

- [ ] **Step 2: Run the moved test to verify it fails on version**

Run: `.venv-test/bin/python -m pytest tests/test_binding.py -v`
Expected: FAIL on `assert pytagshield.__version__ == "0.3.0"` (still 0.2.0).

- [ ] **Step 3: Bump the version in both manifests**

Set `version = "0.3.0"` in `pyproject.toml` and `version = "0.3.0"` under `[workspace.package]` in `Cargo.toml`. Regenerate the lockfile with `cargo check --workspace --all-features`.

- [ ] **Step 4: Verify the whole Python suite passes**

Run:
```bash
uv pip install --python .venv-test/bin/python --reinstall -e .
.venv-test/bin/python -m pytest tests/ -v
```
Expected: all tests PASS and `pytagshield.__version__` is now 0.3.0.

- [ ] **Step 5: Verify the Rust suite**

Run:
```bash
cargo fmt --all -- --check
cargo test --workspace --all-features
```
Expected: both PASS.

### Task 3: Update CI and documentation for pip + uv

**Files:**
- Modify: `.github/workflows/ci.yml:30-52`
- Modify: `README.md:54-62`, `README.md:229-237`, `README.md:250`
- Modify: `CONTRIBUTING.md:14-33`
- Modify: `crates/pytagshield-python/README.md:12-19`

**Interfaces:**
- Consumes: version 0.3.0 and root `tests/` from Task 2.
- Produces: CI that installs via both pip and uv; docs that show both installers and mark `_core` as internal.

- [ ] **Step 1: Replace the Python CI job's install steps**

In `.github/workflows/ci.yml`, keep the matrix (`3.10`, `3.12`) and Rust toolchain steps. Replace the "Install maturin and pytest" and "Build and install the extension" steps with:

```yaml
      - uses: astral-sh/setup-uv@v5
      - name: Install with pip in a clean venv
        run: |
          python -m venv .venv-pip
          .venv-pip/bin/python -m pip install . pytest
          .venv-pip/bin/python -c "import pytagshield; assert pytagshield.__version__ == '0.3.0'"
      - name: Install with uv in a clean venv
        run: |
          uv venv --python ${{ matrix.python-version }} .venv-uv
          uv pip install --python .venv-uv/bin/python . pytest
          .venv-uv/bin/python -c "import pytagshield; assert pytagshield.__version__ == '0.3.0'"
      - name: Run tests
        run: .venv-uv/bin/python -m pytest tests/ -v
```

- [ ] **Step 2: Update README install and development sections**

In `README.md`, keep the existing example unchanged. Change the Quick start install block to show both:

```bash
pip install pytagshield
# or
uv add pytagshield
```

Update the following sentence to say the wheel is cp310-abi3 macOS arm64 today and other platforms install from source with Rust 1.75+. In Development, replace the maturin-only sequence with:

```bash
uv sync --extra dev
uv run pytest tests/
# or, with pip:
python -m venv .venv && source .venv/bin/activate
pip install -e . pytest
pytest tests/
```

Add a line under Quick start: "Current release: **0.3.0**." In the Python API section, add one line stating that `pytagshield._core` is the internal Rust extension and the public names are re-exported by the wrapper. In the roadmap, mark "Python bindings and PyPI package" as done and leave "Publish Linux and Windows wheels" unchecked.

- [ ] **Step 3: Update contributor and crate docs**

In `CONTRIBUTING.md`, replace the `python3 -m venv` / `maturin develop -m ...` block with the same `uv sync --extra dev` / `uv run pytest tests/` commands, and change the test path to `tests/`. In `crates/pytagshield-python/README.md`, update the Install block to show `pip install pytagshield` and `uv add pytagshield`, and add one line stating that `pytagshield._core` is an internal Rust extension and the public API is re-exported by `pytagshield`.

- [ ] **Step 4: Verify docs and CI config are syntactically sound**

Run:
```bash
uv run --with pyyaml python -c "import yaml, pathlib; yaml.safe_load(pathlib.Path('.github/workflows/ci.yml').read_text())"
.venv-test/bin/python -m pytest tests/ -v
```
Expected: YAML parses and all tests PASS.

### Task 4: Build, verify and publish 0.3.0 to PyPI

**Files:**
- Generate: `dist/pytagshield-0.3.0-*.whl`, `dist/pytagshield-0.3.0.tar.gz`
- No source files modified.

**Interfaces:**
- Consumes: all changes from Tasks 1–3.
- Produces: PyPI release `pytagshield` 0.3.0 containing the wrapper and `_core`.

- [ ] **Step 1: Build the wheel and sdist**

Run:
```bash
maturin build --release -m crates/pytagshield-python/Cargo.toml --out dist
maturin sdist -m crates/pytagshield-python/Cargo.toml --out dist
```
Expected: one `dist/pytagshield-0.3.0-cp310-abi3-macosx_11_0_arm64.whl` and one `dist/pytagshield-0.3.0.tar.gz`.

- [ ] **Step 2: Check distribution metadata and contents**

Run:
```bash
twine check dist/pytagshield-0.3.0*
unzip -l dist/pytagshield-0.3.0-*.whl
tar -tzf dist/pytagshield-0.3.0.tar.gz
```
Expected: `twine check` reports PASSED; the wheel contains `pytagshield/__init__.py` and `pytagshield/_core...so`; the sdist contains `python/pytagshield/__init__.py` and `crates/pytagshield-python/src/lib.rs`.

- [ ] **Step 3: Install both artifacts in fresh venvs with both installers**

Run:
```bash
uv venv --python 3.12 .venv-wheel-pip
.venv-wheel-pip/bin/python -m pip install dist/pytagshield-0.3.0-*.whl
.venv-wheel-pip/bin/python -c "import pytagshield; print(pytagshield.__version__, pytagshield.__all__)"

uv venv --python 3.12 .venv-wheel-uv
uv pip install --python .venv-wheel-uv/bin/python dist/pytagshield-0.3.0-*.whl
.venv-wheel-uv/bin/python -c "import pytagshield; print(pytagshield.__version__, pytagshield.__all__)"

uv venv --python 3.12 .venv-sdist
.venv-sdist/bin/python -m pip install dist/pytagshield-0.3.0.tar.gz
.venv-sdist/bin/python -c "import pytagshield; print(pytagshield.__version__, pytagshield.__all__)"
```
Expected: all three print `0.3.0` followed by the five public names.

- [ ] **Step 4: Check PyPI metadata before upload**

Fetch `https://pypi.org/pypi/pytagshield/json` and confirm the latest version is still `0.2.0`, so `0.3.0` is not already taken.

- [ ] **Step 5: Upload using the maintainer's local credentials**

Run:
```bash
twine upload dist/pytagshield-0.3.0*
```
Expected: upload succeeds using `~/.pypirc` with no token printed or stored in the repo.

- [ ] **Step 6: Confirm the release is live**

Fetch `https://pypi.org/pypi/pytagshield/json` and confirm version `0.3.0` is listed with a wheel and an sdist.
