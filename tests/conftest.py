"""Shared test helpers for the petridish package.

Nothing in this file is required by M0's verify command; later modules
(M3, M4, M5) layer additional fixtures on top (real git repos in tmpdirs,
synthetic Claude transcript dirs, etc.).  Kept here so a future module can
``from tests.conftest import ...`` without a separate helper package.
"""
