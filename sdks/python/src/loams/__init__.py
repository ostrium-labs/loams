"""The Loams SDK for Python.

``from loams import Loams`` is the SDK's front door, so this module re-exports
``Loams`` from :mod:`loams.loams`. Without it the package installs but the
documented import fails: Python treats a directory with no ``__init__.py`` as a
namespace package, and a namespace package has no ``Loams`` attribute.
"""

from loams.loams import Loams

__all__ = ["Loams"]