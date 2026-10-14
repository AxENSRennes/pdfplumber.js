"""Generators of unit differential cases (docs/verification.md).

Each ``oracle/unit/<module>.py`` runs as ``python -m oracle.unit.<module>`` and
writes typed JSON cases (fixed seed) to ``<cache>/unit/<module>/`` with the
helpers of ``_common.py``; the Rust tests ``crates/pdftrace/tests/<module>_unit.rs``
replay them. ``python -m oracle.unit`` regenerates whatever is missing or stale.
"""
