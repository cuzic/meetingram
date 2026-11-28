# Conflict cloud spike: summary

Input: hand-made gold graphs (synthetic), not post flush+consolidate graphs, to isolate the cloud prompt from extraction noise. Each candidate uses its default position pair.

## Candidates per meeting (before any cap)

| meeting | candidates | used |
| --- | --- | --- |
| m1 | 2 | 2 |
| m2 | 2 | 2 |
| m3 | 2 | 2 |
| budget | 6 | 4 |
| design | 6 | 4 |
| one-on-one | 5 | 4 |
| long | 0 | 0 |
| e2e-design | 6 | 4 |

Total candidates: 29.
Cap applied: at most 4 candidates per meeting (first 4 in candidate order) so that candidates x 3 runs x 1 models <= 150.

TooLarge (no call made): 0

Total calls (candidate x run x model): 66

## Per model

Elapsed percentiles are over finished attempts (timeouts excluded); `per call` sums a call's attempts.

| model | calls | attempts | p50 s | p95 s | per-call p50 s | per-call p95 s | input chars min-max | parse failures (final) | retried | timeouts | dropped | downgraded |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| sonnet | 66 | 67 | 19.2 | 25.9 | 19.2 | 26.9 | 4545-9212 | 0 | 1 | 0 | 0 | 0 |

## Input stages

- Full: 22 candidates
- OneUtterancePerNode: 0 candidates
- ShortUtterances: 0 candidates
- NoOtherIssues: 0 candidates
- NoSecondLevelReasons: 0 candidates

## Failures and retries

- sonnet design i5:p-review:p8 run3: final ok=true errors=[Some("NotJson"), None]
