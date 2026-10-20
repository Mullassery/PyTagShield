// `#[pymethods]` wraps each method body's return value in `PyResult`, so the
// explicit `PyResult` on signatures looks like a redundant conversion to
// clippy even though it is required for readability and error types.
#![allow(clippy::useless_conversion)]

use std::collections::BTreeMap;
use std::sync::Arc;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use serde_json::{Map, Number, Value};

use pytagshield_core::config::Config;
use pytagshield_core::consent::ConsentStore;
use pytagshield_core::event::EventContext;
use pytagshield_core::metrics::Metrics;
use pytagshield_core::pipeline::Pipeline;
use pytagshield_core::PipelineError;

pyo3::create_exception!(pytagshield, ValidationError, PyValueError);
pyo3::create_exception!(pytagshield, ConfigError, PyValueError);

fn map_error(py: Python<'_>, err: PipelineError) -> PyErr {
    match err {
        PipelineError::Validation(v) => {
            let pyerr = ValidationError::new_err(v.errors.join("; "));
            let _ = pyerr.value_bound(py).setattr("errors", v.errors);
            pyerr
        }
        PipelineError::Config(msg) => ConfigError::new_err(msg),
    }
}

fn py_to_value(obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(Value::Bool(b));
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Ok(Value::Number(i.into()));
    }
    if let Ok(u) = obj.extract::<u64>() {
        return Ok(Value::Number(u.into()));
    }
    if let Ok(f) = obj.extract::<f64>() {
        return Number::from_f64(f)
            .map(Value::Number)
            .ok_or_else(|| PyTypeError::new_err("non-finite float is not valid JSON"));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(Value::String(s));
    }
    if let Ok(d) = obj.extract::<Bound<PyDict>>() {
        let mut map = Map::new();
        for (k, v) in d.iter() {
            let key = k
                .extract::<String>()
                .map_err(|_| PyTypeError::new_err("JSON object keys must be strings"))?;
            map.insert(key, py_to_value(&v)?);
        }
        return Ok(Value::Object(map));
    }
    if let Ok(l) = obj.extract::<Bound<PyList>>() {
        let mut items = Vec::with_capacity(l.len());
        for item in l.iter() {
            items.push(py_to_value(&item)?);
        }
        return Ok(Value::Array(items));
    }
    if let Ok(t) = obj.extract::<Bound<PyTuple>>() {
        let mut items = Vec::with_capacity(t.len());
        for item in t.iter() {
            items.push(py_to_value(&item)?);
        }
        return Ok(Value::Array(items));
    }
    let type_name = obj
        .get_type()
        .name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "?".to_owned());
    Err(PyTypeError::new_err(format!(
        "unsupported type '{type_name}' for JSON conversion"
    )))
}

fn value_to_py<'py>(py: Python<'py>, v: &Value) -> Bound<'py, PyAny> {
    match v {
        Value::Null => py.None().into_bound(py),
        Value::Bool(b) => b.into_py(py).into_bound(py),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_py(py).into_bound(py)
            } else if let Some(u) = n.as_u64() {
                u.into_py(py).into_bound(py)
            } else if let Some(f) = n.as_f64() {
                f.into_py(py).into_bound(py)
            } else {
                py.None().into_bound(py)
            }
        }
        Value::String(s) => s.into_py(py).into_bound(py),
        Value::Array(items) => {
            let list = PyList::empty_bound(py);
            for item in items {
                let _ = list.append(value_to_py(py, item));
            }
            list.into_any()
        }
        Value::Object(map) => {
            let dict = PyDict::new_bound(py);
            for (k, val) in map {
                let _ = dict.set_item(k, value_to_py(py, val));
            }
            dict.into_any()
        }
    }
}

#[pyclass(name = "Config", module = "pytagshield")]
#[derive(Clone)]
struct PyConfig {
    inner: Arc<Config>,
}

#[pymethods]
impl PyConfig {
    /// Parse a configuration from a YAML string.
    #[staticmethod]
    fn from_yaml(yaml: &str) -> PyResult<Self> {
        Config::from_yaml(yaml)
            .map(|c| Self {
                inner: Arc::new(c),
            })
            .map_err(|e| {
                Python::with_gil(|py| map_error(py, e))
            })
    }

    /// Load a configuration from a file path.
    #[staticmethod]
    fn from_path(path: &str) -> PyResult<Self> {
        Config::load(path)
            .map(|c| Self {
                inner: Arc::new(c),
            })
            .map_err(|e| {
                Python::with_gil(|py| map_error(py, e))
            })
    }

    /// Purposes understood by this configuration.
    #[getter]
    fn purposes(&self) -> Vec<String> {
        self.inner.consent.purposes.clone()
    }

    /// Configured default consent policy: "deny" or "allow".
    #[getter]
    fn default_policy(&self) -> &'static str {
        match self.inner.consent.default_policy {
            pytagshield_core::config::DefaultPolicy::Deny => "deny",
            pytagshield_core::config::DefaultPolicy::Allow => "allow",
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "Config(purposes={:?}, default_policy={})",
            self.inner.consent.purposes,
            self.default_policy()
        )
    }
}

#[pyclass(name = "Pipeline", module = "pytagshield")]
struct PyPipeline {
    inner: Pipeline,
}

#[pymethods]
impl PyPipeline {
    #[new]
    fn new(config: &PyConfig) -> Self {
        Self {
            inner: Pipeline::new(
                config.inner.clone(),
                Arc::new(ConsentStore::new()),
                Arc::new(Metrics::default()),
            ),
        }
    }

    /// Process one ingest payload through the full pipeline
    /// (validate -> consent -> transform -> minimize -> route).
    ///
    /// Returns a dict describing the processed event, consent resolution,
    /// routing decisions, and per-stage reports. Raises ValidationError
    /// (with an `errors` list) when the payload is rejected.
    #[pyo3(signature = (payload, *, ip = None, user_agent = None))]
    fn process(
        &self,
        py: Python<'_>,
        payload: Bound<'_, PyAny>,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> PyResult<PyObject> {
        let raw = py_to_value(&payload)?;
        let context = EventContext { ip, user_agent };
        match self.inner.process(&raw, context) {
            Ok(processed) => {
                let v = serde_json::to_value(&processed)
                    .map_err(|e| PyValueError::new_err(e.to_string()))?;
                Ok(value_to_py(py, &v).unbind())
            }
            Err(e) => Err(map_error(py, e)),
        }
    }

    /// Store consent for a distinct id. Unknown purposes are ignored
    /// (reported in `ignored_purposes`). Returns a summary dict.
    #[pyo3(signature = (distinct_id, purposes))]
    fn set_consent(
        &self,
        py: Python<'_>,
        distinct_id: String,
        purposes: Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let raw = py_to_value(&purposes)?;
        let obj = raw
            .as_object()
            .ok_or_else(|| PyTypeError::new_err("purposes must be a dict of str -> bool"))?;

        let trimmed = distinct_id.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > self.inner.config().ingest.max_distinct_id_len
        {
            return Err(PyValueError::new_err(format!(
                "'distinct_id' must be 1..={} characters",
                self.inner.config().ingest.max_distinct_id_len
            )));
        }

        let known = &self.inner.config().consent.purposes;
        let mut ignored: Vec<String> = Vec::new();
        let mut stored = BTreeMap::new();
        for (key, value) in obj {
            match value.as_bool() {
                Some(b) if known.contains(key) => {
                    stored.insert(key.clone(), b);
                }
                Some(_) => ignored.push(key.clone()),
                None => {
                    return Err(PyTypeError::new_err(format!(
                        "consent '{key}' must be a bool"
                    )))
                }
            }
        }

        let evicted = self.inner.consent_store().set(
            trimmed,
            stored.clone(),
            chrono::Utc::now(),
            self.inner.config().consent.store_ttl_secs,
            self.inner.config().consent.store_max_entries,
        );

        let summary = serde_json::json!({
            "stored": true,
            "distinct_id": trimmed,
            "purposes": stored,
            "ignored_purposes": ignored,
            "ttl_secs": self.inner.config().consent.store_ttl_secs,
            "evicted": evicted,
        });
        Ok(value_to_py(py, &summary).unbind())
    }

    /// Read stored consent for a distinct id. Returns None when absent
    /// or expired.
    #[pyo3(signature = (distinct_id,))]
    fn get_consent(&self, py: Python<'_>, distinct_id: String) -> PyResult<Option<PyObject>> {
        let found = self.inner.consent_store().get(
            distinct_id.trim(),
            chrono::Utc::now(),
            self.inner.config().consent.store_ttl_secs,
        );
        Ok(found.map(|purposes| {
            let v = serde_json::json!({
                "distinct_id": distinct_id.trim(),
                "purposes": purposes,
                "source": "stored",
            });
            value_to_py(py, &v).unbind()
        }))
    }

    /// Number of consent entries currently held (expired entries may
    /// still be counted until the next write prunes them).
    #[getter]
    fn consent_entries(&self) -> usize {
        self.inner.consent_store().len()
    }

    /// Current metrics rendered in Prometheus text format.
    fn metrics(&self) -> String {
        self.inner.metrics().render_prometheus()
    }
}

#[pymodule]
fn pytagshield(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<PyConfig>()?;
    m.add_class::<PyPipeline>()?;
    let py = m.py();
    m.add("ValidationError", py.get_type_bound::<ValidationError>())?;
    m.add("ConfigError", py.get_type_bound::<ConfigError>())?;
    Ok(())
}
