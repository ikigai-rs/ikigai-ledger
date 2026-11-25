# Changelog

## 0.26.2

- Refuse unreadable optional arguments instead of silently using defaults. Callers that sent an invalid value and previously received the default now receive a refusal. No persisted state changes.
