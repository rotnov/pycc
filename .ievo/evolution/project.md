# Project — Evolution Overlay

(project-wide rules accumulated here; loaded into context via marker block in AGENTS.md)

## 2026-07-24 22:54 UTC — Keep the project roadmap current
**Trigger:** user-defined convention

родамап нужно тоже держать обновленным

## 2026-07-26 12:58 UTC — Check PR state before waiting for CI
**Trigger:** user-observed mistake during PR monitoring

[rotnov/pycc#132](https://github.com/rotnov/pycc/pull/132)
агенты ждут CI а на самом деле там конфликты и CI не стартует, нужно учитывать состояние PR перед тем как проверять CI, согласен?

## 2026-07-26 21:37 UTC — Do not monitor historical pull requests
**Trigger:** user-observed mistake during PR monitoring

а чего ты мониторишь 119 и 125, зачем?
