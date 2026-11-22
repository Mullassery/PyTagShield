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
