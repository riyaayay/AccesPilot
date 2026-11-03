# AccessPilot Benchmark Report

This document records the measured verification and performance numbers produced by `make bench` (`cargo run -p ap-harness --bin ap-harness`). All numbers are measured directly against AccessPilot with no placeholders.

---

## 1. Measured Results Table

```
AccessPilot harness  |  2026-11-03  |  8-core Intel / 16GB RAM  |  Postgres 16 (Local/Docker)  |  rustc 1.99.1 (58cf8f9c3 2026-10-13)  |  seed 42
T1  cap_under_concurrency   attempts=5000 accepted=10  denied=4990 captured=4966  cap=5000  violations=0   PASS
T2  idempotency_storm       requests=1000 provider_charges=1 replays=999                                     PASS
T3  fail_closed_matrix      passed=17/17                                                                     PASS
T4  crash_recovery          failpoints=2 runs=200 orphans=0 invariant_failures=0                         PASS
T5  model_and_properties    cases=100000 disagreements=0                                                       PASS
T6  latency                 decide_p99=1us req_p50=6.82ms req_p99=14.31ms rps_spread=4176 rps_single_task=4516       INFO
T7  reconciliation          drifts=50 detected=50 healed=10 incidents=40                                   PASS
T8  anomaly_detection       samples=1000 precision=1.00 recall=1.00                                              PASS
T9  ledger_tamper           mutations=100 detected=100                                                        PASS
T10 hyperswitch_sandbox     payments=10 matched=10 p50=60ms p95=62ms                                         INFO
```

---

## 2. Test Specifications & Verifications

| ID | Test Name | Setup | Pass Criterion | Result |
|---|---|---|---|---|
| **T1** | Cap under concurrency | 1 task, ₹50.00 (`5000` paise) cap, 5,000 concurrent purchase attempts of random ₹1–₹20 (`100`–`2000` paise) amounts | `captured + reserved <= cap`, 0 violations | **PASS** (5,000 attempts, 10 accepted, 4,990 denied, ₹49.66 captured, 0 violations) |
| **T2** | Idempotency storm | 1,000 concurrent requests with identical idempotency key | Simulated PSP charges == 1; 999 replays | **PASS** (1 charge, 999 replays) |
| **T3** | Fail-closed matrix | 17 fault / unknown rows from Section 10.2 of Blueprint | 17 of 17 pass | **PASS** (17/17 passed) |
| **T4** | Crash recovery | Failpoint kills at `after_reserve` and `after_provider_ok` across 200 purchases, followed by reconciler restart | 0 orphans, 0 invariant failures | **PASS** (200 runs, 0 orphans, 0 invariant failures) |
| **T5** | Model & property tests | 100,000 randomized policy and stateful lifecycle sequences compared against reference model | 0 disagreements | **PASS** (100,000 cases, 0 disagreements) |
| **T6** | Latency & throughput | Criterion micro-benchmark for 20-rule `decide()`; 5,000-attempt load run spread over 500 tasks, then single task | `decide_p99` < 200 µs; `req_p99` < 25 ms; 1,000+ rps | **INFO** (`decide_p99`: 1 µs, `req_p50`: 6.82 ms, `req_p99`: 14.31 ms, `rps_spread`: 4,176 rps, `rps_single_task`: 4,516 rps) |
| **T7** | Reconciliation | 50 injected drifts across 5 kinds (10 each) | 50 detected; auto-heal applied only where allowed | **PASS** (50 detected, 10 healed to captured, 40 incidents opened) |
| **T8** | Spend anomaly detector | Synthetic series of 1,000 transactions with 50 injected anomalous spikes | Precision >= 0.95, Recall >= 0.95 | **PASS** (Precision: 1.00, Recall: 1.00) |
| **T9** | Ledger tamper | 100 random single-field mutations across hash-chained ledger events | 100 detected at exact sequence number | **PASS** (100/100 detected at exact mutated seq) |
| **T10** | Hyperswitch sandbox smoke | 10 payments through Dummy Connector (5 successes, 5 declines), retrieved & matched | 10 of 10 match; latency recorded | **INFO** (10/10 matched, p50: 60 ms, p95: 62 ms) |

---

## 3. Methodology Notes

1. **Hardware & Environment**:
   - Host: 8-core Intel CPU, 16 GB RAM
   - Database: PostgreSQL 16 (local daemon on port 9432, connection pool size: 50)
   - Compiler: `rustc 1.99.1`
   - Random Seed: `42`
2. **Isolation**:
   - T1 through T9 measure AccessPilot control plane performance using `SimulatedPsp` (0 ms network delay) to isolate pure engine execution from third-party network jitter.
   - T10 measures network-bound round-trip latency against the Hyperswitch Dummy Connector adapter.
3. **Reproducibility**:
   - Run `make bench` or `cargo run -p ap-harness --bin ap-harness` from workspace root.

