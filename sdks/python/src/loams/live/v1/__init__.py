"""Marks loams.live.v1 as a package for the wheel.

protoc's Python plugin does not emit __init__.py files, and these
directories are written by sdks/python/tools/bootstrap_facade.py, which is
the Python facade's generator until protoc-gen-loams-facade grows a
`lang=python` renderer.
"""
