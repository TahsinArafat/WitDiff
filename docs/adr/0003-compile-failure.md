# ADR-0003: Compile failure is not behavioral red evidence

Status: accepted

A changed test that fails to compile against base proves API incompatibility, not necessarily the intended bug. WitDiff v0.1 reports `base_incompatible` instead of `verified`.
